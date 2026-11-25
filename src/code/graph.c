#include "ultra64.h"

#include "scheduler.h"

// Variables are put before most headers as a hacky way to bypass bss reordering
struct FaultAddrConvClient sGraphFaultAddrConvClient;
struct FaultClient sGraphFaultClient;
struct GfxMasterList* gGfxMasterDL;
CfbInfo sGraphCfbInfos[3];
OSTime sGraphPrevUpdateEndTime;

#include "regs.h"
#include "fault.h"

#include "macros.h"
#include "buffers.h"
#include "idle.h"
#include "sys_cfb.h"
#include "sys_ucode.h"
#include "libc64/malloc.h"
#include "z64DLF.h"
#include "z64speed_meter.h"

#include "overlays/gamestates/ovl_daytelop/z_daytelop.h"
#include "overlays/gamestates/ovl_file_choose/z_file_select.h"
#include "overlays/gamestates/ovl_opening/z_opening.h"
#include "overlays/gamestates/ovl_select/z_select.h"
#include "overlays/gamestates/ovl_title/z_title.h"
#include "z_title_setup.h"
#if defined(TARGET_PSP) || defined(PLATFORM_PSP)
#include "mm_psp_diag_log.h"
#include "mm_psp_trace.h"
#include "oot_psp_audio_backend.h"
#include "oot_psp_frame_interpolation.h"
#include "oot_psp_performance.h"
#include "oot_psp_video.h"
#include "z64save.h"
#endif

void Graph_FaultClient(void) {
    FaultDrawer_DrawText(30, 100, "ShowFrameBuffer PAGE 0/1");
    osViSwapBuffer(SysCfb_GetFramebuffer(0));
    osViSetMode(gActiveViMode);
    osViSetSpecialFeatures(OS_VI_DITHER_FILTER_ON | OS_VI_GAMMA_OFF);
    Fault_WaitForInput();
    osViSwapBuffer(SysCfb_GetFramebuffer(1));
    osViSetMode(gActiveViMode);
    osViSetSpecialFeatures(OS_VI_DITHER_FILTER_ON | OS_VI_GAMMA_OFF);
}

void Graph_InitTHGA(TwoHeadGfxArena* arena, Gfx* buffer, s32 size) {
    THGA_Init(arena, buffer, size);
}

void Graph_SetNextGfxPool(GraphicsContext* gfxCtx) {
    GfxPool* pool = &gGfxPools[gfxCtx->gfxPoolIdx % 2];

    gGfxMasterDL = &pool->master;
    gSegments[0x0E] = (uintptr_t)gGfxMasterDL;

    pool->headMagic = GFXPOOL_HEAD_MAGIC;
    pool->tailMagic = GFXPOOL_TAIL_MAGIC;

    Graph_InitTHGA(&gfxCtx->polyOpa, pool->polyOpaBuffer, sizeof(pool->polyOpaBuffer));
    Graph_InitTHGA(&gfxCtx->polyXlu, pool->polyXluBuffer, sizeof(pool->polyXluBuffer));
    Graph_InitTHGA(&gfxCtx->overlay, pool->overlayBuffer, sizeof(pool->overlayBuffer));
    Graph_InitTHGA(&gfxCtx->work, pool->workBuffer, sizeof(pool->workBuffer));
    Graph_InitTHGA(&gfxCtx->debug, pool->debugBuffer, sizeof(pool->debugBuffer));

    gfxCtx->polyOpaBuffer = pool->polyOpaBuffer;
    gfxCtx->polyXluBuffer = pool->polyXluBuffer;
    gfxCtx->overlayBuffer = pool->overlayBuffer;
    gfxCtx->workBuffer = pool->workBuffer;
    gfxCtx->debugBuffer = pool->debugBuffer;

    gfxCtx->curFrameBuffer = SysCfb_GetFramebuffer(gfxCtx->framebufferIndex % 2);
    gSegments[0x0F] = (uintptr_t)gfxCtx->curFrameBuffer;

    gfxCtx->zbuffer = SysCfb_GetZBuffer();

    gSPBranchList(&gGfxMasterDL->disps[0], pool->polyOpaBuffer);
    gSPBranchList(&gGfxMasterDL->disps[1], pool->polyXluBuffer);
    gSPBranchList(&gGfxMasterDL->disps[2], pool->overlayBuffer);
    gSPBranchList(&gGfxMasterDL->disps[3], pool->workBuffer);
    gSPEndDisplayList(&gGfxMasterDL->disps[4]);
    gSPBranchList(&gGfxMasterDL->debugDisp[0], pool->debugBuffer);
}

GameStateOverlay* Graph_GetNextGameState(GameState* gameState) {
    GameStateFunc gameStateInit = GameState_GetInit(gameState);

    // Generates code to match gameStateInit to a gamestate entry and returns it if found
#define DEFINE_GAMESTATE_INTERNAL(typeName, enumName) \
    if (gameStateInit == typeName##_Init) {           \
        return &gGameStateOverlayTable[enumName];     \
    }
#define DEFINE_GAMESTATE(typeName, enumName, name) DEFINE_GAMESTATE_INTERNAL(typeName, enumName)

#include "tables/gamestate_table.h"

#undef DEFINE_GAMESTATE
#undef DEFINE_GAMESTATE_INTERNAL

    return NULL;
}

uintptr_t Graph_FaultAddrConv(uintptr_t address, void* param) {
    uintptr_t addr = address;
    GameStateOverlay* gameStateOvl = &gGameStateOverlayTable[0];
    uintptr_t ramConv;
    void* ramStart;
    size_t diff;
    s32 i;

    for (i = 0; i < gGraphNumGameStates; i++, gameStateOvl++) {
        diff = (uintptr_t)gameStateOvl->vramEnd - (uintptr_t)gameStateOvl->vramStart;
        ramStart = gameStateOvl->loadedRamAddr;
        ramConv = (uintptr_t)gameStateOvl->vramStart - (uintptr_t)ramStart;

        if (ramStart != NULL) {
            if ((addr >= (uintptr_t)ramStart) && (addr < (uintptr_t)ramStart + diff)) {
                return addr + ramConv;
            }
        }
    }
    return 0;
}

void Graph_Init(GraphicsContext* gfxCtx) {
    bzero(gfxCtx, sizeof(GraphicsContext));
    gfxCtx->gfxPoolIdx = 0;
    gfxCtx->framebufferIndex = 0;
    gfxCtx->viMode = NULL;
    gfxCtx->viConfigFeatures = gViConfigFeatures;
    gfxCtx->xScale = gViConfigXScale;
    gfxCtx->yScale = gViConfigYScale;
    osCreateMesgQueue(&gfxCtx->queue, gfxCtx->msgBuff, ARRAY_COUNT(gfxCtx->msgBuff));
    Fault_AddClient(&sGraphFaultClient, (void*)Graph_FaultClient, NULL, NULL);
    Fault_AddAddrConvClient(&sGraphFaultAddrConvClient, Graph_FaultAddrConv, NULL);
}

void Graph_Destroy(GraphicsContext* gfxCtx) {
    Fault_RemoveClient(&sGraphFaultClient);
    Fault_RemoveAddrConvClient(&sGraphFaultAddrConvClient);
}

/**
 * Constructs the graphics OSTask and forwards it to the scheduler.
 * Waits for up to 3 additional seconds for any current graphics task to complete.
 * If it does not signal completion in that time, retry or trigger a crash.
 */
void Graph_TaskSet00(GraphicsContext* gfxCtx, GameState* gameState) {
    static s32 sRetryCount = 10;
    static s32 sCfbIndex = 0;
    OSTask_t* task = &gfxCtx->task.list.t;
    OSScTask* scTask = &gfxCtx->task;
    OSTimer timer;
    OSMesg msg;
    s32 pad;

retry:
    osSetTimer(&timer, OS_USEC_TO_CYCLES(3 * 1000 * 1000), 0, &gfxCtx->queue, (OSMesg)666);
    osRecvMesg(&gfxCtx->queue, &msg, OS_MESG_BLOCK);
    osStopTimer(&timer);

    if (msg == (OSMesg)666) {
        osSyncPrintf("GRAPH SP TIMEOUT\n");
        if (sRetryCount >= 0) {
            sRetryCount--;
            Sched_SendGfxCancelMsg(&gScheduler);
            goto retry;
        } else {
            osSyncPrintf(T("graph.c:もうダメ！死ぬ！\n", "graph.c: I can't go on! I'm dying!\n"));
            Fault_AddHungupAndCrashImpl("RCP is HUNG UP!!", "Oh! MY GOD!!");
        }
    }

    gfxCtx->masterList = gGfxMasterDL;
    if (gfxCtx->callback != NULL) {
        gfxCtx->callback(gfxCtx, gfxCtx->callbackArg);
    }

    task->type = M_GFXTASK;
    task->flags = OS_SC_DRAM_DLIST;
    task->ucode_boot = SysUcode_GetUCodeBoot();
    task->ucode_boot_size = SysUcode_GetUCodeBootSize();
    task->ucode = SysUcode_GetUCode();
    task->ucode_data = SysUcode_GetUCodeData();
    task->ucode_size = SP_UCODE_SIZE;
    task->ucode_data_size = SP_UCODE_DATA_SIZE;
    task->dram_stack = gGfxSPTaskStack;
    task->dram_stack_size = sizeof(gGfxSPTaskStack);
    task->output_buff = gGfxSPTaskOutputBufferPtr;
    task->output_buff_size = gGfxSPTaskOutputBufferEnd;
    task->data_ptr = (u64*)gGfxMasterDL;
    task->data_size = 0;
    task->yield_data_ptr = gGfxSPTaskYieldBuffer;
    task->yield_data_size = sizeof(gGfxSPTaskYieldBuffer);

    scTask->next = NULL;
    scTask->flags = OS_SC_RCP_MASK | OS_SC_SWAPBUFFER | OS_SC_LAST_TASK;

    if (SREG(33) & 1) {
        SREG(33) &= ~1;
        scTask->flags &= ~OS_SC_SWAPBUFFER;
        gfxCtx->framebufferIndex--;
    }

    scTask->msgQ = &gfxCtx->queue;
    scTask->msg = NULL;

    {
        CfbInfo* cfb = &sGraphCfbInfos[sCfbIndex];

        sCfbIndex = (sCfbIndex + 1) % ARRAY_COUNT(sGraphCfbInfos);

        cfb->framebuffer = gfxCtx->curFrameBuffer;
        cfb->swapBuffer = gfxCtx->curFrameBuffer;

        if (gfxCtx->updateViMode) {
            gfxCtx->updateViMode = false;
            cfb->viMode = gfxCtx->viMode;
            cfb->viFeatures = gfxCtx->viConfigFeatures;
            cfb->xScale = gfxCtx->xScale;
            cfb->yScale = gfxCtx->yScale;
        } else {
            cfb->viMode = NULL;
        }
        cfb->unk_10 = 0;
        cfb->updateRate = gameState->framerateDivisor;

        scTask->framebuffer = cfb;
    }

    while (gfxCtx->queue.validCount != 0) {
        osRecvMesg(&gfxCtx->queue, NULL, OS_MESG_NOBLOCK);
    }

    gfxCtx->schedMsgQ = &gScheduler.cmdQueue;
    osSendMesg(&gScheduler.cmdQueue, (OSMesg)scTask, OS_MESG_BLOCK);
    Sched_SendNotifyMsg(&gScheduler);
}

#if defined(TARGET_PSP) && (defined(OOTDEBUG) || MM_PSP_TRACE)
static u64 sGraphTickAudioUsec;
#endif

#if defined(TARGET_PSP) && MM_PSP_TRACE
static u32 sGraphTraceTickId;
static u16 sGraphTraceSceneEpoch;
static s32 sGraphLastSceneId = -1;

static u64 Graph_ComputeStableStateHash(const GameState* gameState, const PlayState* play) {
    u64 h = 14695981039346656037ULL;
    const u32* p;
    u32 tmp[10];
    size_t i;

    tmp[0] = (u32)gameState->frames;
    tmp[1] = (u32)gameState->framerateDivisor;
    tmp[2] = play != NULL ? (u32)play->gameplayFrames : 0U;
    tmp[3] = play != NULL ? (u32)play->sceneId : 0xFFFFFFFFU;
    tmp[4] = play != NULL ? (u32)play->roomCtx.curRoom.num : 0xFFFFFFFFU;
    tmp[5] = play != NULL ? (u32)play->csCtx.state : 0U;
    tmp[6] = (u32)gSaveContext.save.time;
    tmp[7] = (u32)gSaveContext.save.day;
    tmp[8] = (u32)gSaveContext.save.saveInfo.playerData.health;
    tmp[9] = (u32)gSaveContext.save.playerForm;
    p = tmp;
    for (i = 0; i < ARRAY_COUNT(tmp); i++) {
        h ^= p[i];
        h *= 1099511628211ULL;
    }
    return h;
}
#endif

void Graph_UpdateGame(GameState* gameState) {
#if defined(TARGET_PSP) && (defined(OOTDEBUG) || MM_PSP_TRACE)
    u64 audioStartUsec = 0;
#endif
    GameState_GetInput(gameState);
    GameState_IncrementFrameCount(gameState);
    if (R_AUDIOMGR_DEBUG_LEVEL < 3) {
#if defined(TARGET_PSP) && defined(OOTDEBUG)
        audioStartUsec = OotPspPerformance_Now();
#elif defined(TARGET_PSP) && MM_PSP_TRACE
        audioStartUsec = MmPspTrace_NowUs();
#endif
        Audio_Update();
#if defined(TARGET_PSP) || defined(PLATFORM_PSP)
        OotPspAudio_Update();
#endif
#if defined(TARGET_PSP) && defined(OOTDEBUG)
        sGraphTickAudioUsec += OotPspPerformance_Now() - audioStartUsec;
#elif defined(TARGET_PSP) && MM_PSP_TRACE
        sGraphTickAudioUsec += MmPspTrace_NowUs() - audioStartUsec;
#endif
    }
}

/**
 *  Run the game state logic, then finalize the gfx buffer
 *  and run the graphics task for this frame.
 */
void Graph_ExecuteAndDraw(GraphicsContext* gfxCtx, GameState* gameState) {
    u32 problem;
#if defined(TARGET_PSP) && MM_PSP_TRACE
    PlayState* tracePlay = gameState->main == Play_Main ? (PlayState*)gameState : NULL;
    u64 simStartUs;
#endif

    gameState->unk_A3 = 0;
    Graph_SetNextGfxPool(gfxCtx);

#if PLATFORM_PSP
    {
        PlayState* play = gameState->main == Play_Main ? (PlayState*)gameState : NULL;
        OotPspFrameInterpolation_Begin(gameState, play ? play->sceneId : -1,
                                      play ? play->roomCtx.curRoom.num : -1,
                                      play ? play->activeCamId : -1,
                                      play != NULL && play->pauseCtx.state == 0 &&
                                          OotPspVideo_GetFrameRate() > 20);
    }
#endif
#if defined(TARGET_PSP) && MM_PSP_TRACE
    simStartUs = MmPspTrace_BeginSpan(MM_TRACE_THREAD_MAIN, MM_TRACE_EVT_TICK_SIM, 0,
                                      (u32)(tracePlay != NULL ? tracePlay->gameplayFrames : gameState->frames),
                                      0, (u64)gameState->framerateDivisor,
                                      (u64)(tracePlay != NULL ? tracePlay->sceneId : -1));
#endif
    GameState_Update(gameState);
#if defined(TARGET_PSP) && MM_PSP_TRACE
    MmPspTrace_EndSpan(MM_TRACE_THREAD_MAIN, MM_TRACE_EVT_TICK_SIM, 0,
                       (u32)(tracePlay != NULL ? tracePlay->gameplayFrames : gameState->frames),
                       0, simStartUs, (u64)gameState->framerateDivisor,
                       (u64)(tracePlay != NULL ? tracePlay->roomCtx.curRoom.num : -1));
    MmPspTrace_Record(MM_TRACE_THREAD_MAIN, MM_TRACE_EVT_STATE_HASH, MM_TRACE_FLAG_INSTANT,
                      (u32)(tracePlay != NULL ? tracePlay->gameplayFrames : gameState->frames), 0,
                      Graph_ComputeStableStateHash(gameState, tracePlay),
                      (u64)(tracePlay != NULL ? tracePlay->sceneId : -1));
#endif
#if PLATFORM_PSP
    OotPspFrameInterpolation_End();
#endif

    OPEN_DISPS(gfxCtx);

    gSPEndDisplayList(WORK_DISP++);
    gSPEndDisplayList(POLY_OPA_DISP++);
    gSPEndDisplayList(POLY_XLU_DISP++);
    gSPEndDisplayList(OVERLAY_DISP++);
    gSPEndDisplayList(DEBUG_DISP++);

    CLOSE_DISPS(gfxCtx);

    {
        Gfx* gfx = gGfxMasterDL->taskStart;

        gSPSegment(gfx++, 0x0E, gGfxMasterDL);
        gSPDisplayList(gfx++, &D_0E000000.disps[3]);
        gSPDisplayList(gfx++, &D_0E000000.disps[0]);
        gSPDisplayList(gfx++, &D_0E000000.disps[1]);
        gSPDisplayList(gfx++, &D_0E000000.disps[2]);
        gSPDisplayList(gfx++, &D_0E000000.debugDisp[0]);

        gDPPipeSync(gfx++);
        gDPFullSync(gfx++);
        gSPEndDisplayList(gfx++);
    }

    problem = false;

    {
        GfxPool* pool = &gGfxPools[gfxCtx->gfxPoolIdx % 2];

        if (pool->headMagic != GFXPOOL_HEAD_MAGIC) {
            Fault_AddHungupAndCrash("../graph.c", 1054);
        }
        if (pool->tailMagic != GFXPOOL_TAIL_MAGIC) {
            Fault_AddHungupAndCrash("../graph.c", 1066);
        }
    }

    {
        static const char* const sThgaNames[] = { "polyOpa", "polyXlu", "overlay", "work", "debug" };
        TwoHeadGfxArena* thgas[] = {
            &gfxCtx->polyOpa, &gfxCtx->polyXlu, &gfxCtx->overlay, &gfxCtx->work, &gfxCtx->debug,
        };
        static u8 sThgaCrashLoggedMask = 0;

        for (size_t i = 0; i < ARRAY_COUNT(thgas); i++) {
            if (THGA_IsCrash(thgas[i])) {
                problem = true;
#if defined(TARGET_PSP) || defined(PLATFORM_PSP)
                if (!(sThgaCrashLoggedMask & (1U << i))) {
                    sThgaCrashLoggedMask |= (u8)(1U << i);
                    MmPsp_DiagLog("mm-psp thga crash buf=%s size=%lu rem=%ld", sThgaNames[i],
                                  (unsigned long)thgas[i]->size, (long)THGA_GetRemaining(thgas[i]));
                }
#else
                (void)sThgaNames;
                (void)sThgaCrashLoggedMask;
#endif
            }
        }
    }

    if (!problem) {
        Graph_TaskSet00(gfxCtx, gameState);
        gfxCtx->gfxPoolIdx++;
        gfxCtx->framebufferIndex++;
    }

    {
        OSTime time = osGetTime();

        gRSPGfxTimeTotal = gRSPGfxTimeAcc;
        gRSPAudioTimeTotal = gRSPAudioTimeAcc;
        gRDPTimeTotal = gRDPTimeAcc;
        gRSPGfxTimeAcc = 0;
        gRSPAudioTimeAcc = 0;
        gRDPTimeAcc = 0;

        if (sGraphPrevUpdateEndTime != 0) {
            gGraphUpdatePeriod = time - sGraphPrevUpdateEndTime;
        }
        sGraphPrevUpdateEndTime = time;
    }
}

void Graph_Update(GraphicsContext* gfxCtx, GameState* gameState) {
#if defined(TARGET_PSP) && (defined(OOTDEBUG) || MM_PSP_TRACE)
    PlayState* play = gameState->main == Play_Main ? (PlayState*)gameState : NULL;
    s32 sceneId = play != NULL ? play->sceneId : -1;
    s32 roomId = play != NULL ? play->roomCtx.curRoom.num : -1;
#endif
#if defined(TARGET_PSP) && defined(OOTDEBUG)
    u64 frameStartUsec = OotPspPerformance_Now();
    u64 gameStartUsec;
    u64 gameEndUsec;
#endif
#if defined(TARGET_PSP) && MM_PSP_TRACE
    u64 traceStartUs;
#endif

    gameState->unk_A3 = 0;
#if defined(TARGET_PSP) && (defined(OOTDEBUG) || MM_PSP_TRACE)
    sGraphTickAudioUsec = 0;
#endif
#if defined(TARGET_PSP) && defined(OOTDEBUG)
    OotPspPerformance_BeginFrame(sceneId, roomId);
    gameStartUsec = OotPspPerformance_Now();
#endif
#if defined(TARGET_PSP) && MM_PSP_TRACE
    sGraphTraceTickId++;
    if (sceneId != sGraphLastSceneId) {
        sGraphLastSceneId = sceneId;
        sGraphTraceSceneEpoch++;
    }
    MmPspTrace_SetTickContext(sGraphTraceTickId, sGraphTraceSceneEpoch, 0);
    traceStartUs = MmPspTrace_BeginSpan(MM_TRACE_THREAD_MAIN, MM_TRACE_EVT_TICK_FRAME, 0,
                                        (u32)sceneId, 0, (u64)roomId, (u64)gameState->framerateDivisor);
#endif

    Graph_UpdateGame(gameState);
    Graph_ExecuteAndDraw(gfxCtx, gameState);

#if defined(TARGET_PSP) && defined(OOTDEBUG)
    gameEndUsec = OotPspPerformance_Now();
    OotPspPerformance_EndFrame(gameEndUsec - gameStartUsec, sGraphTickAudioUsec, gameEndUsec - frameStartUsec);
#endif
#if defined(TARGET_PSP) && MM_PSP_TRACE
    MmPspTrace_EndSpan(MM_TRACE_THREAD_MAIN, MM_TRACE_EVT_TICK_FRAME, 0,
                       (u32)sceneId, 0, traceStartUs, (u64)roomId, sGraphTickAudioUsec);
#endif
}

void Graph_ThreadEntry(void* arg) {
    GraphicsContext gfxCtx;
    GameStateOverlay* nextOvl = &gGameStateOverlayTable[0];
    GameStateOverlay* ovl;
    GameState* gameState;
    size_t size;
    s32 pad[2];

    gZBufferLoRes = malloc(sizeof(*gZBufferLoRes) + sizeof(*gWorkBufferLoRes) + 64 - 1);
    gZBufferLoRes = (void*)ALIGN64((uintptr_t)gZBufferLoRes);

    gWorkBufferLoRes = (void*)((u8*)gZBufferLoRes + sizeof(*gZBufferLoRes));

    gGfxSPTaskOutputBufferHiRes = gGfxSPTaskOutputBufferLoRes = malloc(sizeof(*gGfxSPTaskOutputBufferLoRes));

    gGfxSPTaskOutputBufferEndLoRes = (u8*)gGfxSPTaskOutputBufferLoRes + sizeof(*gGfxSPTaskOutputBufferLoRes);
    gGfxSPTaskOutputBufferEndHiRes = (u8*)gGfxSPTaskOutputBufferHiRes + sizeof(*gGfxSPTaskOutputBufferHiRes);

    SysCfb_Init();
    Fault_SetFrameBuffer(gWorkBuffer, SCREEN_WIDTH, SCREEN_HEIGHT);
    Graph_Init(&gfxCtx);

    while (nextOvl) {
        ovl = nextOvl;

        Overlay_LoadGameState(ovl);

        size = ovl->instanceSize;

        func_800809F4(ovl->file.vromStart);

        gameState = malloc(size);

        bzero(gameState, size);
        GameState_Init(gameState, ovl->init, &gfxCtx);

        while (GameState_IsRunning(gameState)) {
            Graph_Update(&gfxCtx, gameState);
        }

        nextOvl = Graph_GetNextGameState(gameState);

        if (size) {}

        GameState_Destroy(gameState);
        free(gameState);

        Overlay_FreeGameState(ovl);
    }
    Graph_Destroy(&gfxCtx);
}
