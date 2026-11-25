#ifndef MM_PSP_TRACE_H
#define MM_PSP_TRACE_H

#include <stdint.h>

#ifndef MM_PSP_TRACE
#define MM_PSP_TRACE 0
#endif

#ifndef MM_PSP_TRACE_SCOPES
#define MM_PSP_TRACE_SCOPES 0x1FU
#endif

#define MM_TRACE_MAGIC_VER 0x4D543031U /* 'MT01' */
#define MM_TRACE_FILE_MAGIC "MMTR0001"

#define MM_TRACE_SCOPE_AI_COL_ANIM (1U << 0)
#define MM_TRACE_SCOPE_CTOR        (1U << 1)
#define MM_TRACE_SCOPE_ASSET_XFORM (1U << 2)
#define MM_TRACE_SCOPE_TEX_UPLOAD  (1U << 3)
#define MM_TRACE_SCOPE_ME_CMD      (1U << 4)

#define MM_TRACE_OUTLIER_THRESHOLD_US 25000ULL

typedef enum MmPspTraceThreadSlot {
    MM_TRACE_THREAD_MAIN = 0,
    MM_TRACE_THREAD_VIDEO = 1,
    MM_TRACE_THREAD_AUDIO_PROD = 2,
    MM_TRACE_THREAD_AUDIO_OUT = 3,
    MM_TRACE_THREAD_OTHER = 4,
    MM_TRACE_THREAD_COUNT = 5
} MmPspTraceThreadSlot;

typedef enum MmPspTraceEventType {
    MM_TRACE_EVT_NONE = 0,
    MM_TRACE_EVT_TICK_FRAME = 1,
    MM_TRACE_EVT_TICK_SIM = 2,
    MM_TRACE_EVT_RENDER_PASS = 3,
    MM_TRACE_EVT_DL_XLATE = 4,
    MM_TRACE_EVT_GFX_SUBMIT = 5,
    MM_TRACE_EVT_GE_SYNC = 6,
    MM_TRACE_EVT_VBLANK_WAIT = 7,
    MM_TRACE_EVT_PRESENT_QUEUED = 8,
    MM_TRACE_EVT_FRONT_OBSERVED = 9,
    MM_TRACE_EVT_AUDIO_PRODUCER = 10,
    MM_TRACE_EVT_AUDIO_OUTPUT = 11,
    MM_TRACE_EVT_AUDIO_UNDERRUN = 12,
    MM_TRACE_EVT_ASSET_READ = 13,
    MM_TRACE_EVT_ASSET_LOCK_WAIT = 14,
    MM_TRACE_EVT_SCENE_INIT = 15,
    MM_TRACE_EVT_OBJECT_REQ = 16,
    MM_TRACE_EVT_OBJECT_READY = 17,
    MM_TRACE_EVT_ACTOR_FIRST_DRAW = 18,
    MM_TRACE_EVT_ACTOR_PENDING = 19,
    MM_TRACE_EVT_MEM_SAMPLE = 20,
    MM_TRACE_EVT_INPUT_EDGE = 21,
    MM_TRACE_EVT_STATE_HASH = 22,
    MM_TRACE_EVT_WAKE_EPOCH = 23,
    /* Switchable scopes */
    MM_TRACE_EVT_AI_COL_ANIM = 24,
    MM_TRACE_EVT_ACTOR_CTOR = 25,
    MM_TRACE_EVT_ASSET_XFORM = 26,
    MM_TRACE_EVT_TEX_UPLOAD = 27,
    MM_TRACE_EVT_ME_CMD = 28,
    /* Meta/summary */
    MM_TRACE_EVT_TRACE_SUMMARY = 29,
    MM_TRACE_EVT_TRACE_FLUSH = 30
} MmPspTraceEventType;

typedef enum MmPspTraceFlags {
    MM_TRACE_FLAG_START = 1U << 0,
    MM_TRACE_FLAG_END = 1U << 1,
    MM_TRACE_FLAG_INSTANT = 1U << 2,
    MM_TRACE_FLAG_SYNC_FALLBACK = 1U << 3,
    MM_TRACE_FLAG_OUTLIER = 1U << 4,
    MM_TRACE_FLAG_SCOPED = 1U << 5,
    MM_TRACE_FLAG_URGENT = 1U << 6,
    MM_TRACE_FLAG_CACHE_HIT = 1U << 7,
    MM_TRACE_FLAG_ERROR = 1U << 8,
    MM_TRACE_FLAG_OUTLIER_COPY = 1U << 9
} MmPspTraceFlags;

typedef struct MmPspTraceEvent {
    uint32_t magicVer;
    uint32_t wakeEpoch;
    uint64_t timestampUs;
    uint32_t seq;
    uint16_t threadId;
    uint16_t eventType;
    uint16_t flags;
    uint16_t passId;
    uint32_t tickId;
    uint16_t bufferGen;
    uint16_t sceneEpoch;
    uint32_t targetId;
    uint64_t val0;
    uint64_t val1;
} MmPspTraceEvent;

typedef char MmPspTraceEventSizeCheck[(sizeof(MmPspTraceEvent) == 56) ? 1 : -1];

#if MM_PSP_TRACE

void MmPspTrace_Init(void);
void MmPspTrace_RegisterThread(MmPspTraceThreadSlot slot);
MmPspTraceThreadSlot MmPspTrace_CurrentSlot(void);
uint64_t MmPspTrace_NowUs(void);
void MmPspTrace_SetScopeMask(uint32_t mask);
uint32_t MmPspTrace_GetScopeMask(void);
void MmPspTrace_OnWake(void);
void MmPspTrace_SetTickContext(uint32_t tickId, uint16_t sceneEpoch, uint16_t passId);
void MmPspTrace_SetPassId(uint16_t passId);
void MmPspTrace_Record(MmPspTraceThreadSlot slot, uint16_t eventType, uint16_t flags,
                       uint32_t targetId, uint16_t bufferGen, uint64_t val0, uint64_t val1);
uint64_t MmPspTrace_BeginSpan(MmPspTraceThreadSlot slot, uint16_t eventType, uint16_t flags,
                              uint32_t targetId, uint16_t bufferGen, uint64_t val0, uint64_t val1);
void MmPspTrace_EndSpan(MmPspTraceThreadSlot slot, uint16_t eventType, uint16_t flags,
                        uint32_t targetId, uint16_t bufferGen, uint64_t startUs, uint64_t val0,
                        uint64_t val1);
void MmPspTrace_PollFlush(void);
void MmPspTrace_Flush(int forceSummary);

static inline int MmPspTrace_ScopeEnabled(uint32_t scopeBit) {
    return (MmPspTrace_GetScopeMask() & scopeBit) != 0;
}

#else

static inline void MmPspTrace_Init(void) {}
static inline void MmPspTrace_RegisterThread(MmPspTraceThreadSlot slot) { (void)slot; }
static inline MmPspTraceThreadSlot MmPspTrace_CurrentSlot(void) { return MM_TRACE_THREAD_MAIN; }
static inline uint64_t MmPspTrace_NowUs(void) { return 0; }
static inline void MmPspTrace_SetScopeMask(uint32_t mask) { (void)mask; }
static inline uint32_t MmPspTrace_GetScopeMask(void) { return 0; }
static inline void MmPspTrace_OnWake(void) {}
static inline void MmPspTrace_SetTickContext(uint32_t tickId, uint16_t sceneEpoch, uint16_t passId) {
    (void)tickId; (void)sceneEpoch; (void)passId;
}
static inline void MmPspTrace_SetPassId(uint16_t passId) { (void)passId; }
static inline void MmPspTrace_Record(MmPspTraceThreadSlot slot, uint16_t eventType, uint16_t flags,
                                     uint32_t targetId, uint16_t bufferGen, uint64_t val0, uint64_t val1) {
    (void)slot; (void)eventType; (void)flags; (void)targetId; (void)bufferGen; (void)val0; (void)val1;
}
static inline uint64_t MmPspTrace_BeginSpan(MmPspTraceThreadSlot slot, uint16_t eventType, uint16_t flags,
                                            uint32_t targetId, uint16_t bufferGen, uint64_t val0, uint64_t val1) {
    (void)slot; (void)eventType; (void)flags; (void)targetId; (void)bufferGen; (void)val0; (void)val1;
    return 0;
}
static inline void MmPspTrace_EndSpan(MmPspTraceThreadSlot slot, uint16_t eventType, uint16_t flags,
                                      uint32_t targetId, uint16_t bufferGen, uint64_t startUs, uint64_t val0,
                                      uint64_t val1) {
    (void)slot; (void)eventType; (void)flags; (void)targetId; (void)bufferGen; (void)startUs;
    (void)val0; (void)val1;
}
static inline void MmPspTrace_PollFlush(void) {}
static inline void MmPspTrace_Flush(int forceSummary) { (void)forceSummary; }
static inline int MmPspTrace_ScopeEnabled(uint32_t scopeBit) { (void)scopeBit; return 0; }

#endif

#endif
