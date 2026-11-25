#!/usr/bin/env python3
"""Host PCM and reference tests for PSP audio resampling, ring wrap, underrun, and ME publication."""
from pathlib import Path
import os
import re
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[2]
source = (ROOT / "src/port/psp/backend/oot_psp_audio_backend.c").read_text()


def extract_function(src: str, name: str) -> str:
    pattern = re.compile(r"^(?:static )?(?:inline )?[^\n]*\b" + re.escape(name) + r"\([^;]*?\)\s*\{", re.M)
    m = pattern.search(src)
    if not m:
        raise RuntimeError(f"Could not locate {name}")
    start = m.start()
    brace_start = src.index("{", start)
    depth = 0
    for idx in range(brace_start, len(src)):
        ch = src[idx]
        if ch == "{":
            depth += 1
        elif ch == "}":
            depth -= 1
            if depth == 0:
                return src[start : idx + 1] + "\n\n"
    raise RuntimeError(f"Unterminated function {name}")


stubs = r'''
#include <assert.h>
#include <math.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

typedef uint8_t u8;
typedef uint16_t u16;
typedef uint32_t u32;
typedef uint64_t u64;
typedef int16_t s16;
typedef int32_t s32;

#define OOT_PSP_AUDIO_CHANNELS 2
#define OOT_PSP_AUDIO_OUTPUT_FREQUENCY 44100
#define OOT_PSP_AUDIO_OUTPUT_CHUNK_FRAMES 768
#define OOT_PSP_AUDIO_RING_FRAMES 16384
#define OOT_PSP_AUDIO_RING_MASK (OOT_PSP_AUDIO_RING_FRAMES - 1)
#define OOT_PSP_AUDIO_RESAMPLE_FRAC_BITS 24
#define OOT_PSP_AUDIO_RESAMPLE_FRAC_MASK ((1U << OOT_PSP_AUDIO_RESAMPLE_FRAC_BITS) - 1)
#define OOT_PSP_AUDIO_RESAMPLE_STEP_2X (1U << (OOT_PSP_AUDIO_RESAMPLE_FRAC_BITS - 1))
#define OOT_PSP_AUDIO_LERP_FRAC_BITS 15
#define OOT_PSP_AUDIO_RENDER_FLAG_UNDERRUN 1
#define OOT_PSP_AUDIO_TARGET_CHUNKS 6
#define OOT_PSP_AUDIO_STARTUP_CHUNKS 4
#define OOT_PSP_AUDIO_IO_BACKOFF_CHUNKS 5
#define OOT_PSP_AUDIO_URGENT_CHUNKS 3
#define OOT_PSP_AUDIO_RECOVERY_CHUNKS 2
#define OOT_PSP_AUDIO_PRODUCER_IO_BACKOFF_USEC 1000
#define OOT_PSP_AUDIO_DIAGNOSTICS 0
#define AUDIO_DIAG_SET(var, val) ((void)0)
#define AUDIO_DIAG_INCREMENT(var) ((void)0)

typedef enum {
    OOT_PSP_AUDIO_UPDATE_BLOCKED,
    OOT_PSP_AUDIO_UPDATE_COMPLETED,
    OOT_PSP_AUDIO_UPDATE_IO_BACKOFF,
} OotPspAudioUpdateResult;

static s16 sAudioRing[OOT_PSP_AUDIO_RING_FRAMES * OOT_PSP_AUDIO_CHANNELS];
static volatile u32 sAudioWritePos;
static volatile u32 sAudioReadPos;
static u32 sAudioSourceChunkFrames = 558;
static const s16* sAudioMePendingQueueSrc;
static u32 sAudioMePendingQueueFrames;
static u32 sAudioMePendingQueueWritePos;
static u32 sAudioMeQueueResultWritePos;
static volatile s32 sAudioProducerThreadRunning = 1;
static u32 sFakeNowUs;
static s32 sFakeCanRun = 1;
static s32 sFakeFgPressure = 0;
static u32 sFakePlayableFrames = 0;
static int sAudioUpdatesCalled = 0;
static int sWaitForCommandsCalled = 0;
static int sCpuCopyCalled = 0;
static int sInvalidateCalled = 0;
static int sDelayCalls = 0;

static u32 sceKernelGetSystemTimeLow(void) { return sFakeNowUs; }
static void sceKernelDelayThread(u32 us) { sFakeNowUs += us; sDelayCalls++; }
static s32 sceKernelGetThreadCurrentPriority(void) { return 0x20; }
static void sceKernelRotateThreadReadyQueue(s32 p) { (void)p; }

static s32 OotPspAudioBackend_CanRunUpdate(void) { return sFakeCanRun; }
static u32 OotPspAudioBackend_PlayableFrames(void) { return sFakePlayableFrames; }
static u32 OotPspAudioBackend_IoBackoffFrames(void) { return sAudioSourceChunkFrames * OOT_PSP_AUDIO_IO_BACKOFF_CHUNKS; }
static s32 OotPsp_AssetReadHasForegroundPressure(void) { return sFakeFgPressure; }
static void AudioThread_Update(void) { sAudioUpdatesCalled++; }
static void OotPspAudioBackend_WaitForCommands(void) { sWaitForCommandsCalled++; }
static void OotPspAudioBackend_InvalidateQueuedRingFrames(u32 writePos, u32 frames) {
    (void)writePos; (void)frames; sInvalidateCalled++;
}
static void OotPspAudioBackend_QueueCpuCopy(const s16* samples, u32 frames, u32 writePos) {
    sCpuCopyCalled++;
    for (u32 i = 0; i < frames; i++) {
        u32 idx = ((writePos + i) & OOT_PSP_AUDIO_RING_MASK) * OOT_PSP_AUDIO_CHANNELS;
        sAudioRing[idx] = samples[i * 2];
        sAudioRing[idx + 1] = samples[i * 2 + 1];
    }
    sAudioWritePos = (writePos + frames) & OOT_PSP_AUDIO_RING_MASK;
}
'''

funcs = ""
for fn in (
    "OotPspAudioBackend_CalculateSourceChunkFrames",
    "OotPspAudioBackend_CalculateResampleStep",
    "OotPspAudioBackend_LerpDelta",
    "OotPspAudioBackend_LerpSample",
    "OotPspAudioBackend_LerpHalfSample",
    "OotPspAudioBackend_FadeChunkToSilenceState",
    "OotPspAudioBackend_RenderOutputChunkState",
    "OotPspAudioBackend_PublishPendingMeQueue",
    "OotPspAudioBackend_TryRunUpdate",
    "OotPspAudioBackend_RunScheduledUpdate",
):
    funcs += extract_function(source, fn)

tests = r'''
static void fill_ring_ramp(u32 startPos, u32 count, s32 base) {
    for (u32 i = 0; i < count; i++) {
        u32 idx = ((startPos + i) & OOT_PSP_AUDIO_RING_MASK) * 2;
        sAudioRing[idx] = (s16)((base + (s32)i) % 32000);
        sAudioRing[idx + 1] = (s16)(-((base + (s32)i) % 32000));
    }
}

int main(void) {
    // 1. Source chunk & resample step at 32000 Hz -> 44100 Hz
    u32 chunk32 = OotPspAudioBackend_CalculateSourceChunkFrames(32000);
    assert(chunk32 == 558);
    u32 step32 = OotPspAudioBackend_CalculateResampleStep(32000);
    double ratio = (double)step32 / (double)(1U << OOT_PSP_AUDIO_RESAMPLE_FRAC_BITS);
    assert(fabs(ratio - (32000.0 / 44100.0)) < 1e-6);

    // 2. Signed extrema interpolation (must not overflow s16 or s32, and stay within 1 LSB of float ref)
    assert(OotPspAudioBackend_LerpHalfSample(-32768, 32767) == 0);
    assert(OotPspAudioBackend_LerpHalfSample(32767, -32768) == -1);
    assert(OotPspAudioBackend_LerpDelta(-32768, 65535, 32767) == 32765);
    assert(OotPspAudioBackend_LerpDelta(32767, -65535, 32767) == -32767);

    // 3. Few frames & underrun fade-to-silence (0, 1, 2, 10 frames)
    s16 mix[OOT_PSP_AUDIO_OUTPUT_CHUNK_FRAMES * 2];
    u32 readPos = 0, frac = 0, flags = 0;
    s16 lastL = 0, lastR = 0;
    assert(OotPspAudioBackend_RenderOutputChunkState(0, &readPos, &frac, step32, &lastL, &lastR, &flags, mix) == 0);

    for (u32 n = 1; n <= 10; n++) {
        readPos = OOT_PSP_AUDIO_RING_FRAMES - 3; // forces ring wrap!
        frac = 0; flags = 0; lastL = 0; lastR = 0;
        fill_ring_ramp(readPos, n, 1000);
        u32 consumed = OotPspAudioBackend_RenderOutputChunkState(n, &readPos, &frac, step32, &lastL, &lastR, &flags, mix);
        assert(consumed == n);
        assert(flags & OOT_PSP_AUDIO_RENDER_FLAG_UNDERRUN);
        assert(lastL == 0 && lastR == 0 && frac == 0);
        assert(readPos == ((OOT_PSP_AUDIO_RING_FRAMES - 3 + n) & OOT_PSP_AUDIO_RING_MASK));
        // Final samples must fade smoothly to near zero
        assert(abs(mix[(OOT_PSP_AUDIO_OUTPUT_CHUNK_FRAMES - 1) * 2]) < 500);
    }

    // 4. Ring wrap + prolonged non-integer resampling (2000 chunks at 32000->44100 Hz)
    // Generate a continuous 440 Hz sine wave in the ring and verify zero discontinuities
    // across 2000 chunk boundaries (~34.8s).
    const double twoPi = 6.28318530717958647692;
    readPos = 16000;
    u32 writeCursor = 16000;
    u64 srcSampleIdx = 0;
    frac = 0; lastL = 0; lastR = 0; flags = 0;
    u32 buffered = 0;
    s16 prevOutL = 0;
    int first = 1;
    s32 maxBoundaryJump = 0;
    for (int chunk = 0; chunk < 2000; chunk++) {
        while (buffered < 2048) {
            u32 idx = (writeCursor & OOT_PSP_AUDIO_RING_MASK) * 2;
            s16 v = (s16)(12000.0 * sin(twoPi * 440.0 * (double)srcSampleIdx / 32000.0));
            sAudioRing[idx] = v;
            sAudioRing[idx + 1] = -v;
            writeCursor = (writeCursor + 1) & OOT_PSP_AUDIO_RING_MASK;
            srcSampleIdx++;
            buffered++;
        }
        u32 consumed = OotPspAudioBackend_RenderOutputChunkState(buffered, &readPos, &frac, step32, &lastL, &lastR, &flags, mix);
        assert(!(flags & OOT_PSP_AUDIO_RENDER_FLAG_UNDERRUN));
        assert(consumed == 557 || consumed == 558);
        buffered -= consumed;
        if (!first) {
            s32 jump = abs((s32)mix[0] - (s32)prevOutL);
            if (jump > maxBoundaryJump) maxBoundaryJump = jump;
        }
        first = 0;
        prevOutL = mix[(OOT_PSP_AUDIO_OUTPUT_CHUNK_FRAMES - 1) * 2];
    }
    // 440 Hz at 44100 Hz has max sample-to-sample delta ~ 12000 * 2*pi*440/44100 ~= 752
    assert(maxBoundaryJump < 900);

    // 5. Exact 2x wrap path (22050 -> 44100 Hz)
    readPos = OOT_PSP_AUDIO_RING_FRAMES - 10;
    fill_ring_ramp(readPos, 500, 100);
    frac = 0; flags = 0;
    u32 consumed2x = OotPspAudioBackend_RenderOutputChunkState(500, &readPos, &frac, OOT_PSP_AUDIO_RESAMPLE_STEP_2X,
                                                               &lastL, &lastR, &flags, mix);
    assert(consumed2x == 384 && !(flags & OOT_PSP_AUDIO_RENDER_FLAG_UNDERRUN));
    assert(readPos == ((OOT_PSP_AUDIO_RING_FRAMES - 10 + 384) & OOT_PSP_AUDIO_RING_MASK));

    // 6. Late / incomplete ME queue publication fallback & healthy invalidation
    s16 srcSamples[128 * 2];
    for (int i = 0; i < 128; i++) { srcSamples[i * 2] = (s16)(i + 1); srcSamples[i * 2 + 1] = (s16)(-i - 1); }
    sAudioMePendingQueueSrc = srcSamples;
    sAudioMePendingQueueFrames = 128;
    sAudioMePendingQueueWritePos = OOT_PSP_AUDIO_RING_FRAMES - 64; // wraps across ring end
    // Case A: ME did NOT complete writePos -> must fall back to CPU copy
    sAudioMeQueueResultWritePos = sAudioMePendingQueueWritePos;
    sCpuCopyCalled = sInvalidateCalled = 0;
    OotPspAudioBackend_PublishPendingMeQueue();
    assert(sCpuCopyCalled == 1 && sInvalidateCalled == 0);
    assert(sAudioWritePos == 64);
    assert(sAudioRing[(OOT_PSP_AUDIO_RING_FRAMES - 64) * 2] == 1);
    assert(sAudioRing[63 * 2] == 128);
    // Case B: ME completed -> must invalidate ring and publish writePos
    sAudioMeQueueResultWritePos = 64;
    sAudioWritePos = 0;
    sCpuCopyCalled = sInvalidateCalled = 0;
    OotPspAudioBackend_PublishPendingMeQueue();
    assert(sCpuCopyCalled == 0 && sInvalidateCalled == 1);
    assert(sAudioWritePos == 64);

    // 7. Retry during IO backoff & clears backoff when foreground pressure drops
    sFakeCanRun = 1;
    sFakeFgPressure = 1;
    sFakePlayableFrames = OotPspAudioBackend_IoBackoffFrames();
    sFakeNowUs = 0;
    sAudioUpdatesCalled = sDelayCalls = 0;
    assert(OotPspAudioBackend_RunScheduledUpdate(3500) == OOT_PSP_AUDIO_UPDATE_IO_BACKOFF);
    assert(sDelayCalls == 3 && sAudioUpdatesCalled == 0);
    sFakeFgPressure = 0;
    assert(OotPspAudioBackend_RunScheduledUpdate(10000) == OOT_PSP_AUDIO_UPDATE_COMPLETED);
    assert(sAudioUpdatesCalled == 1 && sWaitForCommandsCalled == 1);

    printf("Audio output: wrap, few frames, signed extrema, 2000-chunk non-integer 32kHz->44.1kHz, underrun, late ME publish and retry passed (maxBoundaryJump=%d)\n",
           (int)maxBoundaryJump);
    return 0;
}
'''

with tempfile.TemporaryDirectory(prefix="mm-audio-out-") as d:
    p = Path(d)
    (p / "test.c").write_text(stubs + funcs + tests)
    subprocess.run(
        [os.environ.get("CC", "cc"), "-std=gnu11", "-O2", "-Wall", "-Wextra", "-Werror", str(p / "test.c"), "-lm", "-o", str(p / "test")],
        check=True,
    )
    subprocess.run([str(p / "test")], check=True)
