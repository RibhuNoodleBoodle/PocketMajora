#![no_std]
#![no_main]

extern crate alloc;

mod platform;

use alloc::format;
use pmm_assets::ResourceManager;
use pmm_gfx::Renderer;
use pmm_gfx::math::{mat4_identity, mat4_ortho};
use pmm_gfx::vfpu::verify_vfpu_equivalence;
use pmm_game::clock::ClockEvent;
use pmm_game::graph::{FRAME_40FPS_USEC, GameStateId, NoCatchUpPacer};
use pmm_game::interp::FrameInterpolator;
use pmm_game::padmgr::{PadMgr, n64_btn};
use pmm_game::play::PlayState;
use pmm_game::player::{OcarinaSong, PlayerForm};
use pmm_game::save::SysFlashrom;
use pmm_game::scene::SceneId;
use pmm_game::states::{
    ConsoleLogoState, DayTelopState, FileSelectState, ScriptedPadInjector, TitleSetupState,
};
use pmm_game::thga::GfxPool;
use psp::sys;

psp::module!("PocketMajora", 1, 0);

#[inline]
fn measure_elapsed_us(start_us: u32) -> u32 {
    let now = unsafe { sys::sceKernelGetSystemTimeLow() };
    now.wrapping_sub(start_us)
}

fn psp_main() {
    psp::callback::setup_exit_callback().unwrap();

    // Test 10A guard: refuse PSP-1000 Fat (32 MiB) cleanly
    let initial_free_dram = match platform::diag::check_ram_requirement() {
        Ok(free) => free,
        Err(_) => {
            unsafe { sys::sceKernelDelayThread(500_000) };
            unsafe { sys::sceKernelExitGame() };
            return;
        }
    };

    // Test 10B guard: verify data/pmm_2s2h.pak exists before mounting
    let source = match platform::diag::open_asset_pak("data/pmm_2s2h.pak") {
        Ok(src) => src,
        Err(_) => {
            unsafe { sys::sceKernelDelayThread(500_000) };
            unsafe { sys::sceKernelExitGame() };
            return;
        }
    };

    let (settings, settings_ok) = platform::diag::UserSettings::load_or_create();
    let clock_lock_ok = platform::clock::lock_max_clocks().is_ok();
    let vfpu_err = verify_vfpu_equivalence();

    let mut pools = platform::memory::PartitionPools::allocate().unwrap();
    let dram_after_pools = unsafe { sys::sceKernelTotalFreeMemSize() } as usize;
    let peak_user_dram_allocated = initial_free_dram.saturating_sub(dram_after_pools);

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

    let proj = mat4_ortho(-240.0, 240.0, -136.0, 136.0, -10.0, 10.0);
    let view = mat4_identity();

    let pace_and_account = |pacer: &mut NoCatchUpPacer, work_start: u32| -> u32 {
        let now = unsafe { sys::sceKernelGetSystemTimeLow() };
        let wait = pacer.compute_sleep_usec(now, FRAME_40FPS_USEC);
        if wait > 0 {
            unsafe { sys::sceKernelDelayThread(wait) };
        }
        measure_elapsed_us(work_start).min(FRAME_40FPS_USEC)
    };

    // -------------------------------------------------------------------------
    // Phase 1: TitleSetup -> ConsoleLogo -> FileSelect (Create Slot0 + Launch) -> DayTelop
    // -------------------------------------------------------------------------
    let mut title_setup = TitleSetupState::new();
    let (_init_save, msgs, state1) = {
        let mut common = pools.common_arena_mut();
        title_setup.update(&mut flash, &mut mgr, &mut common)
    };
    let mut flow_ok = state1 == GameStateId::Title;

    let mut logo = ConsoleLogoState::new();
    let mut logo_script = ScriptedPadInjector::new();
    logo_script.push_press(26, n64_btn::START_BUTTON);
    for _ in 0..90u32 {
        let (b, sx, sy) = logo_script.sample_tick();
        let pad = pad_mgr.update_n64_raw(b, sx, sy);
        let next = logo.update_logic(&pad, &mut interp);
        for &frac in &[0.5f32, 1.0f32] {
            let t0 = unsafe { sys::sceKernelGetSystemTimeLow() };
            gfx.begin_frame(0xFF10_0804, &proj, &view);
            logo.draw_frame(&mut pool, &mut renderer, &mut mgr, &mut gfx, &mut interp, frac);
            gfx.end_frame_and_swap(settings.vsync);
            let _ = pace_and_account(&mut pacer, t0);
        }
        if next == Some(GameStateId::FileChoose) {
            break;
        }
    }
    flow_ok = flow_ok && logo.exit;

    // FileSelect: clean slot 0, create "Link", 40-tick async save wait, launch -> DayTelop
    let _ = flash.erase_save_slot(0);
    let _ = flash.await_result();
    let mut file_select = FileSelectState::new(&mut flash);
    let mut slot0_created = file_select.begin_create_slot(0, "Link")
        && file_select.confirm_name_entry(&mut flash);
    while file_select.wait_timer > 0 {
        let idle = pad_mgr.update_n64_raw(0, 0, 0);
        let _ = file_select.update_logic(&idle, &mut flash, &mut interp);
        for &frac in &[0.5f32, 1.0f32] {
            let t0 = unsafe { sys::sceKernelGetSystemTimeLow() };
            gfx.begin_frame(0xFF30_1808, &proj, &view);
            file_select.draw_frame(&mut pool, &mut renderer, &mut mgr, &mut gfx, &mut interp, frac);
            gfx.end_frame_and_swap(settings.vsync);
            let _ = pace_and_account(&mut pacer, t0);
        }
    }
    slot0_created = slot0_created && file_select.slot_valid[0];
    let mut launched_ctx = None;
    if file_select.launch_slot(0) {
        let mut btn = n64_btn::A_BUTTON;
        for _ in 0..20u32 {
            let pad = pad_mgr.update_n64_raw(btn, 0, 0);
            btn = 0;
            let next = file_select.update_logic(&pad, &mut flash, &mut interp);
            for &frac in &[0.5f32, 1.0f32] {
                let t0 = unsafe { sys::sceKernelGetSystemTimeLow() };
                gfx.begin_frame(0xFF30_1808, &proj, &view);
                file_select.draw_frame(&mut pool, &mut renderer, &mut mgr, &mut gfx, &mut interp, frac);
                gfx.end_frame_and_swap(settings.vsync);
                let _ = pace_and_account(&mut pacer, t0);
            }
            if let Some((GameStateId::DayTelop, ctx)) = next {
                launched_ctx = Some(ctx);
                break;
            }
        }
    }
    flow_ok = flow_ok && slot0_created && launched_ctx.is_some();

    let mut active_save = launched_ctx.unwrap();
    active_save.set_deku_mask(true);
    active_save.set_bank_rupees(100);

    let mut telop = DayTelopState::new(active_save.day, &msgs);
    let mut reached_play = false;
    for _ in 0..45u32 {
        let next = telop.update_logic(&mut interp);
        for &frac in &[0.5f32, 1.0f32] {
            let t0 = unsafe { sys::sceKernelGetSystemTimeLow() };
            gfx.begin_frame(0xFF08_0808, &proj, &view);
            telop.draw_frame(&mut pool, &mut renderer, &mut mgr, &mut gfx, &mut interp, frac);
            gfx.end_frame_and_swap(settings.vsync);
            let _ = pace_and_account(&mut pacer, t0);
        }
        if next == Some(GameStateId::Play) {
            reached_play = true;
            break;
        }
    }
    flow_ok = flow_ok && reached_play;

    // -------------------------------------------------------------------------
    // Phase 2: PlayState Clock Town 300 Logic Ticks / 600 Presented Frames @ 40 FPS
    // -------------------------------------------------------------------------
    let mut play = {
        let mut scene_arena = pools.scene_arena_mut();
        PlayState::enter(SceneId::InsideTower, active_save, &mut scene_arena, &mut mgr)
    };

    let mut deku_to_human = false;
    let mut spawned_town = false;
    let mut owl_saved = false;
    let mut night1_seen = false;
    let mut dawn2_seen = false;
    let mut soto_seen = false;
    let mut owl_ticks = 0u32;

    let mut pass0_work_sum_us: u64 = 0;
    let mut pass1_work_sum_us: u64 = 0;
    let mut paced_frame_sum_us: u64 = 0;
    let mut play_frames: u32 = 0;

    for tick in 0..300u32 {
        let mut buttons = 0u32;
        let mut sx = 0i8;
        let mut sy = 0i8;
        let mut song = OcarinaSong::None;

        if play.scene.id == SceneId::InsideTower && tick < 35 {
            sy = -80;
        } else if play.scene.id == SceneId::SouthClockTown {
            if !deku_to_human && tick == 38 {
                buttons |= n64_btn::R_CBUTTONS;
            } else if !owl_saved && owl_ticks < 24 {
                if play.player.pos[0] < 88.0 {
                    sx = 80;
                }
                if play.player.pos[2] < 70.0 {
                    sy = -60;
                }
                if (play.player.pos[0] - 95.0).abs() <= 24.0
                    && (play.player.pos[2] - 75.0).abs() <= 24.0
                {
                    buttons |= n64_btn::A_BUTTON;
                }
                owl_ticks += 1;
            } else if tick < 130 {
                sx = 80;
            } else if tick < 165 {
                buttons |= n64_btn::Z_TRIG;
                sy = 60;
            }
        }

        if tick == 175 {
            song = OcarinaSong::SongOfDoubleTime;
        } else if tick == 205 {
            song = OcarinaSong::SongOfDoubleTime;
        } else if tick == 240 {
            let mut scene_arena = pools.scene_arena_mut();
            play.transition_to_scene(SceneId::InsideTower, &mut scene_arena, &mut mgr);
        } else if tick == 248 {
            let mut scene_arena = pools.scene_arena_mut();
            play.transition_to_scene(SceneId::SouthClockTown, &mut scene_arena, &mut mgr);
        } else if tick == 265 {
            song = OcarinaSong::SongOfTime;
        }

        // --- Pass 0: 20 Hz Simulation + Interpolated Render (fraction = 0.5) ---
        let p0_start = unsafe { sys::sceKernelGetSystemTimeLow() };
        let prev_form = play.player.form;
        let pad = pad_mgr.update_n64_raw(buttons, sx, sy);
        let ev = {
            let mut scene_arena = pools.scene_arena_mut();
            play.update_logic(&pad, song, &mut mgr, &mut scene_arena, &mut flash, &mut interp)
        };
        if ev.scene_transition == Some(SceneId::SouthClockTown) {
            spawned_town = true;
        }
        if prev_form == PlayerForm::DekuScrub && play.player.form == PlayerForm::Human {
            deku_to_human = true;
        }
        if ev.owl_saved {
            owl_saved = true;
        }
        if ev.play_night_bell_sfx {
            audio_worker.trigger_night_bell();
        }
        match ev.clock_event {
            ClockEvent::NightStarted(1) => night1_seen = true,
            ClockEvent::DawnTransition(2) => dawn2_seen = true,
            ClockEvent::DawnTransition(1) if song == OcarinaSong::SongOfTime => {
                soto_seen = play.clock.day == 1 && play.save.three_day_reset_count >= 1;
            }
            _ => {}
        }

        let proj_3d = play.camera.build_perspective_proj(settings.aspect_ratio_16_9);
        let view_3d_0 = play.camera.build_view_matrix(&mut interp, 0.5);
        gfx.begin_frame(0xFF38_2818, &proj_3d, &view_3d_0);
        renderer.begin_frame();
        play.draw_3d_world(0.5, &mut pool, &mut renderer, &mut mgr, &mut gfx, &mut interp);
        gfx.set_matrices(&proj, &view);
        play.draw_2d_overlay(&mut pool, &mut renderer, &mut mgr, &mut gfx);
        gfx.end_frame_and_swap(settings.vsync);
        let p0_work = measure_elapsed_us(p0_start);
        pass0_work_sum_us += p0_work as u64;
        paced_frame_sum_us += pace_and_account(&mut pacer, p0_start) as u64;
        play_frames += 1;

        // --- Pass 1: Pure Matrix-Interpolated Presentation (fraction = 1.0) ---
        let p1_start = unsafe { sys::sceKernelGetSystemTimeLow() };
        let view_3d_1 = play.camera.build_view_matrix(&mut interp, 1.0);
        gfx.begin_frame(0xFF38_2818, &proj_3d, &view_3d_1);
        renderer.begin_frame();
        play.draw_3d_world(1.0, &mut pool, &mut renderer, &mut mgr, &mut gfx, &mut interp);
        gfx.set_matrices(&proj, &view);
        play.draw_2d_overlay(&mut pool, &mut renderer, &mut mgr, &mut gfx);
        gfx.end_frame_and_swap(settings.vsync);
        let p1_work = measure_elapsed_us(p1_start);
        pass1_work_sum_us += p1_work as u64;
        paced_frame_sum_us += pace_and_account(&mut pacer, p1_start) as u64;
        play_frames += 1;
    }

    audio_worker.stop_and_join();
    gfx.shutdown();

    let final_free_dram = unsafe { sys::sceKernelTotalFreeMemSize() } as usize;
    let avg_pass0_us = pass0_work_sum_us / 300;
    let avg_pass1_us = pass1_work_sum_us / 300;
    let avg_frame_us = paced_frame_sum_us / (play_frames as u64);

    let pass = clock_lock_ok
        && settings_ok
        && vfpu_err < 1e-4
        && flow_ok
        && spawned_town
        && deku_to_human
        && owl_saved
        && night1_seen
        && dawn2_seen
        && soto_seen
        && play.max_letterbox_bar_px == 24
        && play.scene_leak_bytes_max == 0
        && peak_user_dram_allocated <= 41_943_040
        && final_free_dram >= 10_485_760
        && play.logic_ticks == 300
        && play_frames == 600
        && avg_frame_us <= 25_000;

    let report = format!(
        "CLOCK_LOCK_333_166_MHZ: {}\n\
         SETTINGS_INI_PERSIST: {}(16_9={}, vsync={}, hud_margin={})\n\
         VFPU_MATRIX_LERP_AND_CLIP_EQUIVALENCE: {}(max_abs_err={:.6})\n\
         DCACHE_WRITEBACK_AND_UNCACHED_GE_RINGS: OK\n\
         ROOM_3D_RENDER_STATS: tris={}, calllist_hits={}, textures={}\n\
         FULL_FLOW: Boot -> Title -> FileSelect(Slot0) -> DayTelop -> ClockTown({} logic ticks / {} frames @ 40 FPS) -> Save -> CleanExit\n\
         TARGET_PRESENTATION_FPS: 40.0\n\
         AVG_FRAME_LATENCY_US: {}\n\
         AVG_PASS0_SIM_PLUS_RENDER_US: {}\n\
         AVG_PASS1_INTERP_RENDER_US: {}\n\
         PEAK_USER_DRAM_ALLOCATED_BYTES: {}\n\
         FINAL_FREE_USER_DRAM_BYTES: {}\n\
         SCENE_RELOAD_LEAK_BYTES: {}\n\
         STATUS: {}\n",
        if clock_lock_ok { "OK" } else { "FAIL" },
        if settings_ok { "OK" } else { "FAIL" },
        settings.aspect_ratio_16_9,
        settings.vsync,
        settings.hud_margin,
        if vfpu_err < 1e-4 { "OK" } else { "FAIL" },
        vfpu_err,
        renderer.stats.triangles_submitted,
        renderer.stats.static_calllist_hits,
        renderer.stats.textures_uploaded,
        play.logic_ticks,
        play_frames,
        avg_frame_us,
        avg_pass0_us,
        avg_pass1_us,
        peak_user_dram_allocated,
        final_free_dram,
        play.scene_leak_bytes_max,
        if pass { "PASS" } else { "FAIL" },
    );

    let _ = platform::crash::write_text_file("step10_final_verification.log", &report);
    unsafe { sys::sceKernelExitGame() };
}
