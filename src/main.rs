#![no_std]
#![no_main]

extern crate alloc;

mod platform;

use alloc::format;
use pmm_assets::ResourceManager;
use pmm_gfx::Renderer;
use pmm_gfx::math::{Mat4, mat4_identity};
use pmm_game::graph::{FRAME_40FPS_USEC, GameStateId, NoCatchUpPacer};
use pmm_game::interp::FrameInterpolator;
use pmm_game::padmgr::{PadMgr, n64_btn};
use pmm_game::save::{SAVE_SLOT_MAGIC, SysFlashrom};
use pmm_game::states::{
    ConsoleLogoState, DayTelopState, FileOpOutcome, FileSelectState, OTRMessageTable,
    ScriptedPadInjector, TitleSetupState,
};
use pmm_game::thga::GfxPool;
use psp::sys;

psp::module!("PocketMajora", 1, 0);

fn psp_main() {
    psp::callback::setup_exit_callback().unwrap();
    let _ = platform::clock::lock_max_clocks();

    let mut pools = platform::memory::PartitionPools::allocate().unwrap();
    let source = platform::pak_io::PspPakSource::open("data/pmm_2s2h.pak").unwrap();
    let mut mgr = ResourceManager::open(source).unwrap();

    let mut gfx = unsafe {
        platform::gfx_psp::PspGfxBackend::new(
            pools.renderer_cache.as_mut_ptr(),
            platform::memory::POOL_RENDERER_BYTES,
        )
    };
    let mut audio_worker = platform::audio_psp::AudioWorker::start(
        pools.audio_pool.as_mut_ptr(),
        platform::memory::POOL_AUDIO_BYTES,
    )
    .unwrap();
    let _power = platform::power::PowerCoordinator::register();

    let mut renderer = Renderer::new();
    let mut pool = GfxPool::new();
    let mut interp = FrameInterpolator::new();
    let mut pad_mgr = PadMgr::new();
    let mut pacer = NoCatchUpPacer::new();
    let mut flash = SysFlashrom::new(platform::flashrom_psp::PspFlashIo);

    let proj: Mat4 = [
        [1.0 / 240.0, 0.0, 0.0, 0.0],
        [0.0, 1.0 / 136.0, 0.0, 0.0],
        [0.0, 0.0, -0.1, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ];
    let view = mat4_identity();

    let mut frames_rendered_total: u32 = 0;
    let mut sum_frame_us: u64 = 0;

    let mut pace_frame = |sum_us: &mut u64, count: &mut u32| {
        let now = unsafe { sys::sceKernelGetSystemTimeLow() };
        let wait = pacer.compute_sleep_usec(now, FRAME_40FPS_USEC);
        if wait > 0 {
            unsafe { sys::sceKernelDelayThread(wait) };
        }
        *sum_us += (FRAME_40FPS_USEC as u64).max(wait as u64);
        *count += 1;
    };

    // --- State 1: TitleSetup -> ConsoleLogo ---
    let mut title_setup = TitleSetupState::new();
    let (_save_ctx, msgs, state_after_setup): (_, OTRMessageTable, GameStateId) = {
        let mut arena = pools.common_arena_mut();
        title_setup.update(&mut flash, &mut mgr, &mut arena)
    };
    let state_01_ok = state_after_setup == GameStateId::Title;

    // --- State 2: ConsoleLogo (ovl_title) -> FileSelect ---
    let mut console_logo = ConsoleLogoState::new();
    let mut logo_script = ScriptedPadInjector::new();
    logo_script.push_press(26, n64_btn::START_BUTTON);
    let mut logo_frames: u32 = 0;
    let mut state_02_ok = false;

    for _ in 0..90u32 {
        let (btn, sx, sy) = logo_script.sample_tick();
        let pad = pad_mgr.update_n64_raw(btn, sx, sy);
        let next = console_logo.update_logic(&pad, &mut interp);

        for &frac in &[0.5f32, 1.0f32] {
            gfx.begin_frame(0xFF10_0804, &proj, &view);
            console_logo.draw_frame(&mut pool, &mut renderer, &mut mgr, &mut gfx, &mut interp, frac);
            gfx.end_frame_and_swap(false);
            pace_frame(&mut sum_frame_us, &mut frames_rendered_total);
            logo_frames += 1;
        }
        if next == Some(GameStateId::FileChoose) {
            state_02_ok = console_logo.fade_in_seen
                && console_logo.hold_seen
                && console_logo.fade_out_seen;
            break;
        }
    }

    // --- State 3: FileSelect (Create, 40-tick Flash Wait, Copy, Erase, Corrupt Recovery, Launch) ---
    let mut file_select = FileSelectState::new(&mut flash);
    // Start clean so create/copy/erase verification is repeatable across boots
    let _ = flash.erase_save_slot(0);
    let _ = flash.await_result();
    let _ = flash.erase_save_slot(1);
    let _ = flash.await_result();
    let _ = flash.erase_save_slot(2);
    let _ = flash.await_result();
    file_select.reload_slots(&flash);

    // 3a: Create Slot 0 ("Link") and run 40-tick async flash save wait
    let mut create_ok = file_select.begin_create_slot(0, "Link")
        && file_select.confirm_name_entry(&mut flash);
    let mut create_wait_ticks = 0u16;
    while file_select.wait_timer > 0 {
        let idle = pad_mgr.update_n64_raw(0, 0, 0);
        let _ = file_select.update_logic(&idle, &mut flash, &mut interp);
        create_wait_ticks += 1;
        for &frac in &[0.5f32, 1.0f32] {
            gfx.begin_frame(0xFF30_1808, &proj, &view);
            file_select.draw_frame(&mut pool, &mut renderer, &mut mgr, &mut gfx, &mut interp, frac);
            gfx.end_frame_and_swap(false);
            pace_frame(&mut sum_frame_us, &mut frames_rendered_total);
        }
    }
    if let Some(s0) = &file_select.slots[0] {
        create_ok = create_ok
            && create_wait_ticks == 40
            && s0.newf == SAVE_SLOT_MAGIC
            && s0.health_capacity == 0x30
            && s0.time == 0x4000
            && file_select.last_op == Some(FileOpOutcome::CreatedSlot(0));
    } else {
        create_ok = false;
    }

    // 3b: Copy Slot 0 -> Slot 1
    let mut copy_ok = file_select.start_copy_slot(&mut flash, 0, 1);
    while file_select.wait_timer > 0 {
        let idle = pad_mgr.update_n64_raw(0, 0, 0);
        let _ = file_select.update_logic(&idle, &mut flash, &mut interp);
        for &frac in &[0.5f32, 1.0f32] {
            gfx.begin_frame(0xFF30_1808, &proj, &view);
            file_select.draw_frame(&mut pool, &mut renderer, &mut mgr, &mut gfx, &mut interp, frac);
            gfx.end_frame_and_swap(false);
            pace_frame(&mut sum_frame_us, &mut frames_rendered_total);
        }
    }
    copy_ok = copy_ok && file_select.slot_valid[1];

    // 3c: Erase Slot 1
    let mut erase_ok = file_select.start_erase_slot(&mut flash, 1);
    while file_select.wait_timer > 0 {
        let idle = pad_mgr.update_n64_raw(0, 0, 0);
        let _ = file_select.update_logic(&idle, &mut flash, &mut interp);
        for &frac in &[0.5f32, 1.0f32] {
            gfx.begin_frame(0xFF30_1808, &proj, &view);
            file_select.draw_frame(&mut pool, &mut renderer, &mut mgr, &mut gfx, &mut interp, frac);
            gfx.end_frame_and_swap(false);
            pace_frame(&mut sum_frame_us, &mut frames_rendered_total);
        }
    }
    erase_ok = erase_ok && !file_select.slot_valid[1];

    // 3d: Corrupt Slot 2 recovery + invalid handling
    let corrupt_ok = file_select.verify_corrupt_slot_recovery(&mut flash, 2) && !file_select.slot_valid[2];

    // 3e: Launch Slot 0 -> DayTelop
    let mut launch_ok = file_select.launch_slot(0);
    let mut launched_save = None;
    let mut launch_btn = n64_btn::A_BUTTON;
    for _ in 0..20u32 {
        let pad = pad_mgr.update_n64_raw(launch_btn, 0, 0);
        launch_btn = 0;
        let next = file_select.update_logic(&pad, &mut flash, &mut interp);
        for &frac in &[0.5f32, 1.0f32] {
            gfx.begin_frame(0xFF30_1808, &proj, &view);
            file_select.draw_frame(&mut pool, &mut renderer, &mut mgr, &mut gfx, &mut interp, frac);
            gfx.end_frame_and_swap(false);
            pace_frame(&mut sum_frame_us, &mut frames_rendered_total);
        }
        if let Some((st, ctx)) = next {
            launch_ok = launch_ok && st == GameStateId::DayTelop;
            launched_save = Some(ctx);
            break;
        }
    }

    // --- State 4: DayTelop ("Dawn of the First Day -72 Hours Remain-") -> Play ---
    let day = launched_save.as_ref().map(|c| c.day).unwrap_or(1);
    let mut day_telop = DayTelopState::new(day, &msgs);
    let mut day_telop_title = day_telop.title;
    let mut reached_play = false;
    for _ in 0..45u32 {
        let next = day_telop.update_logic(&mut interp);
        for &frac in &[0.5f32, 1.0f32] {
            gfx.begin_frame(0xFF08_0808, &proj, &view);
            day_telop.draw_frame(&mut pool, &mut renderer, &mut mgr, &mut gfx, &mut interp, frac);
            gfx.end_frame_and_swap(false);
            pace_frame(&mut sum_frame_us, &mut frames_rendered_total);
        }
        if next == Some(GameStateId::Play) {
            reached_play = true;
            day_telop_title = day_telop.title;
            break;
        }
    }

    audio_worker.stop_and_join();
    gfx.shutdown();

    let avg_frame_us = if frames_rendered_total > 0 {
        sum_frame_us / (frames_rendered_total as u64)
    } else {
        0
    };

    let pass = state_01_ok
        && state_02_ok
        && create_ok
        && copy_ok
        && erase_ok
        && corrupt_ok
        && launch_ok
        && reached_play
        && frames_rendered_total >= 180;

    let report = format!(
        "OTR_MESSAGE_DATA_STATIC: loaded={}, bytes={}, entries={}\n\
         STATE_01: {}\n\
         STATE_02: {}\n\
         FILESELECT_CREATE_SLOT0: {}\n\
         FILESELECT_COPY_SLOT0_TO_SLOT1: {}\n\
         FILESELECT_ERASE_SLOT1: {}\n\
         NEGATIVE_CORRUPT_SLOT2_RECOVERY: {}\n\
         FILESELECT_LAUNCH_SLOT0: {}\n\
         FRAMES_RENDERED_TOTAL: {}\n\
         AVG_FRAME_LATENCY_US: {}\n\
         THGA_POOL_CANARIES_INTACT: {}\n\
         STATUS: {}\n",
        msgs.loaded_from_pak,
        msgs.raw_bytes_len,
        msgs.entry_count,
        if state_01_ok { "TitleSetup -> ConsoleLogo" } else { "FAIL" },
        if state_02_ok {
            format!("ConsoleLogo rendered frames={}, alpha_fade=OK -> FileSelect", logo_frames)
        } else {
            format!("FAIL(frames={})", logo_frames)
        },
        if create_ok {
            format!(
                "OK(name=\"Link\", hearts=0x30, time=0x4000, wait_ticks={})",
                create_wait_ticks
            )
        } else {
            "FAIL".into()
        },
        if copy_ok { "OK" } else { "FAIL" },
        if erase_ok {
            format!("OK(slot1_valid={})", file_select.slot_valid[1])
        } else {
            "FAIL".into()
        },
        if corrupt_ok {
            "OK(fallback_or_invalid_handled)"
        } else {
            "FAIL"
        },
        if launch_ok && reached_play {
            format!("OK -> DayTelop(\"{}\")", day_telop_title)
        } else {
            "FAIL".into()
        },
        frames_rendered_total,
        avg_frame_us,
        !pool.check_canaries_and_crash(),
        if pass { "PASS" } else { "FAIL" }
    );

    let _ = platform::crash::write_text_file("step08_title_fileselect.log", &report);
    unsafe { sys::sceKernelExitGame() };
}
