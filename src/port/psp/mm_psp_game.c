#include "global.h"

#include "dma.h"
#include "fault.h"
#include "idle.h"
#include "irqmgr.h"
#include "libc64/malloc.h"
#include "libu64/system_heap.h"
#include "oot_psp_asset_loader.h"
#include "oot_psp_audio_backend.h"
#include "oot_psp_memory.h"
#include "mm_psp_diag_log.h"
#include "mm_psp_trace.h"
#include "padmgr.h"
#include "PR/os_internal_flash.h"
#include "PR/os_voice.h"
#include "scheduler.h"
#include "segmented_address.h"
#include "sys_flashrom.h"
#include "z64vimode.h"
#include "z64thread.h"

#include <pspiofilemgr.h>
#include <pspkernel.h>
#include <pspthreadman.h>
#include <stdarg.h>
#include <stdio.h>

#define MM_PSP_NATIVE_ADDR_START 0x08800000U
#define MM_PSP_NATIVE_ADDR_END 0x0C000000U
#define MM_PSP_SEGMENTED_COLLISION_OFFSET_MAX 0x00010000U
#define MM_PSP_SAVE_PATH "mm-psp-save.bin"
#define MM_PSP_DIAG_LOG_PATH "mm-psp-diag.log"
#define MM_PSP_DIAG_LOG_MAX_LINES 512U
#define MM_PSP_DIAG_LOG_MAX_BYTES (64U * 1024U)
#define MM_PSP_FLASH_SIZE 0x20000U
#define MM_PSP_SAVE_IO_CHUNK_SIZE 0x4000U
#define MM_PSP_ASSET_CACHE_BLOCK_COUNT 16
#define MM_PSP_RENDERER_BLOCK_COUNT 8

typedef struct MmPspAssetCacheBlock {
    void* address;
    SceUID blockId;
    size_t size;
} MmPspAssetCacheBlock;

s32 gScreenWidth = SCREEN_WIDTH;
s32 gScreenHeight = SCREEN_HEIGHT;
size_t gSystemHeapSize;
Scheduler gScheduler;
PadMgr gPadMgr;
IrqMgr gIrqMgr;
uintptr_t gSegments[NUM_SEGMENTS];
u64* gAudioSPDataPtr;
u64 aspMainTextStart[1] __attribute__((aligned(16)));
u64 aspMainTextEnd[1] __attribute__((aligned(16)));
u64 aspMainDataStart[1] __attribute__((aligned(16)));
u64 aspMainDataEnd[1] __attribute__((aligned(16)));
STACK(aspMainStack, 0x400) __attribute__((aligned(16)));
u64 gspS2DEX2_fifoTextStart[1] __attribute__((aligned(16)));
u64 gspS2DEX2_fifoTextEnd[1] __attribute__((aligned(16)));
u64 gspS2DEX2_fifoDataStart[1] __attribute__((aligned(16)));
u64 gspS2DEX2_fifoDataEnd[1] __attribute__((aligned(16)));

OSViMode gViConfigMode;
u8 gViConfigModeType = OS_VI_NTSC_LAN1;
u8 D_80096B20 = 1;
vu8 gViConfigUseBlack = true;
u8 gViConfigAdditionalScanLines;
u32 gViConfigFeatures = OS_VI_DITHER_FILTER_ON | OS_VI_GAMMA_OFF;
f32 gViConfigXScale = 1.0f;
f32 gViConfigYScale = 1.0f;

vs32 gIrqMgrResetStatus = IRQ_RESET_STATUS_IDLE;
volatile OSTime sIrqMgrResetTime;
volatile OSTime gIrqMgrRetraceTime = OS_USEC_TO_CYCLES(16667);
s32 sIrqMgrRetraceCount;

size_t gDmaMgrDmaBuffSize = DMAMGR_DEFAULT_BUFSIZE;
f32 qNaN0x10000 = __builtin_nanf("");
Mtx D_01000000 = gdSPDefMtx(1.0f, 0.0f, 0.0f, 0.0f,
                            0.0f, 1.0f, 0.0f, 0.0f,
                            0.0f, 0.0f, 1.0f, 0.0f,
                            0.0f, 0.0f, 0.0f, 1.0f);

u8* gMmPspSystemHeap;
static SceUID sMmPspSystemHeapBlockId = -1;
static SceUID sMmPspRendererBlockIds[MM_PSP_RENDERER_BLOCK_COUNT] = { -1, -1, -1, -1 };
static size_t sMmPspRendererBlockCount;
static MmPspAssetCacheBlock sMmPspAssetCacheBlocks[MM_PSP_ASSET_CACHE_BLOCK_COUNT];
static u8 sMmPspFlash[MM_PSP_FLASH_SIZE] __attribute__((aligned(64)));
static u8 sMmPspFlashStaging[MM_PSP_FLASH_SIZE] __attribute__((aligned(64)));
static s32 sMmPspFlashInitialized;
static s32 sMmPspFlashAsyncResult;
static s32 sMmPspFlashResultPending;

static SceUID sMmPspStackThreadId = -1;
static uintptr_t sMmPspStackStart;
static uintptr_t sMmPspStackEnd;
static uintptr_t sMmPspStackAltStart;
static uintptr_t sMmPspStackAltEnd;
static s32 sMmPspDiagLogReady;
static SceUID sMmPspDiagLogSema = -1;
static u32 sMmPspDiagLogLines;
static u32 sMmPspDiagLogBytes;
static char sMmPspDiagLogPath[384];

void MmPsp_DiagLogInit(void) {
    SceUID fd;
    const char* resolved;

    if (sMmPspDiagLogReady) {
        return;
    }
    if (sMmPspDiagLogSema < 0) {
        sMmPspDiagLogSema = sceKernelCreateSema("MmPspDiagLog", 0, 1, 1, NULL);
    }
    resolved = OotPsp_ResolveRootPath(MM_PSP_DIAG_LOG_PATH, sMmPspDiagLogPath, sizeof(sMmPspDiagLogPath));
    if (resolved == NULL || resolved[0] == '\0') {
        return;
    }
    fd = sceIoOpen(sMmPspDiagLogPath, PSP_O_WRONLY | PSP_O_CREAT | PSP_O_TRUNC, 0777);
    if (fd >= 0) {
        static const char kHeader[] = "=== MM PSP Hardware Diagnostic Log ===\n";
        sceIoWrite(fd, kHeader, sizeof(kHeader) - 1);
        sceIoClose(fd);
        sMmPspDiagLogBytes = sizeof(kHeader) - 1;
        sMmPspDiagLogReady = true;
    }
}

void MmPsp_DiagLog(const char* fmt, ...) {
    char line[384];
    int len;
    va_list ap;

    if (fmt == NULL) {
        return;
    }
    va_start(ap, fmt);
    len = vsnprintf(line, sizeof(line) - 2, fmt, ap);
    va_end(ap);
    if (len < 0) {
        return;
    }
    if ((size_t)len >= sizeof(line) - 2) {
        len = (int)(sizeof(line) - 2);
    }
    if (len == 0 || line[len - 1] != '\n') {
        line[len++] = '\n';
    }
    line[len] = '\0';
    printf("%s", line);

    if (!sMmPspDiagLogReady) {
        return;
    }
    if (sMmPspDiagLogLines >= MM_PSP_DIAG_LOG_MAX_LINES || sMmPspDiagLogBytes >= MM_PSP_DIAG_LOG_MAX_BYTES) {
        return;
    }
    if (sMmPspDiagLogSema >= 0 && sceKernelWaitSema(sMmPspDiagLogSema, 1, NULL) < 0) {
        return;
    }
    if (sMmPspDiagLogLines < MM_PSP_DIAG_LOG_MAX_LINES && sMmPspDiagLogBytes < MM_PSP_DIAG_LOG_MAX_BYTES) {
        SceUID fd = sceIoOpen(sMmPspDiagLogPath, PSP_O_WRONLY | PSP_O_CREAT | PSP_O_APPEND, 0777);
        if (fd >= 0) {
            SceSSize written = sceIoWrite(fd, line, (SceSize)len);
            sceIoClose(fd);
            if (written > 0) {
                sMmPspDiagLogBytes += (u32)written;
            }
        }
        sMmPspDiagLogLines++;
        if (sMmPspDiagLogLines == MM_PSP_DIAG_LOG_MAX_LINES || sMmPspDiagLogBytes >= MM_PSP_DIAG_LOG_MAX_BYTES) {
            SceUID fdCap = sceIoOpen(sMmPspDiagLogPath, PSP_O_WRONLY | PSP_O_CREAT | PSP_O_APPEND, 0777);
            if (fdCap >= 0) {
                static const char kCap[] = "mm-psp diag log cap reached\n";
                sceIoWrite(fdCap, kCap, sizeof(kCap) - 1);
                sceIoClose(fdCap);
            }
        }
    }
    if (sMmPspDiagLogSema >= 0) {
        sceKernelSignalSema(sMmPspDiagLogSema, 1);
    }
}

static const char* MmPsp_GetSavePath(char* buffer, size_t bufferSize) {
    return OotPsp_ResolveRootPath(MM_PSP_SAVE_PATH, buffer, bufferSize);
}

static s32 MmPsp_FormatSaveSidecarPath(const char* livePath, const char* suffix, char* out, size_t outSize) {
    int written;

    if ((livePath == NULL) || (suffix == NULL) || (out == NULL) || (outSize == 0)) {
        return -1;
    }
    written = snprintf(out, outSize, "%s%s", livePath, suffix);
    if ((written < 0) || ((size_t)written >= outSize)) {
        return -1;
    }
    return 0;
}

static s32 MmPsp_SavePathExists(const char* path) {
    SceUID fd = sceIoOpen(path, PSP_O_RDONLY, 0);

    if (fd < 0) {
        return 0;
    }
    sceIoClose(fd);
    return 1;
}

static void MmPsp_SyncSaveDevice(const char* path) {
    if (path == NULL) {
        return;
    }
    if (strncmp(path, "ms0:", 4) == 0) {
        (void)sceIoSync("ms0:", 0);
    } else if (strncmp(path, "ef0:", 4) == 0) {
        (void)sceIoSync("ef0:", 0);
    }
}

static s32 MmPsp_ReadExactSaveFile(const char* path, u8* dst, size_t expectedSize, size_t* bytesReadOut) {
    SceUID fd = sceIoOpen(path, PSP_O_RDONLY, 0);
    u8* output = dst;
    size_t remaining = expectedSize;

    if (bytesReadOut != NULL) {
        *bytesReadOut = 0;
    }
    if (fd < 0) {
        return -1;
    }

    while (remaining != 0) {
        SceSize chunk = remaining > MM_PSP_SAVE_IO_CHUNK_SIZE ? MM_PSP_SAVE_IO_CHUNK_SIZE : (SceSize)remaining;
        SceSSize count = sceIoRead(fd, output, chunk);

        if (count < 0) {
            sceIoClose(fd);
            return -1;
        }
        if (count == 0) {
            break;
        }
        output += count;
        remaining -= (size_t)count;
    }
    if (sceIoClose(fd) < 0) {
        return -1;
    }
    if (bytesReadOut != NULL) {
        *bytesReadOut = expectedSize - remaining;
    }
    return (remaining == 0) ? 0 : -1;
}

static s32 MmPsp_WriteExactSaveFile(const char* path, const u8* src, size_t len) {
    SceUID fd = sceIoOpen(path, PSP_O_WRONLY | PSP_O_CREAT | PSP_O_TRUNC, 0777);
    const u8* input = src;
    size_t remaining = len;

    if (fd < 0) {
        printf("mm-psp save open failed path=%s err=%d\n", path, (int)fd);
        return -1;
    }

    while (remaining != 0) {
        SceSize chunk = remaining > MM_PSP_SAVE_IO_CHUNK_SIZE ? MM_PSP_SAVE_IO_CHUNK_SIZE : (SceSize)remaining;
        SceSSize count = sceIoWrite(fd, input, chunk);

        if (count <= 0) {
            (void)sceIoClose(fd);
            (void)sceIoRemove(path);
            return -1;
        }
        input += count;
        remaining -= (size_t)count;
    }
    if (sceIoClose(fd) < 0) {
        (void)sceIoRemove(path);
        return -1;
    }
    return 0;
}

static void MmPsp_LoadFlash(void) {
    char pathBuffer[384];
    char tmpBuffer[400];
    char bakBuffer[400];
    const char* path = MmPsp_GetSavePath(pathBuffer, sizeof(pathBuffer));
    size_t readSize = 0;

    memset(sMmPspFlash, 0xFF, sizeof(sMmPspFlash));
    if (MmPsp_ReadExactSaveFile(path, sMmPspFlash, sizeof(sMmPspFlash), &readSize) == 0) {
        if (MmPsp_FormatSaveSidecarPath(path, ".tmp", tmpBuffer, sizeof(tmpBuffer)) == 0 &&
            MmPsp_SavePathExists(tmpBuffer)) {
            (void)sceIoRemove(tmpBuffer);
        }
        printf("mm-psp save loaded path=%s size=%lu\n", path, (unsigned long)readSize);
        return;
    }

    memset(sMmPspFlash, 0xFF, sizeof(sMmPspFlash));
    if ((MmPsp_FormatSaveSidecarPath(path, ".bak", bakBuffer, sizeof(bakBuffer)) == 0) &&
        (MmPsp_ReadExactSaveFile(bakBuffer, sMmPspFlash, sizeof(sMmPspFlash), &readSize) == 0)) {
        if (MmPsp_FormatSaveSidecarPath(path, ".tmp", tmpBuffer, sizeof(tmpBuffer)) == 0 &&
            MmPsp_WriteExactSaveFile(tmpBuffer, sMmPspFlash, sizeof(sMmPspFlash)) == 0) {
            if (MmPsp_SavePathExists(path)) {
                (void)sceIoRemove(path);
            }
            if (sceIoRename(tmpBuffer, path) < 0) {
                (void)sceIoRemove(tmpBuffer);
            } else {
                MmPsp_SyncSaveDevice(path);
            }
        }
        printf("mm-psp save recovered from backup path=%s size=%lu\n", bakBuffer, (unsigned long)readSize);
        return;
    }

    memset(sMmPspFlash, 0xFF, sizeof(sMmPspFlash));
    printf("mm-psp save not found path=%s size=%lu\n", path, (unsigned long)readSize);
}

static s32 MmPsp_CommitFlash(const u8* image) {
    char pathBuffer[384];
    char tmpBuffer[400];
    char bakBuffer[400];
    const char* path = MmPsp_GetSavePath(pathBuffer, sizeof(pathBuffer));
    s32 liveExists;

    if (image == NULL) {
        return -1;
    }
    if ((MmPsp_FormatSaveSidecarPath(path, ".tmp", tmpBuffer, sizeof(tmpBuffer)) < 0) ||
        (MmPsp_FormatSaveSidecarPath(path, ".bak", bakBuffer, sizeof(bakBuffer)) < 0)) {
        return -1;
    }
    if (MmPsp_WriteExactSaveFile(tmpBuffer, image, sizeof(sMmPspFlash)) < 0) {
        return -1;
    }

    liveExists = MmPsp_SavePathExists(path);
    if (liveExists) {
        if (MmPsp_SavePathExists(bakBuffer) && (sceIoRemove(bakBuffer) < 0)) {
            (void)sceIoRemove(tmpBuffer);
            return -1;
        }
        if (sceIoRename(path, bakBuffer) < 0) {
            (void)sceIoRemove(tmpBuffer);
            return -1;
        }
    }

    if (sceIoRename(tmpBuffer, path) < 0) {
        if (liveExists) {
            (void)sceIoRename(bakBuffer, path);
        }
        (void)sceIoRemove(tmpBuffer);
        return -1;
    }

    MmPsp_SyncSaveDevice(path);
    return 0;
}

static void MmPsp_UpdateCurrentThreadStackRange(void) {
    SceUID threadId = sceKernelGetThreadId();
    SceKernelThreadInfo threadInfo;
    uintptr_t stack;
    uintptr_t stackSize;

    if (threadId == sMmPspStackThreadId) {
        return;
    }

    sMmPspStackThreadId = threadId;
    sMmPspStackStart = sMmPspStackEnd = 0;
    sMmPspStackAltStart = sMmPspStackAltEnd = 0;
    memset(&threadInfo, 0, sizeof(threadInfo));
    threadInfo.size = sizeof(threadInfo);
    if (sceKernelReferThreadStatus(threadId, &threadInfo) < 0) {
        return;
    }

    stack = (uintptr_t)threadInfo.stack;
    stackSize = (uintptr_t)threadInfo.stackSize;
    if ((stack == 0) || (stackSize == 0)) {
        return;
    }
    if (stack <= UINTPTR_MAX - stackSize) {
        sMmPspStackStart = stack;
        sMmPspStackEnd = stack + stackSize;
    }
    if (stack >= stackSize) {
        sMmPspStackAltStart = stack - stackSize;
        sMmPspStackAltEnd = stack;
    }
}

int OotPsp_IsRuntimeByteRangeSlow(uintptr_t start, uintptr_t end) {
    MmPsp_UpdateCurrentThreadStackRange();
    return ((start >= sMmPspStackStart) && (end <= sMmPspStackEnd)) ||
           ((start >= sMmPspStackAltStart) && (end <= sMmPspStackAltEnd));
}

static s32 MmPsp_IsNativeUserRange(uintptr_t address, size_t size) {
    return (size == 0) || ((address >= MM_PSP_NATIVE_ADDR_START) && (address < MM_PSP_NATIVE_ADDR_END) &&
                          (size <= MM_PSP_NATIVE_ADDR_END - address));
}

static uintptr_t MmPsp_StripKernelAlias(uintptr_t address) {
    uintptr_t stripped;

    if ((address >= 0x80000000U) && (address < 0xA0000000U)) {
        stripped = address - 0x80000000U;
        if (MmPsp_IsNativeUserRange(stripped, 1)) {
            return stripped;
        }
    } else if ((address >= 0xA0000000U) && (address < 0xC0000000U)) {
        stripped = address - 0xA0000000U;
        if (MmPsp_IsNativeUserRange(stripped, 1)) {
            return stripped;
        }
    }
    return address;
}

static s32 MmPsp_IsContainedRange(uintptr_t address, size_t size, uintptr_t rangeStart, uintptr_t rangeEnd) {
    uintptr_t end;

    if ((size == 0) || (address > UINTPTR_MAX - size)) {
        return false;
    }
    end = address + size;
    return (address >= rangeStart) && (end <= rangeEnd);
}

static s32 MmPsp_IsStaticStorageRange(uintptr_t address, size_t size) {
    if (OotPsp_IsSystemHeapRange((const void*)address, size)) {
        return true;
    }
    if (MmPsp_IsContainedRange(address, size, (uintptr_t)sMmPspFlash,
                               (uintptr_t)sMmPspFlash + sizeof(sMmPspFlash))) {
        return true;
    }
    return false;
}

static s32 MmPsp_IsKnownNativePointer(uintptr_t address, size_t size) {
    u32 flags;

    address = MmPsp_StripKernelAlias(address);
    if (!MmPsp_IsNativeUserRange(address, size)) {
        return false;
    }
    /* Executable code, rodata and initialized data precede .bss in the PRX. */
    if (address < (uintptr_t)__bss_start) {
        return true;
    }
    if (MmPsp_IsStaticStorageRange(address, size)) {
        return true;
    }
    if (OotPsp_IsRuntimeByteRange((void*)address, size)) {
        if (OotPspAudio_IsExternalPoolRange((const void*)address, size)) {
            return false;
        }
        return true;
    }
    return OotPsp_GetLoadedExternalAssetRangeFlags((void*)address, size, &flags);
}

static s32 MmPsp_AddressLooksSegmented(uintptr_t address, u32 segment) {
    address = MmPsp_StripKernelAlias(address);
    if ((segment == 0) || (segment >= NUM_SEGMENTS) || ((address & 0xF0000000U) != 0)) {
        return false;
    }
    if (MmPsp_IsKnownNativePointer(address, 1)) {
        return false;
    }
    if ((address < MM_PSP_NATIVE_ADDR_START) || (address >= MM_PSP_NATIVE_ADDR_END)) {
        return true;
    }
    return true;
}

static void* MmPsp_TranslateSegmentedAddress(uintptr_t address, u32 segment) {
    return (void*)(MmPsp_StripKernelAlias(gSegments[segment]) + SEGMENT_OFFSET(address));
}

static void MmPsp_LogUnmappedSegment(uintptr_t address, u32 segment) {
    static s32 logCount;

    if (logCount < 16) {
        MmPsp_DiagLog("mm-psp unmapped segment addr=%08lx segment=%lu offset=%06lx", (unsigned long)address,
                      (unsigned long)segment, (unsigned long)SEGMENT_OFFSET(address));
    }
    logCount++;
}

void* SegmentedToVirtualCompat(uintptr_t address) {
    u32 segment;

    if (address == 0) {
        return NULL;
    }
    address = MmPsp_StripKernelAlias(address);
    segment = SEGMENT_NUMBER(address);
    if (!MmPsp_AddressLooksSegmented(address, segment)) {
        return (void*)address;
    }
    if (gSegments[segment] != 0) {
        return MmPsp_TranslateSegmentedAddress(address, segment);
    }
    MmPsp_LogUnmappedSegment(address, segment);
    return NULL;
}

void* SegmentedToVirtualExplicit(uintptr_t address) {
    u32 segment;

    if (address == 0) {
        return NULL;
    }
    address = MmPsp_StripKernelAlias(address);
    segment = SEGMENT_NUMBER(address);
    if ((segment == 0) || (segment >= NUM_SEGMENTS) || ((address & 0xF0000000U) != 0)) {
        return (void*)address;
    }
    if (gSegments[segment] != 0) {
        return MmPsp_TranslateSegmentedAddress(address, segment);
    }
    MmPsp_LogUnmappedSegment(address, segment);
    return NULL;
}

void OotPsp_ResolveRoomList(RomFile* roomFiles, s32 count) {
    s32 i;

    for (i = 0; i < count; i++) {
        OotPsp_NormalizeRomFile(&roomFiles[i]);
    }
}

void CIC6105_Noop2(void) {
}

void* Graph_Alloc(GraphicsContext* gfxCtx, size_t size) {
    size_t alignedSize = ALIGN16(size);
    void* result = THGA_AllocTail(&gfxCtx->polyOpa, alignedSize);

    if (THGA_IsCrash(&gfxCtx->polyOpa)) {
        printf("mm-psp graph alloc overflow size=%lu remaining=%ld\n", (unsigned long)size,
               (long)THGA_GetRemaining(&gfxCtx->polyOpa));
        Fault_AddHungupAndCrash(__FILE__, __LINE__);
    }
    return result;
}

static s32 MmPsp_DmaReadInternal(void* ram, uintptr_t vrom, size_t size, s32 audioRead) {
    s32 status = audioRead ? ((OotPspAudioBackend_NeedsRefillUrgently() ||
                               OotPspAudioBackend_NeedsRefillDuringIo())
                                  ? OotPsp_AssetReadAudioUrgent(ram, vrom, size)
                                  : OotPsp_AssetReadAudio(ram, vrom, size))
                           : OotPsp_AssetRead(ram, vrom, size);

    if (status == OOT_PSP_ASSET_READ_OK) {
        return 0;
    }
    if ((status == OOT_PSP_ASSET_READ_NOT_EXTERNAL) && MmPsp_IsNativeUserRange(vrom, size)) {
        if (audioRead) {
            memcpy(ram, (const void*)vrom, size);
        } else {
            OotPsp_MemcpyVfpu(ram, (const void*)vrom, size);
        }
        return 0;
    }

    MmPsp_DiagLog("mm-psp dma failed ram=%p vrom=%08lx size=%lu status=%ld", ram, (unsigned long)vrom,
                  (unsigned long)size, (long)status);
    Fault_AddHungupAndCrash(__FILE__, __LINE__);
}

void DmaMgr_Init(void) {
}

void DmaMgr_Stop(void) {
}

s32 DmaMgr_RequestAsync(DmaRequest* req, void* ram, uintptr_t vrom, size_t size, UNK_TYPE unused,
                        OSMesgQueue* queue, void* msg) {
    (void)unused;
    MmPsp_DmaReadInternal(ram, vrom, size, false);
    if (req != NULL) {
        req->vromAddr = vrom;
        req->dramAddr = ram;
        req->size = size;
        req->notifyQueue = queue;
        req->notifyMsg = msg;
    }
    if (queue != NULL) {
        osSendMesg(queue, msg, OS_MESG_NOBLOCK);
    }
    return 0;
}

s32 DmaMgr_RequestSync(void* ram, uintptr_t vrom, size_t size) {
    return MmPsp_DmaReadInternal(ram, vrom, size, false);
}

s32 DmaMgr_DmaRomToRam(uintptr_t rom, void* ram, size_t size) {
    return MmPsp_DmaReadInternal(ram, rom, size, false);
}

s32 DmaMgr_TranslateVromToRom(uintptr_t vrom) {
    return (s32)OotPsp_NormalizeVrom(vrom);
}

s32 DmaMgr_AudioDmaHandler(OSPiHandle* pihandle, OSIoMesg* mb, s32 direction) {
    (void)pihandle;
    (void)direction;
    if (mb == NULL) {
        return -1;
    }
    MmPsp_DmaReadInternal(mb->dramAddr, mb->devAddr, mb->size, true);
    if (mb->hdr.retQueue != NULL) {
        osSendMesg(mb->hdr.retQueue, mb, OS_MESG_NOBLOCK);
    }
    return 0;
}

const char* func_800809F4(uintptr_t vrom) {
    size_t left = 0;
    size_t right = gOotPspExternalAssetCount;
    uintptr_t normalized = OotPsp_NormalizeVrom(vrom);

    while (left < right) {
        size_t mid = left + ((right - left) / 2);
        const OotPspExternalAsset* asset = &gOotPspExternalAssets[mid];

        if (normalized < asset->vromStart) {
            right = mid;
        } else if (normalized >= asset->vromEnd) {
            left = mid + 1;
        } else {
            return asset->name;
        }
    }
    return NULL;
}

static void MmPsp_ReadController(Input* inputs) {
    OSContPad pads[MAXCONTROLLERS];
    Input* input = &inputs[0];

    memset(pads, 0, sizeof(pads));
    osContStartReadData(NULL);
    osContGetReadData(pads);
    input->prev = gPadMgr.inputs[0].cur;
    input->cur = pads[0];
    input->press.button = (input->cur.button ^ input->prev.button) & input->cur.button;
    input->press.stick_x = input->cur.stick_x - input->prev.stick_x;
    input->press.stick_y = input->cur.stick_y - input->prev.stick_y;
    input->rel.button = (input->cur.button ^ input->prev.button) & input->prev.button;
    input->rel.stick_x = input->cur.stick_x;
    input->rel.stick_y = input->cur.stick_y;
#if defined(MM_PSP_TRACE) && MM_PSP_TRACE
    if ((input->press.button != 0) || (input->rel.button != 0)) {
        MmPspTrace_Record(MM_TRACE_THREAD_MAIN, MM_TRACE_EVT_INPUT_EDGE, MM_TRACE_FLAG_INSTANT,
                          (uint32_t)input->cur.button, 0,
                          (uint64_t)input->press.button, (uint64_t)input->rel.button);
    }
#endif
    PadUtils_UpdateRelXY(input);
    memset(&inputs[1], 0, sizeof(Input) * (MAXCONTROLLERS - 1));
    gPadMgr.inputs[0] = *input;
}

void PadMgr_Init(OSMesgQueue* siEvtQ, IrqMgr* irqMgr, OSId threadId, OSPri pri, void* stack) {
    (void)siEvtQ;
    (void)threadId;
    (void)pri;
    (void)stack;
    memset(&gPadMgr, 0, sizeof(gPadMgr));
    gPadMgr.irqMgr = irqMgr;
    gPadMgr.validCtrlrsMask = 1;
    gPadMgr.nControllers = 1;
    gPadMgr.ctrlrType[0] = PADMGR_CONT_NORMAL;
}

void PadMgr_GetInputNoLock(Input* inputs, s32 gameRequest) {
    (void)gameRequest;
    MmPsp_ReadController(inputs);
}

void PadMgr_GetInput(Input* inputs, s32 gameRequest) {
    PadMgr_GetInputNoLock(inputs, gameRequest);
}

void PadMgr_GetInput2(Input* inputs, s32 gameRequest) {
    PadMgr_GetInputNoLock(inputs, gameRequest);
}

u8 PadMgr_GetValidControllersMask(void) {
    return 1;
}

void PadMgr_SetRumbleRetraceCallback(void (*callback)(void*), void* arg) {
    gPadMgr.rumbleRetraceCallback = callback;
    gPadMgr.rumbleRetraceArg = arg;
}

void PadMgr_UnsetRumbleRetraceCallback(void (*callback)(void*), void* arg) {
    if ((gPadMgr.rumbleRetraceCallback == callback) && (gPadMgr.rumbleRetraceArg == arg)) {
        gPadMgr.rumbleRetraceCallback = NULL;
        gPadMgr.rumbleRetraceArg = NULL;
    }
}

void PadMgr_SetInputRetraceCallback(void (*callback)(void*), void* arg) {
    gPadMgr.inputRetraceCallback = callback;
    gPadMgr.inputRetraceArg = arg;
}

void PadMgr_UnsetInputRetraceCallback(void (*callback)(void*), void* arg) {
    if ((gPadMgr.inputRetraceCallback == callback) && (gPadMgr.inputRetraceArg == arg)) {
        gPadMgr.inputRetraceCallback = NULL;
        gPadMgr.inputRetraceArg = NULL;
    }
}

OSMesgQueue* PadMgr_VoiceAcquireSerialEventQueue(void) {
    return NULL;
}

void PadMgr_VoiceReleaseSerialEventQueue(OSMesgQueue* serialEventQueue) {
    (void)serialEventQueue;
}

void PadMgr_RumbleStop(void) {
    memset(gPadMgr.rumbleEnable, 0, sizeof(gPadMgr.rumbleEnable));
}

void PadMgr_RumblePause(void) {
    PadMgr_RumbleStop();
}

void PadMgr_RumbleSetSingle(s32 port, s32 enable) {
    if ((port >= 0) && (port < MAXCONTROLLERS)) {
        gPadMgr.rumbleEnable[port] = enable != 0;
    }
}

void PadMgr_RumbleSet(u8 enable[MAXCONTROLLERS]) {
    if (enable != NULL) {
        memcpy(gPadMgr.rumbleEnable, enable, MAXCONTROLLERS);
    }
}

s32 PadMgr_ControllerHasRumblePak(s32 port) {
    (void)port;
    return false;
}

s32 osVoiceInit(OSMesgQueue* mq, OSVoiceHandle* handle, int channel) {
    if (handle != NULL) {
        memset(handle, 0, sizeof(*handle));
        handle->__mq = mq;
        handle->__channel = channel;
    }
    return -1;
}

s32 osVoiceSetWord(OSVoiceHandle* handle, u8* word) {
    (void)handle;
    (void)word;
    return -1;
}

s32 osVoiceCheckWord(u8* word) {
    (void)word;
    return -1;
}

s32 osVoiceStartReadData(OSVoiceHandle* handle) {
    (void)handle;
    return -1;
}

s32 osVoiceStopReadData(OSVoiceHandle* handle) {
    (void)handle;
    return -1;
}

s32 osVoiceGetReadData(OSVoiceHandle* handle, OSVoiceData* result) {
    (void)handle;
    if (result != NULL) {
        memset(result, 0, sizeof(*result));
    }
    return -1;
}

s32 osVoiceClearDictionary(OSVoiceHandle* handle, u8 numWords) {
    (void)handle;
    (void)numWords;
    return -1;
}

s32 osVoiceMaskDictionary(OSVoiceHandle* handle, u8* maskPattern, int size) {
    (void)handle;
    (void)maskPattern;
    (void)size;
    return -1;
}

s32 osVoiceControlGain(OSVoiceHandle* handle, s32 analog, s32 digital) {
    (void)handle;
    (void)analog;
    (void)digital;
    return -1;
}

s32 SysFlashrom_IsInit(void) {
    return sMmPspFlashInitialized;
}

const char* SysFlashrom_GetVendorStr(void) {
    return "PSP FILE";
}

s32 SysFlashrom_CheckFlashType(void) {
    return sMmPspFlashInitialized ? 0 : -1;
}

s32 SysFlashrom_InitFlash(void) {
    if (!sMmPspFlashInitialized) {
        MmPsp_LoadFlash();
        sMmPspFlashInitialized = true;
    }
    return 0;
}

s32 SysFlashrom_Read(void* addr, u32 pageNum, u32 pageCount) {
    size_t offset = (size_t)pageNum * FLASH_BLOCK_SIZE;
    size_t size = (size_t)pageCount * FLASH_BLOCK_SIZE;

    if (!sMmPspFlashInitialized || (addr == NULL) || (offset > sizeof(sMmPspFlash)) ||
        (size > sizeof(sMmPspFlash) - offset)) {
        return -1;
    }
    memcpy(addr, &sMmPspFlash[offset], size);
    return 0;
}

void SysFlashrom_WriteAsync(void* addr, u32 pageNum, u32 pageCount) {
    size_t offset = (size_t)pageNum * FLASH_BLOCK_SIZE;
    size_t size = (size_t)pageCount * FLASH_BLOCK_SIZE;

    sMmPspFlashAsyncResult = -1;
    sMmPspFlashResultPending = false;
    if (sMmPspFlashInitialized && (addr != NULL) && (offset <= sizeof(sMmPspFlash)) &&
        (size <= sizeof(sMmPspFlash) - offset)) {
        memcpy(sMmPspFlashStaging, sMmPspFlash, sizeof(sMmPspFlash));
        memcpy(&sMmPspFlashStaging[offset], addr, size);
        sMmPspFlashAsyncResult = MmPsp_CommitFlash(sMmPspFlashStaging);
        if (sMmPspFlashAsyncResult == 0) {
            memcpy(&sMmPspFlash[offset], addr, size);
        }
    }
    sMmPspFlashResultPending = sMmPspFlashInitialized;
}

s32 SysFlashrom_IsBusy(void) {
    /* Despite its name, MM uses this as "the result queue is full". Our
     * file write finishes inline, but the caller must still collect it. */
    return sMmPspFlashInitialized ? sMmPspFlashResultPending : -1;
}

s32 SysFlashrom_AwaitResult(void) {
    if (!sMmPspFlashInitialized || !sMmPspFlashResultPending) {
        return -1;
    }
    sMmPspFlashResultPending = false;
    return sMmPspFlashAsyncResult;
}

void SysFlashrom_WriteSync(void* addr, u32 pageNum, u32 pageCount) {
    SysFlashrom_WriteAsync(addr, pageNum, pageCount);
    (void)SysFlashrom_AwaitResult();
}

void IrqMgr_AddClient(IrqMgr* irqMgr, IrqMgrClient* client, OSMesgQueue* msgQueue) {
    (void)irqMgr;
    if (client != NULL) {
        client->queue = msgQueue;
    }
}

void IrqMgr_RemoveClient(IrqMgr* irqMgr, IrqMgrClient* client) {
    (void)irqMgr;
    if (client != NULL) {
        client->queue = NULL;
    }
}

void IrqMgr_Init(IrqMgr* irqMgr, void* stack, OSPri pri, u8 retraceCount) {
    (void)stack;
    (void)pri;
    if (irqMgr != NULL) {
        memset(irqMgr, 0, sizeof(*irqMgr));
        irqMgr->resetStatus = IRQ_RESET_STATUS_IDLE;
    }
    sIrqMgrRetraceCount = retraceCount;
}

void Fault_Init(void) {
}

void Fault_AddClient(FaultClient* client, FaultClientCallback callback, void* arg0, void* arg1) {
    if (client != NULL) {
        client->callback = callback;
        client->arg0 = arg0;
        client->arg1 = arg1;
    }
}

void Fault_RemoveClient(FaultClient* client) {
    if (client != NULL) {
        client->callback = NULL;
    }
}

void Fault_AddAddrConvClient(FaultAddrConvClient* client, FaultAddrConvClientCallback callback, void* arg) {
    if (client != NULL) {
        client->callback = callback;
        client->arg = arg;
    }
}

void Fault_RemoveAddrConvClient(FaultAddrConvClient* client) {
    if (client != NULL) {
        client->callback = NULL;
    }
}

void Fault_WaitForInput(void) {
}

void Fault_FillScreenBlack(void) {
}

void Fault_SetFrameBuffer(void* fb, u16 w, u16 h) {
    (void)fb;
    (void)w;
    (void)h;
}

void FaultDrawer_SetForeColor(u16 color) {
    (void)color;
}

void FaultDrawer_SetBackColor(u16 color) {
    (void)color;
}

void FaultDrawer_SetFontColor(u16 color) {
    (void)color;
}

void FaultDrawer_SetCharPad(s8 padW, s8 padH) {
    (void)padW;
    (void)padH;
}

void FaultDrawer_SetCursor(s32 x, s32 y) {
    (void)x;
    (void)y;
}

s32 FaultDrawer_VPrintf(const char* fmt, va_list ap) {
    return vprintf(fmt, ap);
}

s32 FaultDrawer_Printf(const char* fmt, ...) {
    va_list ap;
    s32 result;

    va_start(ap, fmt);
    result = FaultDrawer_VPrintf(fmt, ap);
    va_end(ap);
    return result;
}

void FaultDrawer_DrawText(s32 x, s32 y, const char* fmt, ...) {
    va_list ap;

    (void)x;
    (void)y;
    va_start(ap, fmt);
    vprintf(fmt, ap);
    va_end(ap);
}

static void MmPsp_FaultWrite(const char* message) {
    printf("%s", message);
    sceIoWrite(1, message, strlen(message));
}

NORETURN void Fault_AddHungupAndCrashImpl(const char* exp1, const char* exp2) {
    char message[256];

    snprintf(message, sizeof(message), "mm-psp fault: %s %s\n", exp1 != NULL ? exp1 : "(null)",
             exp2 != NULL ? exp2 : "(null)");
    MmPsp_FaultWrite(message);
    MmPsp_DiagLog("%s", message);
    sceKernelExitGame();
    while (true) {
        sceKernelDelayThread(1000000);
    }
}

NORETURN void Fault_AddHungupAndCrash(const char* file, s32 line) {
    char message[256];

    snprintf(message, sizeof(message), "mm-psp fault at %s:%ld\n", file != NULL ? file : "(null)", (long)line);
    MmPsp_FaultWrite(message);
    MmPsp_DiagLog("%s", message);
    sceKernelExitGame();
    while (true) {
        sceKernelDelayThread(1000000);
    }
}

/* The shared renderer's pause/home-menu captures are permanent allocations.
 * Keep them out of the deliberately tiny newlib bootstrap heap and out of
 * MM's 8 MiB game arena. LCD mode needs about 816 KiB; Slim TV mode also
 * requests a 1.25 MiB texture cache and needs about 4.22 MiB for captures. */
void* MmPspRenderer_Memalign(size_t alignment, size_t size) {
    SceUID blockId;
    void* block;
    uintptr_t aligned;
    size_t allocationSize;

    if ((alignment == 0) || ((alignment & (alignment - 1)) != 0) ||
        (size > (size_t)-1 - (alignment - 1))) {
        return NULL;
    }
    if (sMmPspRendererBlockCount >= ARRAY_COUNT(sMmPspRendererBlockIds)) {
        printf("mm-psp renderer allocation table full size=%lu blocks=%lu\n",
               (unsigned long)size, (unsigned long)sMmPspRendererBlockCount);
        return NULL;
    }

    allocationSize = size + alignment - 1;
    blockId = sceKernelAllocPartitionMemory(PSP_MEMORY_PARTITION_USER, "MM renderer permanent", PSP_SMEM_Low,
                                            allocationSize, NULL);
    if (blockId < 0) {
        printf("mm-psp renderer allocation failed size=%lu align=%lu err=0x%08lx free=%lu largest=%lu\n",
               (unsigned long)size, (unsigned long)alignment, (unsigned long)blockId,
               (unsigned long)sceKernelTotalFreeMemSize(), (unsigned long)sceKernelMaxFreeMemSize());
        return NULL;
    }

    block = sceKernelGetBlockHeadAddr(blockId);
    if (block == NULL) {
        sceKernelFreePartitionMemory(blockId);
        return NULL;
    }

    aligned = ((uintptr_t)block + alignment - 1) & ~(uintptr_t)(alignment - 1);
    sMmPspRendererBlockIds[sMmPspRendererBlockCount++] = blockId;
    printf("mm-psp renderer allocation addr=%p size=%lu align=%lu free=%lu\n", (void*)aligned,
           (unsigned long)size, (unsigned long)alignment, (unsigned long)sceKernelTotalFreeMemSize());
    return (void*)aligned;
}

/* OoT normally gives newlib nearly all remaining RAM, and its shared asset
 * loader allocates whole pinned assets from that heap. MM reserves a small newlib
 * bootstrap heap for firmware fonts, so back those cache malloc/free calls
 * with ordinary user-partition blocks instead. In particular, MM's 5.3 MiB
 * Audiotable cannot fit in the PSP's 4 MiB volatile partition fallback. */
void* MmPspAssetCache_Malloc(size_t size) {
    MmPspAssetCacheBlock* allocation = NULL;
    SceUID blockId;
    void* address;
    size_t i;

    if (size == 0) {
        return NULL;
    }

    for (i = 0; i < ARRAY_COUNT(sMmPspAssetCacheBlocks); i++) {
        if (sMmPspAssetCacheBlocks[i].address == NULL) {
            allocation = &sMmPspAssetCacheBlocks[i];
            break;
        }
    }
    if (allocation == NULL) {
        MmPsp_DiagLog("mm-psp asset cache allocation table full size=%lu", (unsigned long)size);
        return NULL;
    }

    blockId = sceKernelAllocPartitionMemory(PSP_MEMORY_PARTITION_USER, "MM asset cache", PSP_SMEM_Low, size, NULL);
    if (blockId < 0) {
        MmPsp_DiagLog("mm-psp asset cache allocation failed size=%lu err=0x%08lx free=%lu largest=%lu",
                      (unsigned long)size, (unsigned long)blockId, (unsigned long)sceKernelTotalFreeMemSize(),
                      (unsigned long)sceKernelMaxFreeMemSize());
        return NULL;
    }

    address = sceKernelGetBlockHeadAddr(blockId);
    if (address == NULL) {
        sceKernelFreePartitionMemory(blockId);
        return NULL;
    }

    allocation->address = address;
    allocation->blockId = blockId;
    allocation->size = size;
    printf("mm-psp asset cache allocation addr=%p size=%lu free=%lu\n", address, (unsigned long)size,
           (unsigned long)sceKernelTotalFreeMemSize());
    return address;
}

void MmPspAssetCache_Free(void* address) {
    size_t i;

    if (address == NULL) {
        return;
    }

    for (i = 0; i < ARRAY_COUNT(sMmPspAssetCacheBlocks); i++) {
        MmPspAssetCacheBlock* allocation = &sMmPspAssetCacheBlocks[i];

        if (allocation->address == address) {
            sceKernelFreePartitionMemory(allocation->blockId);
            allocation->address = NULL;
            allocation->blockId = -1;
            allocation->size = 0;
            return;
        }
    }

    printf("mm-psp ignored invalid asset cache free address=%p\n", address);
}

s32 MmPspGame_ReserveHeap(void) {
    void* heap;

    if (gMmPspSystemHeap != NULL) {
        return true;
    }

    printf("mm-psp startup: allocating 8 MiB game arena (free=%lu, largest=%lu)\n",
           (unsigned long)sceKernelTotalFreeMemSize(), (unsigned long)sceKernelMaxFreeMemSize());
    sMmPspSystemHeapBlockId =
        sceKernelAllocPartitionMemory(PSP_MEMORY_PARTITION_USER, "MM 8 MiB game arena", PSP_SMEM_High,
                                      MM_PSP_SYSTEM_HEAP_SIZE, NULL);
    if (sMmPspSystemHeapBlockId < 0) {
        sMmPspSystemHeapBlockId =
            sceKernelAllocPartitionMemory(PSP_MEMORY_PARTITION_USER, "MM 8 MiB game arena", PSP_SMEM_Low,
                                          MM_PSP_SYSTEM_HEAP_SIZE, NULL);
    }
    if (sMmPspSystemHeapBlockId < 0) {
        printf("mm-psp startup: game arena allocation failed: 0x%08lx\n",
               (unsigned long)sMmPspSystemHeapBlockId);
        return false;
    }

    heap = sceKernelGetBlockHeadAddr(sMmPspSystemHeapBlockId);
    if (heap == NULL) {
        sceKernelFreePartitionMemory(sMmPspSystemHeapBlockId);
        sMmPspSystemHeapBlockId = -1;
        printf("mm-psp startup: game arena has no address\n");
        return false;
    }

    gMmPspSystemHeap = heap;
    return true;
}

s32 MmPspGame_Init(void) {
    s32 audioOutputResult;

    if (!MmPspGame_ReserveHeap()) {
        return false;
    }

    memset(&gScheduler, 0, sizeof(gScheduler));
    memset(&gPadMgr, 0, sizeof(gPadMgr));
    memset(&gIrqMgr, 0, sizeof(gIrqMgr));
    memset(gSegments, 0, sizeof(gSegments));

    gSystemHeapSize = MM_PSP_SYSTEM_HEAP_SIZE;
    SystemHeap_Init(gMmPspSystemHeap, gSystemHeapSize);
    Regs_Init();
    DmaMgr_Init();
    PadMgr_Init(NULL, &gIrqMgr, 0, 0, NULL);
    Sched_Init(&gScheduler, NULL, Z_PRIORITY_SCHED, gViConfigModeType, 1, &gIrqMgr);
    audioOutputResult = OotPspAudioBackend_Init();
    printf("mm-psp startup: audio output init=%ld external-pool=2MiB\n",
           (long)audioOutputResult);
    OotPspAudio_Init();
    return true;
}
