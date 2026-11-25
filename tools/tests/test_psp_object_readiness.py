#!/usr/bin/env python3
"""Verify Object_UpdateEntries readiness without artificial extra-frame stalls."""
from pathlib import Path
import os
import re
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[2]
scene_src = (ROOT / "src/code/z_scene.c").read_text()


def function(source, name):
    pattern = r"^(?:static\s+)?[A-Za-z0-9_* \t]+\b" + re.escape(name) + r"\s*\([^;]*?\)\s*\{"
    match = re.search(pattern, source, re.M)
    assert match, f"Missing function {name}"
    start = match.start()
    return source[start:source.index("\n}", source.index("{", start)) + 2] + "\n"


code = r"""
#include <assert.h>
#include <stdint.h>
#include <stddef.h>
#include <stdio.h>
#include <stdbool.h>

typedef int32_t s32;
typedef int16_t s16;
typedef uint32_t u32;
typedef uintptr_t UNK_TYPE;
#define OS_MESG_NOBLOCK 0
#define OBJECT_SLOT_NONE (-1)
#define MmPsp_DiagLog(...) ((void)0)

typedef void* OSMesg;
typedef struct { void* msg; int hasMsg; } OSMesgQueue;
typedef struct { uintptr_t vromStart; uintptr_t vromEnd; } RomFile;
typedef struct { uintptr_t vromAddr; void* dramAddr; size_t size; OSMesgQueue* notifyQueue; void* notifyMsg; } DmaRequest;
typedef struct { s16 id; void* segment; DmaRequest dmaReq; OSMesgQueue loadQueue; OSMesg loadMsg; } ObjectEntry;
typedef struct { s32 numEntries; ObjectEntry slots[8]; } ObjectContext;

static RomFile gObjectTable[16];
static int gAsyncModeCompleteImmediately = 1;
static int gPrefetchCalls = 0;

static void osCreateMesgQueue(OSMesgQueue* q, OSMesg* m, int count) {
    (void)m; (void)count;
    q->hasMsg = 0;
}
static s32 osRecvMesg(OSMesgQueue* q, OSMesg* m, int flag) {
    (void)flag;
    if (!q->hasMsg) return -1;
    if (m) *m = q->msg;
    q->hasMsg = 0;
    return 0;
}
static s32 DmaMgr_RequestAsync(DmaRequest* req, void* ram, uintptr_t vrom, size_t size, UNK_TYPE unused,
                               OSMesgQueue* q, void* msg) {
    (void)unused;
    req->vromAddr = vrom;
    req->dramAddr = ram;
    req->size = size;
    req->notifyQueue = q;
    req->notifyMsg = msg;
    if (gAsyncModeCompleteImmediately && q != NULL) {
        q->msg = msg;
        q->hasMsg = 1;
    }
    return 0;
}
static u32 OotPsp_AssetGetStreamGeneration(void) { return 1; }
static s32 OotPsp_AssetPrefetch(uintptr_t v, size_t s, u32 g, u32 d) {
    (void)v; (void)s; (void)g; (void)d;
    gPrefetchCalls++;
    return 1;
}
"""

code += function(scene_src, "Object_UpdateEntries")
code += function(scene_src, "Object_IsLoaded")

# Strip MmPsp_DiagLog and trace calls cleanly when compiled for host
code = re.sub(r"#if PLATFORM_PSP\s*\n\s*MmPsp_DiagLog\([^\n]+\n#endif", "", code)
code = re.sub(r"#if defined\(MM_PSP_TRACE\) && MM_PSP_TRACE[\s\S]*?#endif", "", code)
code = re.sub(r"#if defined\(TARGET_PSP\) && defined\(MM_PSP_TRACE\) && MM_PSP_TRACE[\s\S]*?#endif", "", code)

code += r"""
int main(void) {
    ObjectContext ctx = {0};
    gObjectTable[1] = (RomFile){0x1000, 0x2000};
    gObjectTable[2] = (RomFile){0x2000, 0x2000}; // empty
    gObjectTable[3] = (RomFile){0x3000, 0x5000};

    ctx.numEntries = 3;
    ctx.slots[0].id = -1; ctx.slots[0].segment = (void*)0x80000000;
    ctx.slots[1].id = -2; ctx.slots[1].segment = (void*)0x80001000;
    ctx.slots[2].id = -3; ctx.slots[2].segment = (void*)0x80002000;

    // 1. Synchronous DMA completion marks objects ready on the first Object_UpdateEntries call.
    gAsyncModeCompleteImmediately = 1;
    Object_UpdateEntries(&ctx);
    assert(ctx.slots[0].id == 1 && Object_IsLoaded(&ctx, 0));
    assert(ctx.slots[1].id == 0 && !Object_IsLoaded(&ctx, 1));
    assert(ctx.slots[2].id == 3 && Object_IsLoaded(&ctx, 2));
    assert(gPrefetchCalls > 0);

    // 2. Truly asynchronous pending DMA stays negative until queue receives completion.
    ctx.slots[0].id = -1;
    ctx.slots[0].dmaReq.vromAddr = 0;
    gAsyncModeCompleteImmediately = 0;
    Object_UpdateEntries(&ctx);
    assert(ctx.slots[0].id == -1 && !Object_IsLoaded(&ctx, 0));
    ctx.slots[0].loadQueue.hasMsg = 1;
    Object_UpdateEntries(&ctx);
    assert(ctx.slots[0].id == 1 && Object_IsLoaded(&ctx, 0));

    puts("Object readiness: immediate completion after synchronous DMA, empty slots and async fallback passed");
    return 0;
}
"""

with tempfile.TemporaryDirectory(prefix="mm-obj-ready-") as d:
    p = Path(d) / "test.c"
    exe = Path(d) / "test"
    p.write_text(code)
    subprocess.run([os.environ.get("CC", "cc"), "-std=c11", "-O2", "-Wall", "-Wextra", "-Werror",
                    "-DPLATFORM_PSP=1", "-Wno-unused-parameter", str(p), "-o", str(exe)], check=True)
    subprocess.run([str(exe)], check=True)
