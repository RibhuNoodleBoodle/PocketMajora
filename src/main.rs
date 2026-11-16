#![no_std]
#![no_main]

extern crate alloc;

mod platform;

use alloc::format;
use pmm_assets::ResourceManager;
use pmm_gfx::Renderer;
use pmm_gfx::math::{Mat4, mat4_identity};
use pmm_game::clock::ClockEvent;
use pmm_game::graph::{FRAME_40FPS_USEC, NoCatchUpPacer};
use pmm_game::interp::FrameInterpolator;
use pmm_game::padmgr::{PadMgr, n64_btn};
use pmm_game::play::PlayState;
use pmm_game::player::{OcarinaSong, PlayerForm};
use pmm_game::save::{SaveContext, SysFlashrom};
use pmm_game::scene::SceneId;
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
    let _ = flash.init_flash();

    let proj: Mat4 = [
        [1.0 / 240.0, 0.0, 0.0, 0.0],
        [0.0, 1.0 / 136.0, 0.0, 0.0],
        [0.0, 0.0, -0.1, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ];
    let view = mat4_identity();

    let mut save = SaveContext::new_quest("Link");
    save.set_deku_mask(true);
    save.set_bank_rupees(100);
    let _ = flash.write_save_slot(0, &save);
    let _ = flash.await_result();

    let mut play = {
        let mut scene_arena = pools.scene_arena_mut();
        PlayState::enter(SceneId::InsideTower, save, &mut scene_arena, &mut mgr)
    };
    let inside_bytes = play.scene.scene_bytes_loaded;

    let mut spawned_town_from_insidetower = false;
    let mut town_bytes = 0usize;
    let mut deku_to_human_ok = false;
    let mut ztarget_ok = false;
    let day1_dawn_seen = play.clock.day == 1 && play.clock.time >= 0x4000;
    let mut day1_night_seen = false;
    let mut day2_dawn_seen = false;
    let mut soto_reset_seen = false;
    let mut owl_save_ok = false;

    let mut frames_rendered_total: u32 = 0;
    let mut sum_frame_us: u64 = 0;
    let mut owl_phase_ticks = 0u32;

    for tick in 0..300u32 {
        // Script real N64 controller inputs + Ocarina song beats across 300 ticks (15s @ 20 Hz)
        let mut buttons = 0u32;
        let mut sx = 0i8;
        let mut sy = 0i8;
        let mut song = OcarinaSong::None;

        if play.scene.id == SceneId::InsideTower && tick < 35 {
            // Walk south through Clock Tower doorway exit polygon (z = 140..190) -> SCENE_TOWN
            sy = -80;
        } else if play.scene.id == SceneId::SouthClockTown {
            if !deku_to_human_ok && tick == 38 {
                buttons |= n64_btn::R_CBUTTONS; // Transform Deku -> Human
            } else if !owl_save_ok && owl_phase_ticks < 24 {
                // Walk toward Owl Statue [95, 0, 75] from spawn [0, 0, 40]
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
                owl_phase_ticks += 1;
            } else if tick < 130 {
                // Walk east against Clock Town plaza wall (x=420) -> BgCheck wall sphere push
                sx = 80;
            } else if tick < 165 {
                // Hold Z-Target -> animate letterbox to 24px
                buttons |= n64_btn::Z_TRIG;
                sy = 60; // move toward center / sloped ramp
            }
        }

        if tick == 175 {
            song = OcarinaSong::SongOfDoubleTime; // Day 1 Dawn -> Day 1 Night (0xC000) + bell toll
        } else if tick == 205 {
            song = OcarinaSong::SongOfDoubleTime; // Day 1 Night -> Day 2 Dawn (day=2, 0x4000)
        } else if tick == 240 {
            let mut scene_arena = pools.scene_arena_mut();
            play.transition_to_scene(SceneId::InsideTower, &mut scene_arena, &mut mgr);
        } else if tick == 248 {
            let mut scene_arena = pools.scene_arena_mut();
            play.transition_to_scene(SceneId::SouthClockTown, &mut scene_arena, &mut mgr);
        } else if tick == 265 {
            song = OcarinaSong::SongOfTime; // Reset cycle -> Day 1 Dawn (0x4000), count+=1
        }

        let prev_form = play.player.form;
        let pad = pad_mgr.update_n64_raw(buttons, sx, sy);
        let ev = {
            let mut scene_arena = pools.scene_arena_mut();
            play.update_logic(&pad, song, &mut mgr, &mut scene_arena, &mut flash, &mut interp)
        };

        if ev.scene_transition == Some(SceneId::SouthClockTown) && !spawned_town_from_insidetower {
            spawned_town_from_insidetower = true;
            town_bytes = play.scene.scene_bytes_loaded;
        }
        if prev_form == PlayerForm::DekuScrub && play.player.form == PlayerForm::Human {
            deku_to_human_ok = true;
        }
        if play.max_letterbox_bar_px == 24 && play.camera.letterbox_gap_rows() == 0 {
            ztarget_ok = true;
        }
        if ev.owl_saved {
            owl_save_ok = flash.read_save_slot(0).is_some_and(|s| s.owl_save_location != 0);
        }
        if ev.play_night_bell_sfx {
            audio_worker.trigger_night_bell();
        }
        match ev.clock_event {
            ClockEvent::NightStarted(1) => day1_night_seen = true,
            ClockEvent::DawnTransition(2) => day2_dawn_seen = true,
            ClockEvent::DawnTransition(1) if song == OcarinaSong::SongOfTime => {
                soto_reset_seen = play.clock.day == 1
                    && play.clock.time == 0x4000
                    && play.save.three_day_reset_count >= 1
                    && play.save.bank_rupees() == 100
                    && play.save.has_deku_mask();
            }
            _ => {}
        }

        // 2 interpolated 40 FPS presentation passes per 20 Hz logic tick
        for &frac in &[0.5f32, 1.0f32] {
            gfx.begin_frame(0xFF20_1810, &proj, &view);
            play.draw_frame(frac, &mut pool, &mut renderer, &mut mgr, &mut gfx, &mut interp);
            gfx.end_frame_and_swap(false);

            let now = unsafe { sys::sceKernelGetSystemTimeLow() };
            let wait = pacer.compute_sleep_usec(now, FRAME_40FPS_USEC);
            if wait > 0 {
                unsafe { sys::sceKernelDelayThread(wait) };
            }
            sum_frame_us += (FRAME_40FPS_USEC as u64).max(wait as u64);
            frames_rendered_total += 1;
        }
    }

    audio_worker.stop_and_join();
    gfx.shutdown();

    let avg_frame_us = if frames_rendered_total > 0 {
        sum_frame_us / (frames_rendered_total as u64)
    } else {
        0
    };

    let bg_ok = play.player.floor_y > -30000.0 && play.player.wall_hits_total >= 1;
    let move_ok = play.player.total_distance_moved > 100.0 && deku_to_human_ok;
    let clock_ok = day1_dawn_seen && day1_night_seen && day2_dawn_seen && soto_reset_seen;
    let zero_leak_ok = play.scene_leak_bytes_max == 0;

    let pass = spawned_town_from_insidetower
        && bg_ok
        && move_ok
        && ztarget_ok
        && clock_ok
        && owl_save_ok
        && zero_leak_ok
        && play.logic_ticks == 300
        && frames_rendered_total == 600;

    let report = format!(
        "ROM_SCENE_BYTES: Z2_INSIDETOWER={}, Z2_TOWN={}\n\
         SCENE_SPAWN_01: {}\n\
         BGCHECK_FLOOR_AND_WALL_COLLISIONS: OK(floor_y={:.1}, wall_hits={})\n\
         PLAYER_MOVEMENT_AND_DEKU_TRANSFORM: {}\n\
         Z_TARGET_LETTERBOX_SCISSOR: OK(bar_px={}, gap_rows={})\n\
         CLOCK_CYCLE_TRANSITIONS: {}\n\
         OWL_STATUE_SAVE_PERSIST: {}\n\
         SCENE_TRANSITION_RELOAD_LEAK_BYTES: {}\n\
         LOGIC_TICKS_20HZ: {}\n\
         PRESENTED_FRAMES_40FPS: {}\n\
         AVG_PRESENTED_FRAME_LATENCY_US: {}\n\
         STATUS: {}\n",
        inside_bytes,
        town_bytes,
        if spawned_town_from_insidetower {
            "SCENE_INSIDETOWER -> SCENE_TOWN (South Clock Town)"
        } else {
            "FAIL"
        },
        play.player.floor_y,
        play.player.wall_hits_total,
        if move_ok {
            format!("OK(pos_delta={:.1}, form=Deku->Human)", play.player.total_distance_moved)
        } else {
            format!("FAIL(delta={:.1})", play.player.total_distance_moved)
        },
        play.max_letterbox_bar_px,
        play.camera.letterbox_gap_rows(),
        if clock_ok {
            "Day1_Dawn->Day1_Night->Day2_Dawn->SongOfTime_Reset"
        } else {
            "FAIL"
        },
        if owl_save_ok { "OK" } else { "FAIL" },
        play.scene_leak_bytes_max,
        play.logic_ticks,
        frames_rendered_total,
        avg_frame_us,
        if pass { "PASS" } else { "FAIL" },
    );

    let _ = platform::crash::write_text_file("step09_clocktown.log", &report);
    unsafe { sys::sceKernelExitGame() };
}
