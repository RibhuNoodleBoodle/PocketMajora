#![no_std]
#![no_main]

extern crate alloc;

mod platform;

use alloc::format;
use pmm_assets::{ResourceManager, SegmentTable};
use pmm_game::graph::{FRAME_40FPS_USEC, GameStateId, NoCatchUpPacer, us_to_n64_cpu_counter};
use pmm_game::interp::FrameInterpolator;
use pmm_game::padmgr::PadMgr;
use pmm_game::save::{FLASH_TOTAL_BYTES, SaveContext, SysFlashrom};
use pmm_game::thga::GfxPool;
use pmm_gfx::Renderer;
use pmm_gfx::math::mat4_identity;
use pmm_schema::{GfxWord, ResourceId};
use psp::input::Controller;
use psp::sys;

psp::module!("PocketMajora", 1, 0);

fn build_perspective(fovy_rad: f32, aspect: f32, near: f32, far: f32) -> [[f32; 4]; 4] {
    let half_tan = libm::tanf(fovy_rad * 0.5);
    let f = 1.0 / half_tan;
    let nf = 1.0 / (near - far);
    [
        [f / aspect, 0.0, 0.0, 0.0],
        [0.0, f, 0.0, 0.0],
        [0.0, 0.0, (far + near) * nf, -1.0],
        [0.0, 0.0, (2.0 * far * near) * nf, 0.0],
    ]
}

fn psp_main() {
    psp::callback::setup_exit_callback().unwrap();
    let _ = platform::clock::lock_max_clocks();

    let mut pools = platform::memory::PartitionPools::allocate().unwrap();
    let source = platform::pak_io::PspPakSource::open("data/pmm_2s2h.pak").unwrap();
    let mut mgr = ResourceManager::open(source).unwrap();

    let mut scene_arena = pools.scene_arena_mut();
    scene_arena.mark_scene();
    let (vtx_ptr, vtx_len) = mgr
        .load_by_name_into_arena("__OTR__scenes/test/room0_vtx", &mut scene_arena)
        .map(|(_, s)| (s.as_ptr(), s.len()))
        .unwrap();
    let (dl_ptr, dl_len) = mgr
        .load_by_name_into_arena("__OTR__scenes/test/room0_dl", &mut scene_arena)
        .map(|(_, s)| (s.as_ptr(), s.len()))
        .unwrap();

    let mut segs = SegmentTable::default();
    let dl_words = unsafe {
        segs.set_segment(0x06, core::slice::from_raw_parts(vtx_ptr, vtx_len));
        core::slice::from_raw_parts(
            dl_ptr as *const GfxWord,
            dl_len / core::mem::size_of::<GfxWord>(),
        )
    };

    let mut backend = unsafe {
        platform::gfx_psp::PspGfxBackend::new(
            pools.renderer_cache.as_mut_ptr(),
            platform::memory::POOL_RENDERER_BYTES,
        )
    };
    let mut renderer = Renderer::new();
    let mut tex_scratch = [0u8; 32 * 1024];
    let proj = build_perspective(core::f32::consts::FRAC_PI_3, 16.0 / 9.0, 1.0, 4000.0);
    let room_id = ResourceId::from_otr_path("__OTR__scenes/test/room0_dl");

    psp::input::enable_analog();
    let mut ctrl = Controller::new();
    let mut pad_mgr = PadMgr::new();
    let mut gfx_pool = GfxPool::new();
    let mut interp = FrameInterpolator::new();
    let mut pacer = NoCatchUpPacer::new();

    // Run 120 logic ticks (20 Hz) -> 240 presented frames (40 FPS @ 25,000 us)
    let mut state_id = GameStateId::TitleSetup;
    let mut logic_ticks = 0u32;
    let mut presented_frames = 0u32;
    let mut total_presented_lat_us = 0u64;
    let mut last_pass_end = unsafe { sys::sceKernelGetSystemTimeLow() };

    for tick in 0..120u32 {
        if tick == 1 {
            state_id = GameStateId::Title;
        } else if tick == 60 {
            state_id = GameStateId::FileChoose;
        }

        ctrl.update();
        let _ = pad_mgr.update_from_psp(
            ctrl.raw().buttons.bits(),
            ctrl.analog_x(),
            ctrl.analog_y(),
        );
        gfx_pool.reset();

        interp.begin_tick(state_id as u32, 0, 0, 1);
        let mut cam_mtx = mat4_identity();
        cam_mtx[3][0] = libm::sinf((tick as f32) * 0.08) * 35.0;
        cam_mtx[3][1] = -85.0;
        cam_mtx[3][2] = -340.0;
        interp.record(1, 0x10, &cam_mtx);
        interp.end_tick();

        // Two 40 FPS render passes per 20 Hz logic tick: Pass 0 (0.5) & Pass 1 (1.0)
        for pass in 0..2u32 {
            let fraction = if pass == 0 { 0.5 } else { 1.0 };
            let now = unsafe { sys::sceKernelGetSystemTimeLow() };
            let wait_us = pacer.compute_sleep_usec(now, FRAME_40FPS_USEC);
            if wait_us > 0 {
                unsafe { sys::sceKernelDelayThread(wait_us) };
            }

            let view = interp.resolve(1, 0x10, &cam_mtx, fraction);
            backend.begin_frame(0xFF20_1810, &proj, &view);
            renderer.begin_frame();
            let _ = renderer.process_static_display_list(
                room_id,
                dl_words,
                &segs,
                &mut mgr,
                &mut tex_scratch,
                &mut backend,
            );
            backend.end_frame_and_swap(false);

            let pass_end = unsafe { sys::sceKernelGetSystemTimeLow() };
            if presented_frames > 0 {
                total_presented_lat_us += pass_end.wrapping_sub(last_pass_end) as u64;
            }
            last_pass_end = pass_end;
            presented_frames += 1;
        }
        logic_ticks += 1;
    }

    let avg_frame_lat_us = total_presented_lat_us / (presented_frames.saturating_sub(1).max(1) as u64);

    // Teleport snap guard verification
    let mut tele_mtx = mat4_identity();
    tele_mtx[3][0] = 5000.0;
    interp.begin_tick(state_id as u32, 0, 0, 1);
    interp.record(1, 0x10, &tele_mtx);
    interp.end_tick();
    let tele_resolved = interp.resolve(1, 0x10, &tele_mtx, 0.5);
    let teleport_guard_ok = libm::fabsf(tele_resolved[3][0] - 5000.0) < 1e-3;

    // 128 KiB SysFlashrom async write + busy check + fresh reload test
    let mut flash = SysFlashrom::new(platform::flashrom_psp::PspFlashIo);
    flash.init_flash();
    let save_ctx = SaveContext::new_quest("Link");
    let _ = flash.write_save_slot(0, &save_ctx);
    let busy_before_collect = flash.is_busy();
    let await_res = flash.await_result();
    let busy_after_collect = flash.is_busy();

    // Negative out-of-bounds page test
    let oob_page = [0u8; 128];
    let oob_res = flash.write_async(&oob_page, 1024, 1);
    let _ = flash.await_result();

    // Reload via fresh SysFlashrom instance reading mm-psp-save.bin from ms0:
    let mut flash_reload = SysFlashrom::new(platform::flashrom_psp::PspFlashIo);
    flash_reload.init_flash();
    let reloaded = flash_reload.read_save_slot(0);
    let reload_ok = reloaded
        .as_ref()
        .is_some_and(|s| s.newf == *b"ZELDA3" && s.health_capacity == 0x30);

    let file_size = psp::io::File::open("mm-psp-save.bin", sys::IoOpenFlags::RD_ONLY)
        .and_then(|f| f.size())
        .unwrap_or(-1);

    let n64_cnt_1s = us_to_n64_cpu_counter(1_000_000);
    backend.shutdown();

    let pass = logic_ticks == 120
        && presented_frames == 240
        && interp.blends_performed > 0
        && teleport_guard_ok
        && !gfx_pool.check_canaries_and_crash()
        && busy_before_collect == 1
        && await_res == 0
        && busy_after_collect == 0
        && oob_res == -1
        && file_size == FLASH_TOTAL_BYTES as i64
        && reload_ok
        && n64_cnt_1s == 46_875_000;

    let status = if pass { "PASS" } else { "FAIL" };
    let tele_str = if teleport_guard_ok { "OK" } else { "FAIL" };
    let reload_str = if reload_ok { "OK(ZELDA3)" } else { "FAIL" };
    let oob_str = if oob_res == -1 {
        "EXPECTED_ERR"
    } else {
        "UNEXPECTED"
    };

    let report = format!(
        "STATUS: {status}\n\
         GRAPH_LOGIC_TICKS_20HZ: {logic_ticks}\n\
         PRESENTED_FRAMES_40FPS: {presented_frames}\n\
         AVG_FRAME_LATENCY_US: {avg_frame_lat_us}\n\
         INTERPOLATED_MATRIX_BLENDS: {}\n\
         TELEPORT_SNAP_GUARD_TEST: {tele_str}\n\
         GAMESTATE_TRANSITIONS: TitleSetup->Title->FileChoose\n\
         FLASHROM_WRITE_ASYNC_BUSY_BEFORE_COLLECT: {busy_before_collect}\n\
         FLASHROM_AWAIT_RESULT: {await_res}\n\
         FLASHROM_FILE_SIZE_BYTES: {file_size}\n\
         SAVE_SLOT0_RELOAD_MAGIC_AND_CHECKSUM: {reload_str}\n\
         NEGATIVE_FLASHROM_OUT_OF_BOUNDS_PAGE: {oob_str}\n",
        interp.blends_performed
    );

    let _ = platform::crash::write_text_file("step06_game.log", &report);
    unsafe { sys::sceKernelExitGame() };
}
