#![no_std]

extern crate alloc;

pub mod graph;
pub mod interp;
pub mod padmgr;
pub mod save;
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
}
