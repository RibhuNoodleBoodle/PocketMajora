#!/usr/bin/env python3
"""Host fixture verifying PocketMajora telemetry binary parsing and reduction."""
from pathlib import Path
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))

import psp_trace_reduce as reducer  # noqa: E402


def make_event(
    *,
    wake_epoch=1,
    timestamp_us=0,
    seq=1,
    thread_id=0,
    event_type=reducer.EVT_TICK_SIM,
    flags=0,
    pass_id=0,
    tick_id=1,
    buffer_gen=0,
    scene_epoch=1,
    target_id=0,
    val0=0,
    val1=0,
):
    return {
        "wake_epoch": wake_epoch,
        "timestamp_us": timestamp_us,
        "seq": seq,
        "thread_id": thread_id,
        "event_type": event_type,
        "flags": flags,
        "pass_id": pass_id,
        "tick_id": tick_id,
        "buffer_gen": buffer_gen,
        "scene_epoch": scene_epoch,
        "target_id": target_id,
        "val0": val0,
        "val1": val1,
    }


def write_binary_trace(path: Path, events, scope_mask=0x1F, start_us=1000):
    buf = bytearray()
    buf.extend(reducer.HEADER_STRUCT.pack(
        reducer.FILE_MAGIC, 1, reducer.EVENT_STRUCT.size, scope_mask, 0, start_us, b"\x00" * 24
    ))
    for ev in events:
        buf.extend(reducer.EVENT_STRUCT.pack(
            reducer.EVENT_MAGIC,
            ev["wake_epoch"],
            ev["timestamp_us"],
            ev["seq"],
            ev["thread_id"],
            ev["event_type"],
            ev["flags"],
            ev["pass_id"],
            ev["tick_id"],
            ev["buffer_gen"],
            ev["scene_epoch"],
            ev["target_id"],
            ev["val0"],
            ev["val1"],
        ))
    path.write_bytes(bytes(buf))


def main():
    # 1. Synthetic overlapping intervals, repeated framebuffer, missing event,
    #    32-bit timer wrap, wake epoch, and stale generation.
    events = [
        # Tick 1: sim [1000, 11000] (10ms), render_pass 0 [11000, 21000] (10ms),
        # overlapping ge_sync on video worker [15000, 26000] (11ms).
        # Union critical path = [1000, 26000] = 25000 us (NOT 10+10+11 = 31ms!).
        make_event(timestamp_us=1000, seq=1, thread_id=0, event_type=reducer.EVT_TICK_SIM,
                   flags=reducer.FLAG_START, tick_id=1, target_id=1),
        make_event(timestamp_us=11000, seq=2, thread_id=0, event_type=reducer.EVT_TICK_SIM,
                   flags=reducer.FLAG_END, tick_id=1, target_id=1),
        make_event(timestamp_us=11000, seq=3, thread_id=0, event_type=reducer.EVT_RENDER_PASS,
                   flags=reducer.FLAG_START, pass_id=0, tick_id=1, target_id=2),
        make_event(timestamp_us=21000, seq=4, thread_id=0, event_type=reducer.EVT_RENDER_PASS,
                   flags=reducer.FLAG_END, pass_id=0, tick_id=1, target_id=2),
        make_event(timestamp_us=15000, seq=1, thread_id=1, event_type=reducer.EVT_GE_SYNC,
                   flags=reducer.FLAG_START, pass_id=0, tick_id=1, target_id=0x1000),
        make_event(timestamp_us=26000, seq=2, thread_id=1, event_type=reducer.EVT_GE_SYNC,
                   flags=reducer.FLAG_END, pass_id=0, tick_id=1, target_id=0x1000),

        # Queued present for buffer 0x1000 gen 1, observed twice (repeated front!)
        make_event(timestamp_us=26500, seq=3, thread_id=1, event_type=reducer.EVT_PRESENT_QUEUED,
                   flags=reducer.FLAG_INSTANT, tick_id=1, buffer_gen=1, target_id=0x1000),
        make_event(timestamp_us=33333, seq=4, thread_id=1, event_type=reducer.EVT_FRONT_OBSERVED,
                   flags=reducer.FLAG_INSTANT, tick_id=1, buffer_gen=1, target_id=0x1000),
        make_event(timestamp_us=50000, seq=5, thread_id=1, event_type=reducer.EVT_FRONT_OBSERVED,
                   flags=reducer.FLAG_INSTANT, tick_id=1, buffer_gen=1, target_id=0x1000),

        # Stale generation: front observed for buffer 0x1000 gen 99 that was never queued!
        make_event(timestamp_us=51000, seq=6, thread_id=1, event_type=reducer.EVT_FRONT_OBSERVED,
                   flags=reducer.FLAG_INSTANT, tick_id=1, buffer_gen=99, target_id=0x1000),

        # Missing event: unclosed start and an orphan end, plus seq gap (4 -> 6)
        make_event(timestamp_us=52000, seq=6, thread_id=0, event_type=reducer.EVT_ASSET_READ,
                   flags=reducer.FLAG_START, tick_id=2, target_id=0xABCD),
        make_event(timestamp_us=54000, seq=7, thread_id=0, event_type=reducer.EVT_DL_XLATE,
                   flags=reducer.FLAG_END, tick_id=2, target_id=0xDEAD),

        # 32-bit timer wrap inside wake_epoch=1:
        # start at 0xFFFFFFF0 (~4294967280), end at 0x000003E8 (1000) -> 1016 us!
        make_event(timestamp_us=0xFFFFFFF0, seq=8, thread_id=0, event_type=reducer.EVT_RENDER_PASS,
                   flags=reducer.FLAG_START, pass_id=1, tick_id=2, target_id=2),
        make_event(timestamp_us=0x000003E8, seq=9, thread_id=0, event_type=reducer.EVT_RENDER_PASS,
                   flags=reducer.FLAG_END, pass_id=1, tick_id=2, target_id=2),

        # Wake epoch 2: audio playable & underrun, plus memory sample
        make_event(wake_epoch=2, timestamp_us=10000, seq=1, thread_id=2,
                   event_type=reducer.EVT_AUDIO_PRODUCER, flags=reducer.FLAG_START,
                   tick_id=3, val0=1500),
        make_event(wake_epoch=2, timestamp_us=12000, seq=2, thread_id=2,
                   event_type=reducer.EVT_AUDIO_PRODUCER, flags=reducer.FLAG_END,
                   tick_id=3, val0=2058),
        make_event(wake_epoch=2, timestamp_us=13000, seq=1, thread_id=3,
                   event_type=reducer.EVT_AUDIO_UNDERRUN, flags=reducer.FLAG_INSTANT,
                   tick_id=3),
        make_event(wake_epoch=2, timestamp_us=14000, seq=10, thread_id=0,
                   event_type=reducer.EVT_MEM_SAMPLE, flags=reducer.FLAG_INSTANT,
                   tick_id=3, val0=18000000, val1=17500000),
    ]

    with tempfile.TemporaryDirectory(prefix="mm-trace-fixture-") as tmp:
        trace_path = Path(tmp) / "mm-psp-trace.bin"
        csv_path = Path(tmp) / "metrics.csv"
        write_binary_trace(trace_path, events)
        parsed = reducer.read_trace_file(trace_path)
        assert parsed["trailing_bytes"] == 0
        summary = reducer.reduce_events(parsed["events"])
        reducer.write_metrics_csv(summary, csv_path)
        assert csv_path.exists()

    assert summary["channels"]["tick_sim"]["count"] == 1
    assert summary["channels"]["tick_sim"]["max_us"] == 10000
    assert summary["channels"]["ge_sync"]["max_us"] == 11000
    assert summary["channels"]["render_pass"]["count"] == 2
    assert summary["channels"]["render_pass"]["max_us"] == 10000
    # 32-bit wrap interval: (0x100000000 + 1000) - 0xFFFFFFF0 = 1016 us
    assert summary["channels"]["render_pass"]["p50_us"] == (10000 + 1016) / 2.0
    # Union critical path for tick 1 must be 25000 (not 31000)
    assert summary["critical_path_max_us"] == 25000
    # Fronts: 1 distinct, 1 repeated, 1 stale rejected
    assert summary["distinct_fronts"] == 1
    assert summary["repeated_fronts"] == 1
    assert summary["stale_generations"] == 1
    # Missing events and sequence gap
    assert summary["unclosed_starts"] == 1
    assert summary["orphan_ends"] == 1
    assert summary["seq_gaps"] == 1
    # Audio & mem
    assert summary["audio_playable_min"] == 2058
    assert summary["audio_underruns"] == 1
    assert summary["mem_free_min"] == 18000000
    assert summary["mem_largest_min"] == 17500000
    print("Trace reducer: overlapping intervals, repeated front, missing events, 32-bit wrap, wake epoch and stale generation passed")


if __name__ == "__main__":
    main()
