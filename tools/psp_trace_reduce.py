#!/usr/bin/env python3
"""Reduce PocketMajora PSP binary telemetry traces into interval and frame metrics."""
from __future__ import annotations

import argparse
import csv
import json
import math
import struct
from collections import defaultdict
from pathlib import Path

FILE_MAGIC = b"MMTR0001"
EVENT_MAGIC = 0x4D543031
HEADER_STRUCT = struct.Struct("<8sIIIIQ24s")
EVENT_STRUCT = struct.Struct("<IIQIHHIHIIQQ")
# Check exact struct layout matches C MmPspTraceEvent (56 bytes):
# u32 magicVer, u32 wakeEpoch, u64 timestampUs, u32 seq, u16 threadId, u16 eventType,
# u16 flags, u16 passId, u32 tickId, u16 bufferGen, u16 sceneEpoch, u32 targetId,
# u64 val0, u64 val1 -> <IIQ I H H H H I H H I Q Q = 56 bytes!
EVENT_STRUCT = struct.Struct("<IIQIHHHHIHHIQQ")
assert HEADER_STRUCT.size == 56
assert EVENT_STRUCT.size == 56

THREAD_NAMES = {
    0: "main",
    1: "video",
    2: "audio_prod",
    3: "audio_out",
    4: "other",
}

EVT_NONE = 0
EVT_TICK_FRAME = 1
EVT_TICK_SIM = 2
EVT_RENDER_PASS = 3
EVT_DL_XLATE = 4
EVT_GFX_SUBMIT = 5
EVT_GE_SYNC = 6
EVT_VBLANK_WAIT = 7
EVT_PRESENT_QUEUED = 8
EVT_FRONT_OBSERVED = 9
EVT_AUDIO_PRODUCER = 10
EVT_AUDIO_OUTPUT = 11
EVT_AUDIO_UNDERRUN = 12
EVT_ASSET_READ = 13
EVT_ASSET_LOCK_WAIT = 14
EVT_SCENE_INIT = 15
EVT_OBJECT_REQ = 16
EVT_OBJECT_READY = 17
EVT_ACTOR_FIRST_DRAW = 18
EVT_ACTOR_PENDING = 19
EVT_MEM_SAMPLE = 20
EVT_INPUT_EDGE = 21
EVT_STATE_HASH = 22
EVT_WAKE_EPOCH = 23
EVT_AI_COL_ANIM = 24
EVT_ACTOR_CTOR = 25
EVT_ASSET_XFORM = 26
EVT_TEX_UPLOAD = 27
EVT_ME_CMD = 28
EVT_TRACE_SUMMARY = 29
EVT_TRACE_FLUSH = 30

EVENT_NAMES = {
    EVT_TICK_FRAME: "tick_frame",
    EVT_TICK_SIM: "tick_sim",
    EVT_RENDER_PASS: "render_pass",
    EVT_DL_XLATE: "dl_xlate",
    EVT_GFX_SUBMIT: "gfx_submit",
    EVT_GE_SYNC: "ge_sync",
    EVT_VBLANK_WAIT: "vblank_wait",
    EVT_PRESENT_QUEUED: "present_queued",
    EVT_FRONT_OBSERVED: "front_observed",
    EVT_AUDIO_PRODUCER: "audio_producer",
    EVT_AUDIO_OUTPUT: "audio_output",
    EVT_AUDIO_UNDERRUN: "audio_underrun",
    EVT_ASSET_READ: "asset_read",
    EVT_ASSET_LOCK_WAIT: "asset_lock_wait",
    EVT_SCENE_INIT: "scene_init",
    EVT_OBJECT_REQ: "object_req",
    EVT_OBJECT_READY: "object_ready",
    EVT_ACTOR_FIRST_DRAW: "actor_first_draw",
    EVT_ACTOR_PENDING: "actor_pending",
    EVT_MEM_SAMPLE: "mem_sample",
    EVT_INPUT_EDGE: "input_edge",
    EVT_STATE_HASH: "state_hash",
    EVT_WAKE_EPOCH: "wake_epoch",
    EVT_AI_COL_ANIM: "ai_col_anim",
    EVT_ACTOR_CTOR: "actor_ctor",
    EVT_ASSET_XFORM: "asset_xform",
    EVT_TEX_UPLOAD: "tex_upload",
    EVT_ME_CMD: "me_cmd",
    EVT_TRACE_SUMMARY: "trace_summary",
}

FLAG_START = 1 << 0
FLAG_END = 1 << 1
FLAG_INSTANT = 1 << 2
FLAG_SYNC_FALLBACK = 1 << 3
FLAG_OUTLIER = 1 << 4
FLAG_SCOPED = 1 << 5
FLAG_URGENT = 1 << 6
FLAG_CACHE_HIT = 1 << 7
FLAG_ERROR = 1 << 8
FLAG_OUTLIER_COPY = 1 << 9


def extend_32bit_timestamps(events):
    """Extend 32-bit timestamps monotonically within each wake_epoch."""
    high_by_epoch = {}
    last_by_epoch = {}
    out = []
    for ev in events:
        epoch = ev["wake_epoch"]
        ts = ev["timestamp_us"]
        # If ts fits in 32-bit range and wrapped relative to previous sample in same epoch
        if ts < (1 << 32):
            last = last_by_epoch.get(epoch)
            if last is not None and ts + 0x80000000 < last:
                high_by_epoch[epoch] = high_by_epoch.get(epoch, 0) + (1 << 32)
            last_by_epoch[epoch] = ts
            ev = dict(ev)
            ev["timestamp_us"] = high_by_epoch.get(epoch, 0) + ts
        out.append(ev)
    return out


def percentile(values, pct):
    if not values:
        return 0.0
    s = sorted(values)
    idx = (len(s) - 1) * pct / 100.0
    lo = int(math.floor(idx))
    hi = int(math.ceil(idx))
    if lo == hi:
        return float(s[lo])
    frac = idx - lo
    return float(s[lo] * (1.0 - frac) + s[hi] * frac)


def merge_union_duration(intervals):
    """Compute non-overlapping union duration of (start, end) intervals."""
    if not intervals:
        return 0
    sorted_iv = sorted((min(a, b), max(a, b)) for a, b in intervals)
    total = 0
    cur_s, cur_e = sorted_iv[0]
    for s, e in sorted_iv[1:]:
        if s <= cur_e:
            if e > cur_e:
                cur_e = e
        else:
            total += cur_e - cur_s
            cur_s, cur_e = s, e
    total += cur_e - cur_s
    return total


def read_trace_file(path: Path):
    data = path.read_bytes()
    if len(data) < HEADER_STRUCT.size:
        raise ValueError(f"Trace file too small: {path}")
    magic, version, rec_size, scope_mask, _res0, start_us, _res1 = HEADER_STRUCT.unpack_from(data, 0)
    if magic != FILE_MAGIC:
        raise ValueError(f"Bad trace magic: {magic!r}")
    if rec_size != EVENT_STRUCT.size:
        raise ValueError(f"Unexpected record size {rec_size}, expected {EVENT_STRUCT.size}")
    events = []
    offset = HEADER_STRUCT.size
    trailing = (len(data) - offset) % EVENT_STRUCT.size
    limit = len(data) - trailing
    while offset < limit:
        (magic_ver, wake_epoch, ts_us, seq, thread_id, event_type, flags, pass_id,
         tick_id, buffer_gen, scene_epoch, target_id, val0, val1) = EVENT_STRUCT.unpack_from(data, offset)
        offset += EVENT_STRUCT.size
        if magic_ver != EVENT_MAGIC:
            continue
        events.append({
            "wake_epoch": wake_epoch,
            "timestamp_us": ts_us,
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
        })
    return {
        "version": version,
        "scope_mask": scope_mask,
        "start_time_us": start_us,
        "trailing_bytes": trailing,
        "events": events,
    }


def reduce_events(raw_events):
    events = extend_32bit_timestamps(raw_events)

    open_spans = {}
    paired_spans = defaultdict(list)
    unclosed_starts = 0
    orphan_ends = 0
    stale_generations = 0

    seq_last = {}
    seq_gaps = 0
    ring_summaries = {}

    queued_presents = {}  # (target_id, buffer_gen) -> ts
    observed_fronts = []  # (ts, addr, buffer_gen, is_distinct_new_gen)
    last_observed_front = None
    distinct_fronts = 0
    repeated_fronts = 0

    audio_playable = []
    audio_underruns = 0
    mem_free_min = None
    mem_largest_min = None
    obj_reqs = {}
    obj_ready_latencies = []
    state_hashes = []
    input_edges = 0
    tick_work_intervals = defaultdict(list)  # (wake_epoch, tick_id) -> list of (start, end)
    render_pass_intervals = defaultdict(list)

    for ev in events:
        tid = ev["thread_id"]
        seq = ev["seq"]
        etype = ev["event_type"]
        flags = ev["flags"]
        epoch = ev["wake_epoch"]
        ts = ev["timestamp_us"]

        if etype == EVT_TRACE_SUMMARY:
            ring_summaries[THREAD_NAMES.get(tid, str(tid))] = {
                "total_events": ev["val0"],
                "overflow_events": ev["val1"],
            }
            continue

        if flags & FLAG_OUTLIER_COPY:
            continue

        if seq != 0:
            prev_seq = seq_last.get((epoch, tid))
            if prev_seq is not None and seq > prev_seq + 1:
                seq_gaps += (seq - prev_seq - 1)
            seq_last[(epoch, tid)] = seq

        if flags & FLAG_START:
            key = (epoch, tid, etype)
            stack = open_spans.setdefault(key, [])
            stack.append(ev)
        elif flags & FLAG_END:
            key = (epoch, tid, etype)
            stack = open_spans.get(key)
            start_ev = stack.pop() if stack else None
            if start_ev is None:
                orphan_ends += 1
            elif ts >= start_ev["timestamp_us"]:
                dur = ts - start_ev["timestamp_us"]
                name = EVENT_NAMES.get(etype, f"evt_{etype}")
                paired_spans[name].append(dur)
                if etype in (EVT_TICK_SIM, EVT_RENDER_PASS, EVT_GE_SYNC):
                    tick_work_intervals[(epoch, start_ev["tick_id"])].append((start_ev["timestamp_us"], ts))
                if etype == EVT_RENDER_PASS:
                    render_pass_intervals[(epoch, start_ev["tick_id"], start_ev["pass_id"])].append(
                        (start_ev["timestamp_us"], ts)
                    )
        else:
            if etype == EVT_PRESENT_QUEUED:
                if ev["buffer_gen"] != 0:
                    queued_presents[(ev["target_id"], ev["buffer_gen"])] = ts
            elif etype == EVT_FRONT_OBSERVED:
                addr = ev["target_id"]
                bgen = ev["buffer_gen"]
                if bgen != 0 and (addr, bgen) not in queued_presents:
                    stale_generations += 1
                    continue
                cur_key = (addr, bgen)
                if cur_key != last_observed_front:
                    distinct_fronts += 1
                    last_observed_front = cur_key
                else:
                    repeated_fronts += 1
                observed_fronts.append((ts, addr, bgen))
            elif etype == EVT_AUDIO_UNDERRUN:
                audio_underruns += 1
            elif etype == EVT_MEM_SAMPLE:
                free_b, largest_b = ev["val0"], ev["val1"]
                mem_free_min = free_b if mem_free_min is None else min(mem_free_min, free_b)
                mem_largest_min = largest_b if mem_largest_min is None else min(mem_largest_min, largest_b)
            elif etype == EVT_OBJECT_REQ:
                obj_reqs[(epoch, ev["target_id"])] = ts
            elif etype == EVT_OBJECT_READY:
                req_ts = obj_reqs.pop((epoch, ev["target_id"]), None)
                if req_ts is not None and ts >= req_ts:
                    obj_ready_latencies.append(ts - req_ts)
            elif etype == EVT_STATE_HASH:
                state_hashes.append((ev["tick_id"], ev["val0"]))
            elif etype == EVT_INPUT_EDGE:
                input_edges += 1

        if etype == EVT_AUDIO_PRODUCER and (flags & FLAG_END):
            audio_playable.append(ev["val0"])
        elif etype == EVT_AUDIO_OUTPUT and (flags & FLAG_START):
            audio_playable.append(ev["val0"])

    unclosed_starts += sum(len(v) for v in open_spans.values())

    # Critical-path per tick (union of overlapping main + video worker intervals)
    critical_path_us = [merge_union_duration(ivs) for ivs in tick_work_intervals.values()]
    pass_durations = paired_spans.get("render_pass", [])
    over_25ms = sum(1 for d in pass_durations if d > 25000)
    over_50ms = sum(1 for d in pass_durations if d > 50000)

    # Wall window & rates
    all_ts = [e["timestamp_us"] for e in events if e["event_type"] != EVT_TRACE_SUMMARY]
    duration_us = (max(all_ts) - min(all_ts)) if len(all_ts) >= 2 else 0
    duration_s = duration_us / 1_000_000.0 if duration_us > 0 else 0.0

    # Slowest 10s window of render passes (if timestamps span >0)
    worst_10s_over25 = 0
    if observed_fronts:
        front_ts = [x[0] for x in observed_fronts]
    else:
        front_ts = []

    pass_end_ts = sorted(
        ev["timestamp_us"] for ev in events
        if (ev["flags"] & FLAG_END) and ev["event_type"] == EVT_RENDER_PASS
    )
    if pass_end_ts and len(pass_durations) == len(pass_end_ts):
        j = 0
        for i, t_end in enumerate(pass_end_ts):
            while pass_end_ts[j] < t_end - 10_000_000:
                j += 1
            cnt = sum(1 for k in range(j, i + 1) if pass_durations[k] > 25000)
            if cnt > worst_10s_over25:
                worst_10s_over25 = cnt

    channels = {}
    for name, vals in sorted(paired_spans.items()):
        channels[name] = {
            "count": len(vals),
            "mean_us": sum(vals) / len(vals),
            "p50_us": percentile(vals, 50),
            "p95_us": percentile(vals, 95),
            "p99_us": percentile(vals, 99),
            "max_us": max(vals),
        }

    return {
        "duration_s": duration_s,
        "tick_count": len(tick_work_intervals),
        "render_pass_count": len(pass_durations),
        "ticks_per_s": len(tick_work_intervals) / duration_s if duration_s > 0 else 0.0,
        "rendered_passes_per_s": len(pass_durations) / duration_s if duration_s > 0 else 0.0,
        "distinct_fronts": distinct_fronts,
        "repeated_fronts": repeated_fronts,
        "distinct_fronts_per_s": distinct_fronts / duration_s if duration_s > 0 else 0.0,
        "over_25ms_passes": over_25ms,
        "over_50ms_passes": over_50ms,
        "worst_10s_over_25ms": worst_10s_over25,
        "critical_path_mean_us": (sum(critical_path_us) / len(critical_path_us)) if critical_path_us else 0.0,
        "critical_path_p95_us": percentile(critical_path_us, 95),
        "critical_path_p99_us": percentile(critical_path_us, 99),
        "critical_path_max_us": max(critical_path_us) if critical_path_us else 0,
        "audio_playable_min": min(audio_playable) if audio_playable else None,
        "audio_playable_p01": percentile(audio_playable, 1) if audio_playable else None,
        "audio_underruns": audio_underruns,
        "obj_ready_latencies_us": obj_ready_latencies,
        "mem_free_min": mem_free_min,
        "mem_largest_min": mem_largest_min,
        "input_edges": input_edges,
        "state_hash_count": len(state_hashes),
        "state_hashes": state_hashes,
        "unclosed_starts": unclosed_starts,
        "orphan_ends": orphan_ends,
        "stale_generations": stale_generations,
        "seq_gaps": seq_gaps,
        "ring_summaries": ring_summaries,
        "channels": channels,
    }


def write_metrics_csv(summary, csv_path: Path):
    csv_path.parent.mkdir(parents=True, exist_ok=True)
    with csv_path.open("w", newline="") as f:
        writer = csv.writer(f)
        writer.writerow(["channel", "count", "mean_us", "p50_us", "p95_us", "p99_us", "max_us"])
        for ch, stats in summary["channels"].items():
            writer.writerow([
                ch,
                stats["count"],
                f"{stats['mean_us']:.2f}",
                f"{stats['p50_us']:.2f}",
                f"{stats['p95_us']:.2f}",
                f"{stats['p99_us']:.2f}",
                stats["max_us"],
            ])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("trace_file", type=Path)
    parser.add_argument("--csv", type=Path, default=None)
    parser.add_argument("--json", type=Path, default=None)
    args = parser.parse_args()

    parsed = read_trace_file(args.trace_file)
    summary = reduce_events(parsed["events"])
    summary["trailing_bytes"] = parsed["trailing_bytes"]
    summary["scope_mask"] = parsed["scope_mask"]
    if args.csv:
        write_metrics_csv(summary, args.csv)
    if args.json:
        args.json.parent.mkdir(parents=True, exist_ok=True)
        args.json.write_text(json.dumps(summary, indent=2))
    print(json.dumps({k: v for k, v in summary.items() if k != "state_hashes"}, indent=2))


if __name__ == "__main__":
    main()
