#include "mm_psp_trace.h"

#if MM_PSP_TRACE

#include <pspiofilemgr.h>
#include <pspkernel.h>
#include <pspthreadman.h>
#include <stdio.h>
#include <string.h>

#include "oot_psp_asset_loader.h"

#define MM_TRACE_RING_CAPACITY 1024U
#define MM_TRACE_RING_MASK (MM_TRACE_RING_CAPACITY - 1U)
#define MM_TRACE_OUTLIER_CAPACITY 128U
#define MM_TRACE_FLUSH_THRESHOLD 512U
#define MM_TRACE_FLUSH_BATCH 128U
#define MM_TRACE_FILE_NAME "mm-psp-trace.bin"

typedef struct MmPspTraceFileHeader {
    char magic[8];
    uint32_t version;
    uint32_t recordSize;
    uint32_t scopeMask;
    uint32_t reserved0;
    uint64_t startTimeUs;
    uint8_t reserved1[24];
} MmPspTraceFileHeader;

typedef struct MmPspTraceRing {
    MmPspTraceEvent events[MM_TRACE_RING_CAPACITY];
    volatile uint32_t writeIdx;
    volatile uint32_t readIdx;
    uint32_t seq;
    uint32_t totalEvents;
    uint32_t overflowCount;
} MmPspTraceRing;

static MmPspTraceRing sRings[MM_TRACE_THREAD_COUNT];
static MmPspTraceEvent sOutliers[MM_TRACE_OUTLIER_CAPACITY];
static volatile uint32_t sOutlierWriteIdx;
static volatile uint32_t sOutlierReadIdx;
static uint32_t sOutlierOverflowCount;
static volatile SceUID sThreadIds[MM_TRACE_THREAD_COUNT] = { -1, -1, -1, -1, -1 };
static volatile uint32_t sWakeEpoch = 1;
static volatile uint32_t sTickId = 0;
static volatile uint16_t sSceneEpoch = 0;
static volatile uint16_t sPassId = 0;
static volatile uint32_t sScopeMask = MM_PSP_TRACE_SCOPES;
static SceUID sTraceFd = -1;
static int sTraceHeaderWritten = 0;
static int sTraceFileFailed = 0;
static uint64_t sStartTimeUs = 0;
static char sTracePath[384];

static int MmPspTrace_WriteAll(SceUID fd, const void* data, size_t size) {
    const uint8_t* ptr = (const uint8_t*)data;
    size_t remaining = size;

    while (remaining != 0) {
        SceSSize written = sceIoWrite(fd, ptr, (SceSize)remaining);
        if (written <= 0) {
            return 0;
        }
        ptr += written;
        remaining -= (size_t)written;
    }
    return 1;
}

static inline void MmPspTrace_CompilerBarrier(void) {
    __asm__ volatile("" ::: "memory");
}

uint64_t MmPspTrace_NowUs(void) {
    return (uint64_t)sceKernelGetSystemTimeWide();
}

void MmPspTrace_Init(void) {
    uint32_t i;

    if (sStartTimeUs != 0) {
        return;
    }
    sStartTimeUs = MmPspTrace_NowUs();
    for (i = 0; i < MM_TRACE_THREAD_COUNT; i++) {
        sThreadIds[i] = -1;
        sRings[i].writeIdx = 0;
        sRings[i].readIdx = 0;
        sRings[i].seq = 0;
        sRings[i].totalEvents = 0;
        sRings[i].overflowCount = 0;
    }
    sThreadIds[MM_TRACE_THREAD_MAIN] = sceKernelGetThreadId();
}

void MmPspTrace_RegisterThread(MmPspTraceThreadSlot slot) {
    if ((uint32_t)slot < MM_TRACE_THREAD_COUNT) {
        sThreadIds[slot] = sceKernelGetThreadId();
    }
}

MmPspTraceThreadSlot MmPspTrace_CurrentSlot(void) {
    SceUID tid = sceKernelGetThreadId();
    uint32_t i;

    for (i = 0; i < MM_TRACE_THREAD_OTHER; i++) {
        if (sThreadIds[i] == tid) {
            return (MmPspTraceThreadSlot)i;
        }
    }
    return MM_TRACE_THREAD_OTHER;
}

void MmPspTrace_SetScopeMask(uint32_t mask) {
    sScopeMask = mask;
}

uint32_t MmPspTrace_GetScopeMask(void) {
    return sScopeMask;
}

void MmPspTrace_OnWake(void) {
    sWakeEpoch++;
    MmPspTrace_Record(MM_TRACE_THREAD_MAIN, MM_TRACE_EVT_WAKE_EPOCH, MM_TRACE_FLAG_INSTANT,
                      sWakeEpoch, 0, MmPspTrace_NowUs(), 0);
}

void MmPspTrace_SetTickContext(uint32_t tickId, uint16_t sceneEpoch, uint16_t passId) {
    sTickId = tickId;
    sSceneEpoch = sceneEpoch;
    sPassId = passId;
}

void MmPspTrace_SetPassId(uint16_t passId) {
    sPassId = passId;
}

static void MmPspTrace_WriteEventToRing(MmPspTraceThreadSlot slot, uint16_t eventType, uint16_t flags,
                                        uint32_t targetId, uint16_t bufferGen, uint64_t tsUs,
                                        uint64_t val0, uint64_t val1) {
    MmPspTraceRing* ring;
    uint32_t writeIdx;
    uint32_t readIdx;
    MmPspTraceEvent* ev;

    if ((uint32_t)slot >= MM_TRACE_THREAD_COUNT) {
        slot = MM_TRACE_THREAD_OTHER;
    }
    ring = &sRings[slot];
    ring->totalEvents++;
    writeIdx = ring->writeIdx;
    readIdx = ring->readIdx;
    if ((writeIdx - readIdx) >= MM_TRACE_RING_CAPACITY) {
        ring->overflowCount++;
        return;
    }
    ev = &ring->events[writeIdx & MM_TRACE_RING_MASK];
    ev->magicVer = MM_TRACE_MAGIC_VER;
    ev->wakeEpoch = sWakeEpoch;
    ev->timestampUs = tsUs;
    ev->seq = ++ring->seq;
    ev->threadId = (uint16_t)slot;
    ev->eventType = eventType;
    ev->flags = flags;
    ev->passId = sPassId;
    ev->tickId = sTickId;
    ev->bufferGen = bufferGen;
    ev->sceneEpoch = sSceneEpoch;
    ev->targetId = targetId;
    ev->val0 = val0;
    ev->val1 = val1;
    MmPspTrace_CompilerBarrier();
    ring->writeIdx = writeIdx + 1U;

    if ((flags & MM_TRACE_FLAG_OUTLIER) && (slot == MM_TRACE_THREAD_MAIN)) {
        uint32_t ow = sOutlierWriteIdx;
        uint32_t orIdx = sOutlierReadIdx;
        if ((ow - orIdx) < MM_TRACE_OUTLIER_CAPACITY) {
            MmPspTraceEvent copy = *ev;
            copy.seq = 0;
            copy.flags |= MM_TRACE_FLAG_OUTLIER_COPY;
            sOutliers[ow % MM_TRACE_OUTLIER_CAPACITY] = copy;
            MmPspTrace_CompilerBarrier();
            sOutlierWriteIdx = ow + 1U;
        } else {
            sOutlierOverflowCount++;
        }
    }
}

void MmPspTrace_Record(MmPspTraceThreadSlot slot, uint16_t eventType, uint16_t flags,
                       uint32_t targetId, uint16_t bufferGen, uint64_t val0, uint64_t val1) {
    MmPspTrace_WriteEventToRing(slot, eventType, flags, targetId, bufferGen, MmPspTrace_NowUs(), val0, val1);
}

uint64_t MmPspTrace_BeginSpan(MmPspTraceThreadSlot slot, uint16_t eventType, uint16_t flags,
                              uint32_t targetId, uint16_t bufferGen, uint64_t val0, uint64_t val1) {
    uint64_t now = MmPspTrace_NowUs();

    MmPspTrace_WriteEventToRing(slot, eventType, (uint16_t)(flags | MM_TRACE_FLAG_START),
                                targetId, bufferGen, now, val0, val1);
    return now;
}

void MmPspTrace_EndSpan(MmPspTraceThreadSlot slot, uint16_t eventType, uint16_t flags,
                        uint32_t targetId, uint16_t bufferGen, uint64_t startUs, uint64_t val0,
                        uint64_t val1) {
    uint64_t now = MmPspTrace_NowUs();
    uint16_t endFlags = (uint16_t)(flags | MM_TRACE_FLAG_END);

    if ((startUs != 0) && (now >= startUs) && ((now - startUs) >= MM_TRACE_OUTLIER_THRESHOLD_US)) {
        endFlags |= MM_TRACE_FLAG_OUTLIER;
    }
    MmPspTrace_WriteEventToRing(slot, eventType, endFlags, targetId, bufferGen, now, val0, val1);
}

static SceUID MmPspTrace_OpenFileForFlush(void) {
    MmPspTraceFileHeader header;
    const char* resolved;
    int flags;

    if (sTraceFileFailed) {
        return -1;
    }
    if (sTracePath[0] == '\0') {
        resolved = OotPsp_ResolveRootPath(MM_TRACE_FILE_NAME, sTracePath, sizeof(sTracePath));
        if (resolved == NULL || resolved[0] == '\0') {
            sTraceFileFailed = 1;
            return -1;
        }
    }
    flags = PSP_O_WRONLY | PSP_O_CREAT | (sTraceHeaderWritten ? PSP_O_APPEND : PSP_O_TRUNC);
    sTraceFd = sceIoOpen(sTracePath, flags, 0777);
    if (sTraceFd < 0) {
        sTraceFileFailed = 1;
        return -1;
    }
    if (!sTraceHeaderWritten) {
        memset(&header, 0, sizeof(header));
        memcpy(header.magic, MM_TRACE_FILE_MAGIC, 8);
        header.version = 1U;
        header.recordSize = (uint32_t)sizeof(MmPspTraceEvent);
        header.scopeMask = sScopeMask;
        header.startTimeUs = sStartTimeUs;
        if (!MmPspTrace_WriteAll(sTraceFd, &header, sizeof(header))) {
            sceIoClose(sTraceFd);
            sTraceFd = -1;
            sTraceFileFailed = 1;
            return -1;
        }
        sTraceHeaderWritten = 1;
    }
    return sTraceFd;
}

static void MmPspTrace_DrainRing(SceUID fd, MmPspTraceThreadSlot slot, uint32_t maxEvents) {
    MmPspTraceRing* ring = &sRings[slot];
    MmPspTraceEvent batch[MM_TRACE_FLUSH_BATCH];
    uint32_t drained = 0;

    while (drained < maxEvents) {
        uint32_t writeIdx = ring->writeIdx;
        uint32_t readIdx = ring->readIdx;
        uint32_t avail = writeIdx - readIdx;
        uint32_t count;
        uint32_t i;

        if (avail == 0) {
            break;
        }
        count = avail;
        if (count > MM_TRACE_FLUSH_BATCH) {
            count = MM_TRACE_FLUSH_BATCH;
        }
        if (count > (maxEvents - drained)) {
            count = maxEvents - drained;
        }
        MmPspTrace_CompilerBarrier();
        for (i = 0; i < count; i++) {
            batch[i] = ring->events[(readIdx + i) & MM_TRACE_RING_MASK];
        }
        MmPspTrace_CompilerBarrier();
        ring->readIdx = readIdx + count;
        (void)MmPspTrace_WriteAll(fd, batch, (size_t)count * sizeof(MmPspTraceEvent));
        drained += count;
    }
}

void MmPspTrace_PollFlush(void) {
    uint32_t i;
    int needFlush = 0;
    SceUID fd;

    for (i = 0; i < MM_TRACE_THREAD_COUNT; i++) {
        if ((sRings[i].writeIdx - sRings[i].readIdx) >= MM_TRACE_FLUSH_THRESHOLD) {
            needFlush = 1;
            break;
        }
    }
    if (!needFlush) {
        return;
    }
    fd = MmPspTrace_OpenFileForFlush();
    if (fd < 0) {
        return;
    }
    for (i = 0; i < MM_TRACE_THREAD_COUNT; i++) {
        MmPspTrace_DrainRing(fd, (MmPspTraceThreadSlot)i, MM_TRACE_FLUSH_THRESHOLD);
    }
    sceIoClose(fd);
    sTraceFd = -1;
}

void MmPspTrace_Flush(int forceSummary) {
    uint32_t i;
    SceUID fd = MmPspTrace_OpenFileForFlush();

    if (fd < 0) {
        return;
    }
    for (i = 0; i < MM_TRACE_THREAD_COUNT; i++) {
        MmPspTrace_DrainRing(fd, (MmPspTraceThreadSlot)i, MM_TRACE_RING_CAPACITY);
    }
    while (sOutlierReadIdx != sOutlierWriteIdx) {
        MmPspTraceEvent ev = sOutliers[sOutlierReadIdx % MM_TRACE_OUTLIER_CAPACITY];
        sOutlierReadIdx++;
        (void)MmPspTrace_WriteAll(fd, &ev, sizeof(ev));
    }
    if (forceSummary) {
        for (i = 0; i < MM_TRACE_THREAD_COUNT; i++) {
            MmPspTraceEvent ev;
            memset(&ev, 0, sizeof(ev));
            ev.magicVer = MM_TRACE_MAGIC_VER;
            ev.wakeEpoch = sWakeEpoch;
            ev.timestampUs = MmPspTrace_NowUs();
            ev.threadId = (uint16_t)i;
            ev.eventType = MM_TRACE_EVT_TRACE_SUMMARY;
            ev.flags = MM_TRACE_FLAG_INSTANT;
            ev.targetId = i;
            ev.val0 = sRings[i].totalEvents;
            ev.val1 = sRings[i].overflowCount;
            (void)MmPspTrace_WriteAll(fd, &ev, sizeof(ev));
        }
    }
    sceIoClose(fd);
    sTraceFd = -1;
}

#endif
