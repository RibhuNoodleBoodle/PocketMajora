#![no_std]

extern crate alloc;

pub mod actors;
pub mod bgcheck;
pub mod camera;
pub mod clock;
pub mod graph;
pub mod hud;
pub mod interp;
pub mod padmgr;
pub mod play;
pub mod player;
pub mod save;
pub mod scene;
pub mod states;
pub mod thga;

#[cfg(test)]
mod tests {
    use super::*;
    use graph::{FRAME_40FPS_USEC, NoCatchUpPacer, us_to_n64_cpu_counter};
    use interp::FrameInterpolator;
    use padmgr::{PadMgr, n64_btn, psp_btn};
    use pmm_gfx::math::mat4_identity;
    use pmm_schema::GfxWord;
    use save::{FLASH_TOTAL_BYTES, MemFlashIo, SAVE_SLOT_MAGIC, SaveContext, SysFlashrom};
    use thga::{GFXPOOL_HEAD_MAGIC, GFXPOOL_TAIL_MAGIC, GfxPool, TwoHeadGfxArena};

    #[test]
    fn frame_interpolator_blends_midpoint_and_guards_teleports_flips_and_scene_cuts() {
        let mut interp = FrameInterpolator::new();
        let mut m0 = mat4_identity();
        m0[3][0] = 10.0;
        let mut m1 = mat4_identity();
        m1[3][0] = 20.0;

        interp.begin_tick(5, 1, 0, 1);
        interp.record(100, 1, &m0);
        interp.end_tick();

        interp.begin_tick(5, 1, 0, 1);
        interp.record(100, 1, &m1);
        interp.end_tick();

        let mid = interp.resolve(100, 1, &m1, 0.5);
        assert!((mid[3][0] - 15.0).abs() < 1e-4);

        // Negative: teleport > 1000 units must snap to curr
        let mut m_tele = m1;
        m_tele[3][0] = 2500.0;
        interp.begin_tick(5, 1, 0, 1);
        interp.record(100, 1, &m_tele);
        interp.end_tick();
        let snapped = interp.resolve(100, 1, &m_tele, 0.5);
        assert!((snapped[3][0] - 2500.0).abs() < 1e-4);

        // Negative: > 90 deg flip must snap to curr
        let mut m_flip = mat4_identity();
        m_flip[0][0] = -1.0;
        interp.begin_tick(5, 1, 0, 1);
        interp.record(100, 1, &m_flip);
        interp.end_tick();
        let snapped_flip = interp.resolve(100, 1, &m_flip, 0.5);
        assert!((snapped_flip[0][0] + 1.0).abs() < 1e-4);

        // Negative: scene cut must snap to curr
        interp.begin_tick(5, 2, 0, 1);
        interp.record(100, 1, &m0);
        interp.end_tick();
        let snapped_cut = interp.resolve(100, 1, &m0, 0.5);
        assert!((snapped_cut[3][0] - 10.0).abs() < 1e-4);
    }

    #[test]
    fn thga_arena_allocation_and_crash_detection() {
        let mut arena = TwoHeadGfxArena::new(4);
        assert!(!arena.is_crash());
        assert!(arena.push_cmd(GfxWord::new(1, 2)));
        assert!(arena.alloc_tail_words(2).is_some());
        assert!(!arena.is_crash());
        // Overflow tail into head
        assert!(arena.alloc_tail_words(2).is_none());
        assert!(arena.is_crash());

        let mut pool = GfxPool::new();
        assert_eq!(pool.head_magic, GFXPOOL_HEAD_MAGIC);
        assert_eq!(pool.tail_magic, GFXPOOL_TAIL_MAGIC);
        assert!(!pool.check_canaries_and_crash());
        pool.reset();
        assert!(!pool.check_canaries_and_crash());
    }

    #[test]
    fn padmgr_press_release_edges_and_deadzone() {
        let mut pad = PadMgr::new();
        let p1 = pad.update_from_psp(psp_btn::CROSS | psp_btn::LTRIGGER, 128, 128);
        assert_eq!(p1.cur_buttons, n64_btn::A_BUTTON | n64_btn::Z_TRIG);
        assert_eq!(p1.press_buttons, n64_btn::A_BUTTON | n64_btn::Z_TRIG);
        assert_eq!(p1.rel_buttons, 0);
        assert_eq!(p1.stick_x, 0);
        assert_eq!(p1.stick_y, 0);

        // Held Cross, release LTrigger, analog inside deadzone (135 -> +7 < 14 -> 0)
        let p2 = pad.update_from_psp(psp_btn::CROSS, 135, 120);
        assert_eq!(p2.press_buttons, 0);
        assert_eq!(p2.rel_buttons, n64_btn::Z_TRIG);
        assert_eq!(p2.stick_x, 0);
        assert_eq!(p2.stick_y, 0);

        // Full right + up (lx=255, ly=0)
        let p3 = pad.update_from_psp(0, 255, 0);
        assert!(p3.stick_x >= 75 && p3.stick_x <= 80);
        assert_eq!(p3.stick_y, 80);
    }

    #[test]
    fn save_context_checksum_and_corrupt_detection() {
        let ctx = SaveContext::new_quest("Link");
        let mut bytes = ctx.serialize_slot();
        assert_eq!(&bytes[0..6], &SAVE_SLOT_MAGIC);
        let decoded = SaveContext::deserialize_slot(&bytes).expect("valid checksum");
        assert_eq!(decoded.player_name, ctx.player_name);
        assert_eq!(decoded.time, 0x4000);

        // Corrupt a byte -> must fail checksum check
        bytes[26] ^= 0x01;
        assert!(SaveContext::deserialize_slot(&bytes).is_none());
    }

    #[test]
    fn sys_flashrom_async_contract_and_oob_negative() {
        let mut flash = SysFlashrom::new(MemFlashIo::default());
        assert_eq!(flash.is_busy(), -1);
        assert_eq!(flash.init_flash(), 0);
        assert_eq!(flash.is_busy(), 0);

        let ctx = SaveContext::new_quest("Link");
        assert_eq!(flash.write_save_slot(0, &ctx), 0);
        assert_eq!(flash.is_busy(), 1);
        assert_eq!(flash.await_result(), 0);
        assert_eq!(flash.is_busy(), 0);

        // Reload slot 0
        let reloaded = flash.read_save_slot(0).expect("reload slot 0");
        assert_eq!(reloaded.newf, SAVE_SLOT_MAGIC);
        assert_eq!(reloaded.health_capacity, 0x30);

        // Negative: out-of-range page write returns -1
        let page_buf = [0u8; 128];
        assert_eq!(flash.write_async(&page_buf, 1024, 1), -1);
        let _ = flash.await_result();
    }

    #[test]
    fn pacer_40fps_and_n64_counter_conversion() {
        let mut pacer = NoCatchUpPacer::new();
        assert_eq!(pacer.compute_sleep_usec(1_000_000, FRAME_40FPS_USEC), 0);
        // 10 ms into frame -> should wait 15,000 us
        assert_eq!(
            pacer.compute_sleep_usec(1_010_000, FRAME_40FPS_USEC),
            15_000
        );
        // Missed deadline (+100 ms later) -> must rebase with 0 catch-up sleep
        assert_eq!(pacer.compute_sleep_usec(1_150_000, FRAME_40FPS_USEC), 0);
        assert_eq!(us_to_n64_cpu_counter(1_000_000), 46_875_000);
        assert_eq!(FLASH_TOTAL_BYTES, 131_072);
    }

    #[test]
    fn frame_interpolator_scale_jump_guard() {
        let mut interp = FrameInterpolator::new();
        let m0 = mat4_identity();
        let mut m_scaled = mat4_identity();
        m_scaled[0][0] = 5.0; // 25x squared > 16x threshold
        interp.begin_tick(5, 1, 0, 1);
        interp.record(7, 1, &m0);
        interp.end_tick();
        interp.begin_tick(5, 1, 0, 1);
        interp.record(7, 1, &m_scaled);
        interp.end_tick();
        let res = interp.resolve(7, 1, &m_scaled, 0.5);
        assert!((res[0][0] - 5.0).abs() < 1e-4);
    }

    #[test]
    fn frame_interpolator_nan_guard() {
        let mut interp = FrameInterpolator::new();
        let m0 = mat4_identity();
        let mut m_nan = mat4_identity();
        m_nan[3][0] = f32::NAN;
        interp.begin_tick(5, 1, 0, 1);
        interp.record(8, 1, &m0);
        interp.end_tick();
        interp.begin_tick(5, 1, 0, 1);
        interp.record(8, 1, &m_nan);
        interp.end_tick();
        let res = interp.resolve(8, 1, &m_nan, 0.5);
        assert!(res[3][0].is_nan());
    }

    #[test]
    fn padmgr_triangle_c_buttons_and_select_jpad() {
        let mut pad = PadMgr::new();
        let p_c = pad.update_from_psp(psp_btn::TRIANGLE | psp_btn::LEFT, 128, 128);
        assert_ne!(p_c.cur_buttons & (n64_btn::U_CBUTTONS | n64_btn::L_CBUTTONS), 0);
        let p_j = pad.update_from_psp(psp_btn::SELECT | psp_btn::UP, 128, 128);
        assert_eq!(p_j.cur_buttons & n64_btn::U_JPAD, n64_btn::U_JPAD);
    }

    #[test]
    fn save_slot_backup_fallback_on_corrupt_primary() {
        let mut flash = SysFlashrom::new(MemFlashIo::default());
        flash.init_flash();
        let ctx = SaveContext::new_quest("Zelda");
        flash.write_save_slot(2, &ctx);
        let _ = flash.await_result();
        flash.corrupt_primary_slot_for_test(2);
        let recovered = flash.read_save_slot(2).expect("backup page fallback");
        assert_eq!(recovered.player_name, ctx.player_name);
    }

    #[test]
    fn thga_pool_canary_corruption_detection() {
        let mut pool = GfxPool::new();
        assert!(!pool.check_canaries_and_crash());
        pool.tail_magic = 0xDEAD;
        assert!(pool.check_canaries_and_crash());
    }

    #[test]
    fn game_state_id_names_and_pacer_wrap() {
        assert_eq!(graph::GameStateId::TitleSetup.name(), "TitleSetup");
        assert_eq!(graph::GameStateId::FileChoose.name(), "FileChoose");
        let mut pacer = NoCatchUpPacer::new();
        // Near u32::MAX wraparound
        assert_eq!(pacer.compute_sleep_usec(u32::MAX - 5_000, FRAME_40FPS_USEC), 0);
        let wait = pacer.compute_sleep_usec(u32::MAX - 1_000, FRAME_40FPS_USEC);
        assert_eq!(wait, 21_000);
    }

    fn build_test_pak_with_messages() -> alloc::vec::Vec<u8> {
        use pmm_schema::{PAK_ENTRY_SIZE, PAK_HEADER_SIZE, PmmPakEntry, PmmPakHeader, ResourceId, fourcc};
        let payload = b"Dawn of the First Day\xbf-72 Hours Remain-\xbfLink\xbf";
        let id = ResourceId::from_otr_path("__OTR__seg/message_data_static").0;
        let data_offset = ((PAK_HEADER_SIZE + PAK_ENTRY_SIZE + 15) / 16) * 16;
        let total_size = data_offset + payload.len();
        let hdr = PmmPakHeader::new(1, PAK_HEADER_SIZE as u32, data_offset as u32, 0x1234_5678, 0, total_size as u32);
        let entry = PmmPakEntry {
            id,
            fourcc: fourcc::OTXM,
            flags: 0,
            offset: data_offset as u32,
            size: payload.len() as u32,
            uncompressed_size: payload.len() as u32,
            align: 16,
        };
        let mut pak = alloc::vec![0u8; total_size];
        pak[..PAK_HEADER_SIZE].copy_from_slice(&hdr.to_le_bytes());
        pak[PAK_HEADER_SIZE..PAK_HEADER_SIZE + PAK_ENTRY_SIZE].copy_from_slice(&entry.to_le_bytes());
        pak[data_offset..data_offset + payload.len()].copy_from_slice(payload);
        pak
    }

    #[test]
    fn step08_title_setup_and_console_logo_fade_skip() {
        use pmm_assets::{BoundedArena, MemPakSource, ResourceManager};
        use pmm_gfx::{MockGfxBackend, Renderer};
        use states::{ConsoleLogoState, ScriptedPadInjector, TitleSetupState};

        let pak = build_test_pak_with_messages();
        let mut res = ResourceManager::open(MemPakSource::new(pak)).expect("open test pak");
        let mut mem = alloc::vec![0u8; 4096];
        let mut arena = BoundedArena::from_slice(&mut mem);
        let mut flash = SysFlashrom::new(MemFlashIo::default());

        let mut setup = TitleSetupState::new();
        let (save, msgs, next) = setup.update(&mut flash, &mut res, &mut arena);
        assert_eq!(next, graph::GameStateId::Title);
        assert_eq!(save.health_capacity, 0x30);
        assert!(msgs.loaded_from_pak);
        assert_eq!(msgs.entry_count, 3);

        let mut logo = ConsoleLogoState::new();
        let mut pad_mgr = PadMgr::new();
        let mut interp = FrameInterpolator::new();
        let mut pool = GfxPool::new();
        let mut renderer = Renderer::new();
        let mut backend = MockGfxBackend::default();
        let mut script = ScriptedPadInjector::new();
        // Hold off until cover_alpha hits 0 (~22 ticks), then press START at tick 24
        script.push_press(24, n64_btn::START_BUTTON);

        let mut transitioned = None;
        for _ in 0..80 {
            let (btn, sx, sy) = script.sample_tick();
            let pad = pad_mgr.update_n64_raw(btn, sx, sy);
            let res_next = logo.update_logic(&pad, &mut interp);
            logo.draw_frame(&mut pool, &mut renderer, &mut res, &mut backend, &mut interp, 0.5);
            logo.draw_frame(&mut pool, &mut renderer, &mut res, &mut backend, &mut interp, 1.0);
            if res_next.is_some() {
                transitioned = res_next;
                break;
            }
        }
        assert_eq!(transitioned, Some(graph::GameStateId::FileChoose));
        assert!(logo.fade_in_seen && logo.hold_seen && logo.fade_out_seen);
        assert!(renderer.stats.triangles_submitted >= 80);
    }

    #[test]
    fn step08_file_select_create_copy_erase_and_corrupt_recovery() {
        use states::{ConfigMode, FileOpOutcome, FileSelectState};

        let mut flash = SysFlashrom::new(MemFlashIo::default());
        let mut fs = FileSelectState::new(&mut flash);
        let mut pad_mgr = PadMgr::new();
        let mut interp = FrameInterpolator::new();

        // Create slot 0 as "Link" and run 40-tick async save wait
        assert!(fs.begin_create_slot(0, "Link"));
        assert_eq!(fs.config_mode, ConfigMode::NameEntry);
        assert!(fs.confirm_name_entry(&mut flash));
        assert_eq!(fs.config_mode, ConfigMode::NameEntryWaitForFlashSave);
        let mut idle = pad_mgr.update_n64_raw(0, 0, 0);
        for _ in 0..40 {
            fs.update_logic(&idle, &mut flash, &mut interp);
        }
        assert_eq!(fs.config_mode, ConfigMode::MainMenu);
        assert!(fs.slot_valid[0]);
        assert_eq!(fs.last_op, Some(FileOpOutcome::CreatedSlot(0)));
        assert_eq!(fs.slots[0].as_ref().unwrap().health_capacity, 0x30);
        assert_eq!(fs.slots[0].as_ref().unwrap().time, 0x4000);

        // Copy slot 0 -> slot 1
        assert!(fs.start_copy_slot(&mut flash, 0, 1));
        for _ in 0..40 {
            fs.update_logic(&idle, &mut flash, &mut interp);
        }
        assert!(fs.slot_valid[1]);
        assert_eq!(fs.last_op, Some(FileOpOutcome::CopiedSlot { src: 0, dst: 1 }));

        // Erase slot 1
        assert!(fs.start_erase_slot(&mut flash, 1));
        for _ in 0..40 {
            fs.update_logic(&idle, &mut flash, &mut interp);
        }
        assert!(!fs.slot_valid[1]);
        assert_eq!(fs.last_op, Some(FileOpOutcome::ErasedSlot(1)));

        // Corrupt slot 2 negative recovery
        assert!(fs.verify_corrupt_slot_recovery(&mut flash, 2));
        assert!(!fs.slot_valid[2]);

        // Launch slot 0 -> DayTelop
        assert!(fs.launch_slot(0));
        idle = pad_mgr.update_n64_raw(n64_btn::A_BUTTON, 0, 0);
        let mut launched = None;
        for _ in 0..16 {
            if let Some(res) = fs.update_logic(&idle, &mut flash, &mut interp) {
                launched = Some(res);
                break;
            }
            idle = pad_mgr.update_n64_raw(0, 0, 0);
        }
        let (next_state, ctx) = launched.expect("launched slot 0");
        assert_eq!(next_state, graph::GameStateId::DayTelop);
        assert_eq!(ctx.time, 0x4000);
    }

    #[test]
    fn step08_day_telop_renders_and_transitions_to_play() {
        use pmm_assets::{BoundedArena, MemPakSource, ResourceManager};
        use pmm_gfx::{MockGfxBackend, Renderer};
        use states::{DayTelopState, OTRMessageTable};

        let pak = build_test_pak_with_messages();
        let mut res = ResourceManager::open(MemPakSource::new(pak)).expect("open pak");
        let mut mem = alloc::vec![0u8; 4096];
        let mut arena = BoundedArena::from_slice(&mut mem);
        let msgs = OTRMessageTable::load(&mut res, &mut arena);

        let mut telop = DayTelopState::new(1, &msgs);
        assert_eq!(telop.title, "Dawn of the First Day");
        assert_eq!(telop.subtitle, "-72 Hours Remain-");

        let mut interp = FrameInterpolator::new();
        let mut pool = GfxPool::new();
        let mut renderer = Renderer::new();
        let mut backend = MockGfxBackend::default();

        let mut next = None;
        for _ in 0..40 {
            next = telop.update_logic(&mut interp);
            telop.draw_frame(&mut pool, &mut renderer, &mut res, &mut backend, &mut interp, 0.5);
            telop.draw_frame(&mut pool, &mut renderer, &mut res, &mut backend, &mut interp, 1.0);
        }
        assert_eq!(next, Some(graph::GameStateId::Play));
        assert!(renderer.stats.triangles_submitted >= 160);
    }

    #[test]
    fn step08_options_menu_and_cancel_name_entry() {
        use states::{ConfigMode, FileSelectState, MenuMode};

        let mut flash = SysFlashrom::new(MemFlashIo::default());
        let mut fs = FileSelectState::new(&mut flash);
        let mut pad_mgr = PadMgr::new();
        let mut interp = FrameInterpolator::new();

        // Advance out of Init into MainMenu
        let idle = pad_mgr.update_n64_raw(0, 0, 0);
        for _ in 0..12 {
            fs.update_logic(&idle, &mut flash, &mut interp);
        }
        assert_eq!(fs.menu_mode, MenuMode::Config);
        assert_eq!(fs.config_mode, ConfigMode::MainMenu);

        // Press R -> OptionsMenu, press A -> toggle Z-target hold, press B -> MainMenu
        let p_r = pad_mgr.update_n64_raw(n64_btn::R_TRIG, 0, 0);
        fs.update_logic(&p_r, &mut flash, &mut interp);
        assert_eq!(fs.config_mode, ConfigMode::OptionsMenu);
        let p_a = pad_mgr.update_n64_raw(n64_btn::A_BUTTON, 0, 0);
        fs.update_logic(&p_a, &mut flash, &mut interp);
        assert!(fs.options.z_target_hold);
        let p_b = pad_mgr.update_n64_raw(n64_btn::B_BUTTON, 0, 0);
        fs.update_logic(&p_b, &mut flash, &mut interp);
        assert_eq!(fs.config_mode, ConfigMode::MainMenu);
    }

    #[test]
    fn step09_bgcheck_floor_raycast_horizontal_slope_and_oob() {
        use bgcheck::{BGCHECK_Y_MIN, BgCheck, CollisionMesh};

        let mut mesh = CollisionMesh::default();
        mesh.add_floor_quad(-100.0, -100.0, 100.0, 0.0, 0.0, None);
        // Sloped triangle rising from y=0 at z=0 to y=40 at z=-80 (CCW upward normal)
        mesh.add_triangle([120.0, 0.0, 0.0], [200.0, 40.0, -80.0], [120.0, 40.0, -80.0], None);
        mesh.add_triangle([120.0, 0.0, 0.0], [200.0, 0.0, 0.0], [200.0, 40.0, -80.0], None);

        let (fy_flat, exit0) = BgCheck::entity_raycast_floor(&mesh, 0.0, 50.0, -50.0);
        assert!((fy_flat - 0.0).abs() < 1e-3);
        assert_eq!(exit0, None);

        let (fy_mid, _) = BgCheck::entity_raycast_floor(&mesh, 160.0, 100.0, -40.0);
        assert!((fy_mid - 20.0).abs() < 1e-3);

        let (fy_oob, _) = BgCheck::entity_raycast_floor(&mesh, 999.0, 50.0, 999.0);
        assert_eq!(fy_oob, BGCHECK_Y_MIN);
    }

    #[test]
    fn step09_bgcheck_wall_sphere_collision_and_exit_trigger() {
        use bgcheck::{BgCheck, CollisionMesh};

        let mut mesh = CollisionMesh::default();
        mesh.add_floor_quad(-50.0, -50.0, 50.0, 50.0, 0.0, None);
        mesh.add_floor_quad(-20.0, 50.0, 20.0, 90.0, 0.0, Some(3));
        mesh.add_wall_quad(50.0, -50.0, 50.0, 50.0, 0.0, 100.0);

        let mut pos = [45.0, 0.0, 0.0];
        let hits = BgCheck::entity_sph_vs_wall(&mesh, &mut pos, 18.0, 44.0);
        assert!(hits >= 1);
        assert!(pos[0] <= 32.1 || pos[0] >= 67.9);

        let (_fy, exit_idx) = BgCheck::entity_raycast_floor(&mesh, 0.0, 20.0, 70.0);
        assert_eq!(exit_idx, Some(3));
    }

    #[test]
    fn step09_three_day_clock_progression_inverted_double_and_reset() {
        use clock::{CLOCK_DAWN, CLOCK_NIGHT, ClockEvent, TIME_SPEED_INVERTED, TIME_SPEED_NORMAL, ThreeDayClock};

        let mut clk = ThreeDayClock::new(1, CLOCK_DAWN);
        assert_eq!(clk.r_time_speed, TIME_SPEED_NORMAL);
        clk.set_inverted_song(true);
        assert_eq!(clk.r_time_speed, TIME_SPEED_INVERTED);
        clk.set_inverted_song(false);

        assert_eq!(clk.song_of_double_time(), ClockEvent::NightStarted(1));
        assert_eq!(clk.time, CLOCK_NIGHT);
        assert_eq!(clk.song_of_double_time(), ClockEvent::DawnTransition(2));
        assert_eq!((clk.day, clk.time), (2, CLOCK_DAWN));
        assert_eq!(clk.song_of_double_time(), ClockEvent::NightStarted(2));
        assert_eq!(clk.song_of_double_time(), ClockEvent::DawnTransition(3));
        assert_eq!(clk.song_of_double_time(), ClockEvent::NightStarted(3));

        // Advance past midnight of Day 3 into Final Hours (< 0x4000)
        let ev_mid = clk.advance_units(0x4100);
        assert_eq!(ev_mid, ClockEvent::None);
        assert!(clk.is_final_hours());

        // Double Time on Day 3 final hours must NOT skip past Final Day 6:00 AM
        let ev_block = clk.song_of_double_time();
        assert_eq!(ev_block, ClockEvent::None);
        assert_eq!(clk.day, 3);

        // Normal clock ticks across Day 3 6:00 AM triggers Moon Crash at day=4
        let ev_crash = clk.advance_units(0x0200);
        assert_eq!(ev_crash, ClockEvent::MoonCrash);

        // Song of Time reset preserves banked rupees & Deku mask, resets consumables
        let mut save = SaveContext::new_quest("Link");
        save.set_bank_rupees(250);
        save.set_deku_mask(true);
        save.rupees = 99;
        save.magic = 48;
        clk.song_of_time_reset(&mut save);
        assert_eq!((clk.day, clk.time), (1, CLOCK_DAWN));
        assert_eq!(save.three_day_reset_count, 1);
        assert_eq!(save.bank_rupees(), 250);
        assert!(save.has_deku_mask());
        assert_eq!(save.rupees, 0);
        assert_eq!(save.magic, 0);
    }

    #[test]
    fn step09_play_state_scene_reload_zero_leak_deku_ztarget_and_owl_save() {
        use play::PlayState;
        use player::{OcarinaSong, PlayerForm};
        use pmm_assets::{BoundedArena, MemPakSource, ResourceManager};
        use pmm_gfx::{MockGfxBackend, Renderer, math::compute_letterbox_scissor};
        use scene::SceneId;

        let pak = build_test_pak_with_messages();
        let mut res = ResourceManager::open(MemPakSource::new(pak)).expect("open test pak");
        let mut mem = alloc::vec![0u8; 64 * 1024];
        let mut arena = BoundedArena::from_slice(&mut mem);
        let mut flash = SysFlashrom::new(MemFlashIo::default());
        flash.init_flash();

        let save = SaveContext::new_quest("Link");
        let mut play = PlayState::enter(SceneId::InsideTower, save, &mut arena, &mut res);
        let mut pad_mgr = PadMgr::new();
        let mut interp = FrameInterpolator::new();
        let mut pool = GfxPool::new();
        let mut renderer = Renderer::new();
        let mut backend = MockGfxBackend::default();

        // Walk forward from InsideTower doorway into SouthClockTown (exit 1 at z=140..190)
        for _ in 0..24 {
            let pad = pad_mgr.update_n64_raw(0, 0, -80);
            let ev = play.update_logic(&pad, OcarinaSong::None, &mut res, &mut arena, &mut flash, &mut interp);
            play.draw_frame(0.5, &mut pool, &mut renderer, &mut res, &mut backend, &mut interp);
            play.draw_frame(1.0, &mut pool, &mut renderer, &mut res, &mut backend, &mut interp);
            if ev.scene_transition == Some(SceneId::SouthClockTown) {
                break;
            }
        }
        assert_eq!(play.scene.id, SceneId::SouthClockTown);

        // Toggle Deku -> Human & Z-target letterbox to 24px
        let pad_toggle = pad_mgr.update_n64_raw(n64_btn::R_CBUTTONS | n64_btn::Z_TRIG, 0, 0);
        play.update_logic(&pad_toggle, OcarinaSong::None, &mut res, &mut arena, &mut flash, &mut interp);
        assert_eq!(play.player.form, PlayerForm::Human);
        for _ in 0..5 {
            let pad_z = pad_mgr.update_n64_raw(n64_btn::Z_TRIG, 0, 0);
            play.update_logic(&pad_z, OcarinaSong::None, &mut res, &mut arena, &mut flash, &mut interp);
        }
        assert_eq!(play.camera.letterbox_bar_px, 24);
        assert_eq!(play.camera.letterbox_gap_rows(), 0);
        let (mid, top, bot) = compute_letterbox_scissor(480, 272, 24);
        assert_eq!(top.y1, mid.y0);
        assert_eq!(mid.y1, bot.y0);

        // Transition out and back to SouthClockTown -> zero scene arena leak
        play.transition_to_scene(SceneId::InsideTower, &mut arena, &mut res);
        play.transition_to_scene(SceneId::SouthClockTown, &mut arena, &mut res);
        assert_eq!(play.scene_leak_bytes_max, 0);
    }
}
