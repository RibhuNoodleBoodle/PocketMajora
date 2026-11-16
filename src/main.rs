#![no_std]
#![no_main]

extern crate alloc;

mod platform;

use alloc::format;
use pmm_assets::{ResourceManager, SegmentTable};
use pmm_gfx::math::{mat4_identity, mat4_lerp_scalar, mat4_lerp_vfpu, mat4_mul_scalar, mat4_mul_vfpu};
use pmm_gfx::{GfxError, Renderer};
use pmm_schema::{GfxWord, ResourceId, gbi};
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

    // Initialize hardware GU backend on Pool 3 (`RendererCache` 6 MiB)
    let mut backend = unsafe {
        platform::gfx_psp::PspGfxBackend::new(
            pools.renderer_cache.as_mut_ptr(),
            platform::memory::POOL_RENDERER_BYTES,
        )
    };
    let vram_total = backend.vram_layout().total_bytes;

    // Verify VFPU vs Scalar equivalence right on hardware/PPSSPP
    let mut m_a = mat4_identity();
    m_a[3][0] = 12.5;
    m_a[3][1] = -3.25;
    m_a[3][2] = 4.0;
    let mut m_b = mat4_identity();
    m_b[0][0] = 2.0;
    m_b[1][1] = 0.5;
    m_b[3][2] = -10.0;
    let vfpu_mul = mat4_mul_vfpu(&m_a, &m_b);
    let scal_mul = mat4_mul_scalar(&m_a, &m_b);
    let vfpu_lerp = mat4_lerp_vfpu(&m_a, &m_b, 0.5);
    let scal_lerp = mat4_lerp_scalar(&m_a, &m_b, 0.5);
    let mut max_err = 0.0f32;
    for r in 0..4 {
        for c in 0..4 {
            let e1 = libm::fabsf(vfpu_mul[r][c] - scal_mul[r][c]);
            let e2 = libm::fabsf(vfpu_lerp[r][c] - scal_lerp[r][c]);
            if e1 > max_err {
                max_err = e1;
            }
            if e2 > max_err {
                max_err = e2;
            }
        }
    }
    let vfpu_eq_ok = max_err < 1e-4;

    // Upload a second texture from pak so TEXTURES_UPLOADED_AND_CACHED >= 2
    let mut tex_scratch = [0u8; 32 * 1024];
    let grad_dl = [
        GfxWord::new(
            ((gbi::G_SETIMG as u32) << 24) | 0x0018_001F,
            (ResourceId::from_otr_path("__OTR__textures/test/gradient32").0 & 0xFFFF_FFFF) as u32,
        ),
        GfxWord::new(
            (gbi::G_SETTILE as u32) << 24,
            (ResourceId::from_otr_path("__OTR__textures/test/gradient32").0 >> 32) as u32,
        ),
        GfxWord::new((gbi::G_SETTILESIZE as u32) << 24, (31 << 14) | (31 << 2)),
        GfxWord::new((gbi::G_SETOTHERMODE_L as u32) << 24, 0),
        GfxWord::new((gbi::G_SETOTHERMODE_L as u32) << 24, 0), // intentional redundant bind check
        GfxWord::new((gbi::G_ENDDL as u32) << 24, 0),
    ];

    let mut renderer = Renderer::new();
    let proj = build_perspective(core::f32::consts::FRAC_PI_3, 16.0 / 9.0, 1.0, 4000.0);
    let room_id = ResourceId::from_otr_path("__OTR__scenes/test/room0_dl");

    let t_start = unsafe { sys::sceKernelGetSystemTimeLow() };
    let mut frames_rendered = 0u32;
    for frame in 0..180u32 {
        let angle = (frame as f32) * 0.02;
        let mut view = mat4_identity();
        view[3][0] = libm::sinf(angle) * 40.0;
        view[3][1] = -90.0;
        view[3][2] = -360.0;

        backend.begin_frame(0xFF28_1810, &proj, &view);
        renderer.begin_frame();
        if frame == 0 {
            let _ = renderer.process_display_list(
                &grad_dl,
                &segs,
                &mut mgr,
                &mut tex_scratch,
                &mut backend,
                0,
            );
        }
        let _ = renderer.process_static_display_list(
            room_id,
            dl_words,
            &segs,
            &mut mgr,
            &mut tex_scratch,
            &mut backend,
        );
        if (frame % 30) == 0 {
            renderer.capture_motion_blur(&mut backend);
        }
        backend.end_frame_and_swap(false);
        frames_rendered += 1;
    }
    let t_end = unsafe { sys::sceKernelGetSystemTimeLow() };
    let avg_cpu_us = t_end.wrapping_sub(t_start) / frames_rendered.max(1);

    // Negative test: recursive display list depth > 18
    let self_dl = [GfxWord::new((gbi::G_DL as u32) << 24, 0x0E00_0000)];
    let self_bytes = unsafe {
        core::slice::from_raw_parts(
            self_dl.as_ptr() as *const u8,
            core::mem::size_of_val(&self_dl),
        )
    };
    segs.set_segment(0x0E, self_bytes);
    let depth_err_caught = matches!(
        renderer.process_display_list(&self_dl, &segs, &mut mgr, &mut tex_scratch, &mut backend, 0),
        Err(GfxError::DisplayListDepthExceeded)
    );

    backend.shutdown();

    let st = renderer.stats;
    let pass = vram_total == 2_097_152
        && frames_rendered == 180
        && avg_cpu_us <= 12_000
        && st.commands_decoded > 1000
        && st.triangles_submitted > 500
        && st.textures_uploaded >= 2
        && st.static_calllist_hits >= 170
        && st.redundant_binds_suppressed > 0
        && vfpu_eq_ok
        && st.motion_blur_passes > 0
        && depth_err_caught;

    let status = if pass { "PASS" } else { "FAIL" };
    let vfpu_str = if vfpu_eq_ok { "OK" } else { "FAIL" };
    let neg_str = if depth_err_caught {
        "EXPECTED_ERR_CAUGHT"
    } else {
        "UNEXPECTED"
    };

    let report = format!(
        "STATUS: {status}\n\
         VRAM_LAYOUT_TOTAL_BYTES: {vram_total}\n\
         FRAMES_RENDERED: {frames_rendered}\n\
         AVG_RENDER_PASS_CPU_US: {avg_cpu_us}\n\
         TOTAL_GBI_COMMANDS_DECODED: {}\n\
         TOTAL_TRIANGLES_SUBMITTED: {}\n\
         TEXTURES_UPLOADED_AND_CACHED: {}\n\
         STATIC_MESH_CALLLIST_HITS: {}\n\
         REDUNDANT_STATE_BINDS_SUPPRESSED: {}\n\
         VFPU_MATRIX_LERP_EQUIVALENCE: {vfpu_str}\n\
         MOTION_BLUR_GPU_COPYIMAGE_PASSES: {}\n\
         NEGATIVE_DL_DEPTH_OVERFLOW_GUARD: {neg_str}\n",
        st.commands_decoded,
        st.triangles_submitted,
        st.textures_uploaded,
        st.static_calllist_hits,
        st.redundant_binds_suppressed,
        st.motion_blur_passes,
    );

    let _ = platform::crash::write_text_file("step05_gfx.log", &report);
    unsafe { sys::sceKernelExitGame() };
}
