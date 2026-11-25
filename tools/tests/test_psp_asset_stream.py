#!/usr/bin/env python3
"""Test deadline-aware packed asset prefetch, cancellation, cache wrap and audio yield."""
from pathlib import Path
import os
import re
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[2]
PORT = ROOT / "src/port/psp/backend"
loader = (PORT / "oot_psp_asset_loader.c").read_text()


def function(source, name):
    pattern = r"^(?:static\s+)?[A-Za-z0-9_* \t]+\b" + re.escape(name) + r"\s*\([^;]*?\)\s*\{"
    match = re.search(pattern, source, re.M)
    assert match, f"Missing function {name}"
    start = match.start()
    return source[start:source.index("\n}", source.index("{", start)) + 2] + "\n"


lines = loader.splitlines()
defines = "\n".join(lines[19:55]) + "\n"
defines += "\n".join(re.findall(r"^#define OOT_PSP_PREFETCH_[^\n]+", loader, re.M)) + "\n"

code = r"""
#include <assert.h>
#include <stdint.h>
#include <stddef.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdbool.h>

typedef uint8_t u8;
typedef uint16_t u16;
typedef uint32_t u32;
typedef uint64_t u64;
typedef int32_t s32;
typedef int SceUID;
typedef size_t SceSize;
typedef long long SceOff;
typedef struct { uintptr_t vromStart; uintptr_t vromEnd; } RomFile;

#define PSP_O_RDONLY 1
#define PSP_SEEK_SET 0
#define PSP_SEEK_END 2
#define THREAD_ATTR_USER 0
""" + defines + r"""

typedef struct OotPspExternalAsset {
    uintptr_t vromStart;
    uintptr_t vromEnd;
    uintptr_t originalVromStart;
    uintptr_t originalVromEnd;
    u32 flags;
    uintptr_t fileOffset;
    const char* name;
} OotPspExternalAsset;

typedef struct OotPspPackedAssetCacheBlock {
    size_t blockIndex;
    size_t dataSize;
    u32 lastUsed;
    s32 valid;
    s32 loading;
} OotPspPackedAssetCacheBlock;

typedef struct OotPspPrefetchEntry {
    size_t fileOffset;
    size_t size;
    u32 generation;
    u32 deadlineUs;
    u32 order;
    s32 active;
} OotPspPrefetchEntry;

static OotPspExternalAsset gOotPspExternalAssets[8];
static size_t gOotPspExternalAssetCount = 0;

static u8* sOotPspPackedCacheData = NULL;
static size_t sOotPspPackedCacheBlockCount = 0;
static s32 sOotPspPackedCacheInitTried = false;
static OotPspPackedAssetCacheBlock sOotPspPackedCacheBlocks[OOT_PSP_PACKED_CACHE_MAX_BLOCK_COUNT];
static OotPspPackedAssetCacheBlock* sOotPspPackedCacheLookup[OOT_PSP_PACKED_CACHE_LOOKUP_COUNT];
static u32 sOotPspAssetCacheClock = 0;
static SceUID sOotPspAssetSema = 1;
static SceUID sOotPspPackedAssetFd = -1;
static s32 sOotPspPackedAssetUnavailable = false;
static size_t sOotPspPackedAssetSize = 0;
static s32 sOotPspPackedAssetSizeKnown = false;
static size_t sOotPspPackedAssetPosition = 0;
static volatile s32 sOotPspPackedAssetPositionKnown = false;
static s32 sOotPspAssetsSuspended = false;
static volatile s32 sOotPspForegroundAssetReadWaiters = 0;
static volatile s32 sOotPspForegroundAssetReadActive = 0;
static OotPspPrefetchEntry sOotPspPrefetchQueue[OOT_PSP_PREFETCH_QUEUE_CAPACITY];
static u8 sOotPspPrefetchStaging[OOT_PSP_PACKED_CACHE_BLOCK_SIZE];
static volatile u32 sOotPspPrefetchGeneration = 1;
static u32 sOotPspPrefetchNextOrder = 1;
static SceUID sOotPspPrefetchQueueSema = 2;
static SceUID sOotPspPrefetchWorkSema = 3;
static SceUID sOotPspPrefetchThreadId = 4;
static s32 sOotPspPrefetchThreadInitTried = true;

#define PACKED_FILE_SIZE (6 * 65536 + 12345)
static u8 gPackedFile[PACKED_FILE_SIZE];
static size_t gFdPos = 0;
static int gIoMaxChunk = 1021;
static int gZeroReadsBeforeData = 0;
static int gFailReads = 0;
static int gAudioUrgent = 0;
static int gAudioUrgentAfterReads = -1;
static int gBumpGenAfterReads = -1;
static int gSuspendAfterReads = -1;
static int gReadCalls = 0;
static int gDelayCalls = 0;
static int gSemaCount[4] = {0, 1, 1, 0};

static void OotPsp_InitAssetSema(void) {}
static void OotPsp_LockAssetLoader(void) { assert(gSemaCount[1] == 1); gSemaCount[1] = 0; }
static void OotPsp_UnlockAssetLoader(void) { assert(gSemaCount[1] == 0); gSemaCount[1] = 1; }
static int sceKernelWaitSema(SceUID id, int count, void* t) { (void)count; (void)t; assert(gSemaCount[id] > 0); gSemaCount[id]--; return 0; }
static int sceKernelSignalSema(SceUID id, int count) { (void)count; if (id == 3) { gSemaCount[3]++; return 0; } assert(gSemaCount[id] == 0); gSemaCount[id]++; return 0; }
static SceUID sceKernelCreateSema(const char* n, int a, int i, int m, void* o) { (void)n;(void)a;(void)i;(void)m;(void)o; return 9; }
static SceUID sceKernelCreateThread(const char* n, int (*e)(SceSize,void*), int p, int s, int a, void* o) { (void)n;(void)e;(void)p;(void)s;(void)a;(void)o; return 9; }
static int sceKernelStartThread(SceUID id, SceSize s, void* p) { (void)id;(void)s;(void)p; return 0; }
static void sceKernelDelayThread(u32 us) { (void)us; gDelayCalls++; gAudioUrgent = 0; }
static s32 OotPspAudioBackend_NeedsRefillDuringIo(void) { return gAudioUrgent; }
static s32 OotPspAudioBackend_NeedsRefillUrgently(void) { return gAudioUrgent; }
static const char* OotPsp_ResolveRootPath(const char* p, char* b, size_t s) { (void)b;(void)s; return p; }
static void OotPsp_InitPackedAssetCache(void) {}
static void OotPsp_CooperateWithAudioRead(s32 a, volatile s32* k) { (void)a; if (gAudioUrgent && k) *k = false; }
static void OotPsp_CopyAssetBytes(void* d, const void* s, size_t n, s32 v) { (void)v; memcpy(d, s, n); }
static s32 sceIoOpen(const char* p, int m, int perm) { (void)p;(void)m;(void)perm; gFdPos = 0; return 77; }
static int sceIoClose(SceUID fd) { assert(fd == 77); return 0; }
static SceOff sceIoLseek32(SceUID fd, int off, int whence) {
    assert(fd == 77);
    if (whence == PSP_SEEK_END) gFdPos = PACKED_FILE_SIZE;
    else if (whence == PSP_SEEK_SET) gFdPos = (size_t)off;
    return (SceOff)gFdPos;
}
static int sceIoRead(SceUID fd, void* out, int size) {
    assert(fd == 77 && gSemaCount[1] == 0);
    gReadCalls++;
    if (gFailReads) return -1;
    if (gZeroReadsBeforeData > 0) { gZeroReadsBeforeData--; return 0; }
    if (gAudioUrgentAfterReads == 0) gAudioUrgent = 1;
    if (gAudioUrgentAfterReads > 0) gAudioUrgentAfterReads--;
    if (gBumpGenAfterReads == 0) sOotPspPrefetchGeneration++;
    if (gBumpGenAfterReads > 0) gBumpGenAfterReads--;
    if (gSuspendAfterReads == 0) sOotPspAssetsSuspended = true;
    if (gSuspendAfterReads > 0) gSuspendAfterReads--;
    size_t avail = (gFdPos < PACKED_FILE_SIZE) ? (PACKED_FILE_SIZE - gFdPos) : 0;
    size_t n = (size_t)size;
    if (n > (size_t)gIoMaxChunk) n = (size_t)gIoMaxChunk;
    if (n > avail) n = avail;
    if (n > 0) { memcpy(out, gPackedFile + gFdPos, n); gFdPos += n; }
    return (int)n;
}

static void OotPsp_ClearPackedAssetCache(void) {
    memset(sOotPspPackedCacheBlocks, 0, sizeof(sOotPspPackedCacheBlocks));
    memset(sOotPspPackedCacheLookup, 0, sizeof(sOotPspPackedCacheLookup));
}

static const OotPspExternalAsset* OotPsp_FindContainingOriginalExternalAssetRange(uintptr_t s, uintptr_t e) {
    (void)s; (void)e; return NULL;
}
"""

for name in (
    "OotPsp_NextAssetCacheClock",
    "OotPsp_OpenPackedAssetFile",
    "OotPsp_ReadAssetChunk",
    "OotPsp_PackedCacheBlockData",
    "OotPsp_FindPackedCacheBlock",
    "OotPsp_SelectPackedCacheBlock",
    "OotPsp_ReadPackedOpenFileRange",
    "OotPsp_LoadPackedCacheBlock",
    "OotPsp_ReadPackedAssetFileRangeCached",
    "OotPsp_ReadPackedAssetFileRange",
    "OotPsp_FindContainingExternalAsset",
    "OotPsp_IsExternalAssetSpanContiguous",
    "OotPsp_RangeContains",
    "OotPsp_TryTranslateAssetRange",
    "OotPsp_TryNormalizeExternalRange",
    "OotPsp_NormalizeVromRange",
    "OotPsp_AssetReadHasForegroundPressure",
    "OotPsp_LockPrefetchQueue",
    "OotPsp_UnlockPrefetchQueue",
    "OotPsp_PrefetchShouldYield",
    "OotPsp_PopNextPrefetchEntry",
    "OotPsp_CommitPrefetchedBlock",
    "OotPsp_PrefetchOneBlock",
    "OotPsp_ProcessPrefetchEntry",
    "OotPsp_PrefetchThreadMain",
    "OotPsp_EnsurePrefetchWorker",
    "OotPsp_AssetBeginStreamGeneration",
    "OotPsp_AssetGetStreamGeneration",
    "OotPsp_AssetPrefetch",
):
    code += function(loader, name)

code = code.replace("OotPsp_AreOriginalAssetRangesSorted()", "0")

code += r"""
int main(void) {
    static u8 cachePool[4 * OOT_PSP_PACKED_CACHE_BLOCK_SIZE];
    u8 outBuf[2 * OOT_PSP_PACKED_CACHE_BLOCK_SIZE];
    OotPspPrefetchEntry popped;

    for (size_t i = 0; i < PACKED_FILE_SIZE; i++) {
        gPackedFile[i] = (u8)((i * 131U + 17U) ^ (i >> 8));
    }

    sOotPspPackedCacheData = cachePool;
    sOotPspPackedCacheBlockCount = 4;
    OotPsp_ClearPackedAssetCache();

    gOotPspExternalAssets[0] = (OotPspExternalAsset){0x01000000, 0x01020000, 0, 0, 0, 0x00000, "objA"};
    gOotPspExternalAssets[1] = (OotPspExternalAsset){0x02000000, 0x02020000, 0, 0, 0, 0x20000, "objB"};
    gOotPspExternalAssets[2] = (OotPspExternalAsset){0x03000000, 0x03023039, 0, 0, 0, 0x40000, "objC"};
    gOotPspExternalAssetCount = 3;

    // 1. Deadline ordering + generation bump cancellation
    u32 gen = OotPsp_AssetBeginStreamGeneration();
    assert(OotPsp_AssetPrefetch(0x02000000, 0x10000, gen, 20000));
    assert(OotPsp_AssetPrefetch(0x01000000, 0x10000, gen, 2000));
    assert(OotPsp_PopNextPrefetchEntry(gen, &popped) && popped.fileOffset == 0x00000 && popped.deadlineUs == 2000);
    assert(OotPsp_PopNextPrefetchEntry(gen, &popped) && popped.fileOffset == 0x20000);
    assert(!OotPsp_PopNextPrefetchEntry(gen, &popped));

    // Stale generation dropped on bump and ignored on submit
    assert(OotPsp_AssetPrefetch(0x02000000, 0x10000, gen, 5000));
    assert(!OotPsp_AssetPrefetch(0x01000000, 0x10000, gen - 1, 100));
    u32 gen2 = OotPsp_AssetBeginStreamGeneration();
    assert(!OotPsp_PopNextPrefetchEntry(gen2, &popped));

    // 2. Short reads, transient zero reads, and urgent audio preemption during prefetch
    sOotPspAssetCacheClock = 0xFFFFFFFCU;
    gZeroReadsBeforeData = 2;
    gAudioUrgentAfterReads = 5;
    assert(OotPsp_AssetPrefetch(0x01000000, 0x20000, gen2, 1000));
    assert(OotPsp_PopNextPrefetchEntry(gen2, &popped));
    assert(OotPsp_ProcessPrefetchEntry(&popped));
    assert(gDelayCalls > 0);
    assert(gSemaCount[1] == 1);
    for (size_t b = 0; b < 4; b++) {
        assert(!sOotPspPackedCacheBlocks[b].loading);
    }

    // Subsequent foreground read gets 100% cache hit (0 new sceIoRead calls!) with clock wrap intact
    int readsBefore = gReadCalls;
    OotPsp_LockAssetLoader();
    assert(OotPsp_ReadPackedAssetFileRange(&gOotPspExternalAssets[0], 0, outBuf, 0x20000, false, false, false));
    OotPsp_UnlockAssetLoader();
    assert(gReadCalls == readsBefore);
    assert(memcmp(outBuf, gPackedFile, 0x20000) == 0);
    assert(sOotPspAssetCacheClock >= 1);

    // 3. Eviction across more blocks than cache capacity (4 blocks) + truncated EOF tail block
    assert(OotPsp_AssetPrefetch(0x02000000, 0x20000, gen2, 1000));
    assert(OotPsp_AssetPrefetch(0x03000000, 0x23039, gen2, 2000));
    while (OotPsp_PopNextPrefetchEntry(gen2, &popped)) {
        assert(OotPsp_ProcessPrefetchEntry(&popped));
    }
    readsBefore = gReadCalls;
    OotPsp_LockAssetLoader();
    assert(OotPsp_ReadPackedAssetFileRange(&gOotPspExternalAssets[2], 0, outBuf, 0x20000, false, false, false));
    assert(OotPsp_ReadPackedAssetFileRange(&gOotPspExternalAssets[2], 0x20000, outBuf, 0x3039, true, false, false));
    OotPsp_UnlockAssetLoader();
    assert(gReadCalls == readsBefore);
    assert(memcmp(outBuf, gPackedFile + 0x40000 + 0x20000, 0x3039) == 0);

    // 4. Mid-flight generation cancellation & suspend abort leave zero loading slots or held locks
    OotPsp_ClearPackedAssetCache();
    gBumpGenAfterReads = 2;
    assert(OotPsp_AssetPrefetch(0x01000000, 0x20000, gen2, 100));
    assert(OotPsp_PopNextPrefetchEntry(gen2, &popped));
    assert(!OotPsp_ProcessPrefetchEntry(&popped));
    assert(gSemaCount[1] == 1);
    for (size_t b = 0; b < 4; b++) {
        assert(!sOotPspPackedCacheBlocks[b].loading);
    }

    u32 gen3 = OotPsp_AssetGetStreamGeneration();
    gSuspendAfterReads = 2;
    assert(OotPsp_AssetPrefetch(0x02000000, 0x20000, gen3, 100));
    assert(OotPsp_PopNextPrefetchEntry(gen3, &popped));
    assert(!OotPsp_ProcessPrefetchEntry(&popped));
    assert(gSemaCount[1] == 1);
    sOotPspAssetsSuspended = false;

    puts("Asset streaming: deadline ordering, short/zero reads, urgent audio yield, cache wrap/eviction, generation cancel and suspend abort passed");
    return 0;
}
"""

with tempfile.TemporaryDirectory(prefix="mm-asset-stream-") as d:
    p = Path(d) / "test.c"
    exe = Path(d) / "test"
    p.write_text(code)
    subprocess.run([os.environ.get("CC", "cc"), "-std=c11", "-O2", "-Wall", "-Wextra", "-Werror",
                    "-Wno-unused-parameter", "-Wno-unused-function", "-Wno-unused-variable",
                    str(p), "-o", str(exe)], check=True)
    subprocess.run([str(exe)], check=True)
