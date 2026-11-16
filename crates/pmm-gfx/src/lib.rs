#![no_std]
#![cfg_attr(all(target_arch = "mips", feature = "psp"), feature(asm_experimental_arch))]

extern crate alloc;

pub mod math;
pub mod vfpu;
pub mod vram;

use alloc::vec::Vec;
use core::fmt;
use math::{Mat4, mat4_identity, mat4_mul};
use pmm_assets::{PakSource, ResourceManager, SegError, SegmentTable};
use pmm_schema::{GfxWord, N64Mtx, N64Vtx, PmmTexHeader, ResourceId, gbi};

pub const MAX_DL_DEPTH: usize = 18;
pub const MAX_MTX_STACK: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GfxError {
    DisplayListDepthExceeded,
    MatrixStackOverflow,
    MatrixStackUnderflow,
    Segment(SegError),
    InvalidVertexSlice,
}

impl From<SegError> for GfxError {
    fn from(e: SegError) -> Self {
        Self::Segment(e)
    }
}

impl fmt::Display for GfxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DisplayListDepthExceeded => f.write_str("F3DEX2 display list depth > 18"),
            Self::MatrixStackOverflow => f.write_str("matrix stack overflow"),
            Self::MatrixStackUnderflow => f.write_str("matrix stack underflow"),
            Self::Segment(s) => write!(f, "segment error: {s:?}"),
            Self::InvalidVertexSlice => f.write_str("invalid vertex buffer slice"),
        }
    }
}

/// 24-byte PSP GE hardware vertex (`TEXTURE_32BITF | COLOR_8888 | VERTEX_32BITF`).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
#[repr(C, align(4))]
pub struct PspVertex {
    pub u: f32,
    pub v: f32,
    pub color: u32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DepthMode {
    pub write: bool,
    pub test: bool,
    pub bias: i32,
}

impl Default for DepthMode {
    fn default() -> Self {
        Self {
            write: true,
            test: true,
            bias: 0,
        }
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct RenderStats {
    pub commands_decoded: u64,
    pub triangles_submitted: u64,
    pub textures_uploaded: u64,
    pub static_calllist_hits: u64,
    pub redundant_binds_suppressed: u64,
    pub motion_blur_passes: u64,
}

/// Hardware abstraction implemented by `src/platform/gfx_psp.rs` on PSP and
/// `MockGfxBackend` on host tests.
pub trait GfxBackend {
    fn bind_texture(&mut self, tex_id: ResourceId, hdr: &PmmTexHeader, payload: &[u8]) -> bool;
    fn set_depth_mode(&mut self, mode: DepthMode);
    fn set_blend_xlu(&mut self, xlu: bool);
    fn set_model_matrix(&mut self, mtx: &Mat4);
    fn draw_triangles(&mut self, verts: &[PspVertex]);
    fn call_static_list(&mut self, list_id: u64) -> bool;
    fn begin_compile_static_list(&mut self, list_id: u64) -> bool;
    fn end_compile_static_list(&mut self, list_id: u64);
    fn capture_motion_blur(&mut self);
}

#[derive(Default)]
struct ActiveGpuState {
    bound_tex_id: Option<ResourceId>,
    depth_mode: Option<DepthMode>,
    blend_xlu: Option<bool>,
}

pub struct Renderer {
    state: ActiveGpuState,
    mtx_stack: Vec<Mat4>,
    vtx_cache: [PspVertex; 32],
    tex_scale_u: f32,
    tex_scale_v: f32,
    pending_tex_id_lo: u32,
    pending_tex_id: Option<ResourceId>,
    static_list_tris: Vec<(u64, u64)>,
    pub stats: RenderStats,
}

impl Default for Renderer {
    fn default() -> Self {
        Self::new()
    }
}

impl Renderer {
    pub fn new() -> Self {
        let mut mtx_stack = Vec::with_capacity(MAX_MTX_STACK);
        mtx_stack.push(mat4_identity());
        Self {
            state: ActiveGpuState::default(),
            mtx_stack,
            vtx_cache: [PspVertex::default(); 32],
            tex_scale_u: 1.0 / 1024.0,
            tex_scale_v: 1.0 / 1024.0,
            pending_tex_id_lo: 0,
            pending_tex_id: None,
            static_list_tris: Vec::new(),
            stats: RenderStats::default(),
        }
    }

    pub fn begin_frame(&mut self) {
        self.state = ActiveGpuState::default();
        self.mtx_stack.clear();
        self.mtx_stack.push(mat4_identity());
    }

    /// Execute a cached static room/object display list (`sceGuCallList` on cache hit,
    /// compile + execute on first frame).
    pub fn process_static_display_list<B: GfxBackend, S: PakSource>(
        &mut self,
        list_id: ResourceId,
        cmds: &[GfxWord],
        segs: &SegmentTable,
        res: &mut ResourceManager<S>,
        tex_scratch: &mut [u8],
        backend: &mut B,
    ) -> Result<(), GfxError> {
        if backend.call_static_list(list_id.0) {
            self.stats.static_calllist_hits += 1;
            self.stats.commands_decoded += cmds.len() as u64;
            if let Some((_, tris)) = self.static_list_tris.iter().find(|(id, _)| *id == list_id.0) {
                self.stats.triangles_submitted += *tris;
            }
            return Ok(());
        }
        let tris_before = self.stats.triangles_submitted;
        let compiling = backend.begin_compile_static_list(list_id.0);
        let r = self.process_display_list(cmds, segs, res, tex_scratch, backend, 0);
        if compiling {
            backend.end_compile_static_list(list_id.0);
            let compiled_tris = self.stats.triangles_submitted.saturating_sub(tris_before);
            self.static_list_tris.push((list_id.0, compiled_tris));
            let _ = backend.call_static_list(list_id.0);
        }
        r
    }

    pub fn process_display_list<B: GfxBackend, S: PakSource>(
        &mut self,
        cmds: &[GfxWord],
        segs: &SegmentTable,
        res: &mut ResourceManager<S>,
        tex_scratch: &mut [u8],
        backend: &mut B,
        depth: usize,
    ) -> Result<(), GfxError> {
        if depth > MAX_DL_DEPTH {
            return Err(GfxError::DisplayListDepthExceeded);
        }

        let mut tri_batch = Vec::with_capacity(64);
        for &cmd in cmds {
            self.stats.commands_decoded += 1;
            match cmd.opcode() {
                gbi::G_ENDDL => {
                    self.flush_triangles(&mut tri_batch, backend);
                    return Ok(());
                }
                gbi::G_DL => {
                    self.flush_triangles(&mut tri_batch, backend);
                    let branch = ((cmd.w0 >> 16) & 0xFF) != 0;
                    let (ptr, rem) = segs.resolve(cmd.w1)?;
                    let count = rem / core::mem::size_of::<GfxWord>();
                    // SAFETY: `segs.resolve` guarantees `rem` readable bytes at `ptr`.
                    let child = unsafe { core::slice::from_raw_parts(ptr as *const GfxWord, count) };
                    self.process_display_list(child, segs, res, tex_scratch, backend, depth + 1)?;
                    if branch {
                        return Ok(());
                    }
                }
                gbi::G_MTX => {
                    self.flush_triangles(&mut tri_batch, backend);
                    let flags = cmd.w0 & 0xFF;
                    let (ptr, rem) = segs.resolve(cmd.w1)?;
                    if rem < core::mem::size_of::<N64Mtx>() {
                        return Err(GfxError::InvalidVertexSlice);
                    }
                    // SAFETY: `rem >= 64` verified above.
                    let n64_mtx = unsafe { &*(ptr as *const N64Mtx) };
                    let m = n64_mtx.to_f32_4x4();
                    let push = (flags & (gbi::G_MTX_PUSH as u32)) != 0;
                    let load = (flags & (gbi::G_MTX_LOAD as u32)) != 0;
                    if push {
                        if self.mtx_stack.len() >= MAX_MTX_STACK {
                            return Err(GfxError::MatrixStackOverflow);
                        }
                        let top = *self.mtx_stack.last().unwrap();
                        self.mtx_stack.push(top);
                    }
                    let top = self.mtx_stack.last_mut().unwrap();
                    *top = if load { m } else { mat4_mul(top, &m) };
                    backend.set_model_matrix(top);
                }
                gbi::G_POPMTX => {
                    self.flush_triangles(&mut tri_batch, backend);
                    if self.mtx_stack.len() <= 1 {
                        return Err(GfxError::MatrixStackUnderflow);
                    }
                    self.mtx_stack.pop();
                    backend.set_model_matrix(self.mtx_stack.last().unwrap());
                }
                gbi::G_SETIMG => {
                    self.pending_tex_id_lo = cmd.w1;
                }
                gbi::G_SETTILE => {
                    let hi = cmd.w1;
                    if hi != 0 {
                        let full = ((hi as u64) << 32) | (self.pending_tex_id_lo as u64);
                        self.pending_tex_id = Some(ResourceId(full));
                    }
                }
                gbi::G_SETTILESIZE => {
                    if let Some(tex_id) = self.pending_tex_id {
                        self.flush_triangles(&mut tri_batch, backend);
                        if self.state.bound_tex_id == Some(tex_id) {
                            self.stats.redundant_binds_suppressed += 1;
                        } else {
                            let mut arena = pmm_assets::BoundedArena::from_slice(tex_scratch);
                            if let Ok((_, payload)) = res.load_into_arena(tex_id, &mut arena)
                                && let Ok(hdr) = PmmTexHeader::from_le_bytes(payload)
                            {
                                let uploaded = backend.bind_texture(tex_id, &hdr, &payload[16..]);
                                if uploaded {
                                    self.stats.textures_uploaded += 1;
                                }
                                self.tex_scale_u = 1.0 / ((hdr.width.max(1) as f32) * 32.0);
                                self.tex_scale_v = 1.0 / ((hdr.height.max(1) as f32) * 32.0);
                                self.state.bound_tex_id = Some(tex_id);
                            }
                        }
                    }
                }
                gbi::G_SETOTHERMODE_L => {
                    let xlu = (cmd.w1 & 0x0010_0000) != 0;
                    let dec = (cmd.w1 & 0x0C00) == 0x0C00;
                    let desired_depth = DepthMode {
                        write: !xlu,
                        test: true,
                        bias: if dec { -2 } else { 0 },
                    };
                    if self.state.depth_mode == Some(desired_depth) {
                        self.stats.redundant_binds_suppressed += 1;
                    } else {
                        self.flush_triangles(&mut tri_batch, backend);
                        backend.set_depth_mode(desired_depth);
                        self.state.depth_mode = Some(desired_depth);
                    }
                    if self.state.blend_xlu == Some(xlu) {
                        self.stats.redundant_binds_suppressed += 1;
                    } else {
                        self.flush_triangles(&mut tri_batch, backend);
                        backend.set_blend_xlu(xlu);
                        self.state.blend_xlu = Some(xlu);
                    }
                }
                gbi::G_VTX => {
                    let n = ((cmd.w0 >> 12) & 0xFF) as usize;
                    let v0_end = ((cmd.w0 & 0xFF) / 2) as usize;
                    let v0 = v0_end.saturating_sub(n);
                    let (ptr, rem) = segs.resolve(cmd.w1)?;
                    if n > 32 || v0 + n > 32 || rem < n * core::mem::size_of::<N64Vtx>() {
                        return Err(GfxError::InvalidVertexSlice);
                    }
                    // SAFETY: Bounds checked above.
                    let src = unsafe { core::slice::from_raw_parts(ptr as *const N64Vtx, n) };
                    for (i, v) in src.iter().enumerate() {
                        let color = u32::from_le_bytes(v.cn);
                        self.vtx_cache[v0 + i] = PspVertex {
                            u: (v.tc[0] as f32) * self.tex_scale_u,
                            v: (v.tc[1] as f32) * self.tex_scale_v,
                            color,
                            x: v.ob[0] as f32,
                            y: v.ob[1] as f32,
                            z: v.ob[2] as f32,
                        };
                    }
                }
                gbi::G_TRI1 => {
                    let i0 = (((cmd.w0 >> 16) & 0xFF) / 2) as usize;
                    let i1 = (((cmd.w0 >> 8) & 0xFF) / 2) as usize;
                    let i2 = ((cmd.w0 & 0xFF) / 2) as usize;
                    if i0 < 32 && i1 < 32 && i2 < 32 {
                        tri_batch.push(self.vtx_cache[i0]);
                        tri_batch.push(self.vtx_cache[i1]);
                        tri_batch.push(self.vtx_cache[i2]);
                    }
                }
                gbi::G_TRI2 | gbi::G_QUAD => {
                    let a0 = (((cmd.w0 >> 16) & 0xFF) / 2) as usize;
                    let a1 = (((cmd.w0 >> 8) & 0xFF) / 2) as usize;
                    let a2 = ((cmd.w0 & 0xFF) / 2) as usize;
                    let b0 = (((cmd.w1 >> 16) & 0xFF) / 2) as usize;
                    let b1 = (((cmd.w1 >> 8) & 0xFF) / 2) as usize;
                    let b2 = ((cmd.w1 & 0xFF) / 2) as usize;
                    if a0 < 32 && a1 < 32 && a2 < 32 {
                        tri_batch.push(self.vtx_cache[a0]);
                        tri_batch.push(self.vtx_cache[a1]);
                        tri_batch.push(self.vtx_cache[a2]);
                    }
                    if b0 < 32 && b1 < 32 && b2 < 32 {
                        tri_batch.push(self.vtx_cache[b0]);
                        tri_batch.push(self.vtx_cache[b1]);
                        tri_batch.push(self.vtx_cache[b2]);
                    }
                }
                _ => {}
            }
        }
        self.flush_triangles(&mut tri_batch, backend);
        Ok(())
    }

    fn flush_triangles<B: GfxBackend>(&mut self, batch: &mut Vec<PspVertex>, backend: &mut B) {
        if !batch.is_empty() {
            self.stats.triangles_submitted += (batch.len() / 3) as u64;
            backend.draw_triangles(batch);
            batch.clear();
        }
    }

    pub fn capture_motion_blur<B: GfxBackend>(&mut self, backend: &mut B) {
        backend.capture_motion_blur();
        self.stats.motion_blur_passes += 1;
    }
}

/// Host/test mock backend tracking state deduplication and static list cache hits.
#[derive(Default)]
pub struct MockGfxBackend {
    pub draw_calls: usize,
    pub total_tris: usize,
    pub tex_binds: usize,
    pub cached_lists: Vec<u64>,
    pub blur_captures: usize,
}

impl GfxBackend for MockGfxBackend {
    fn bind_texture(&mut self, _tex_id: ResourceId, _hdr: &PmmTexHeader, _payload: &[u8]) -> bool {
        self.tex_binds += 1;
        true
    }
    fn set_depth_mode(&mut self, _mode: DepthMode) {}
    fn set_blend_xlu(&mut self, _xlu: bool) {}
    fn set_model_matrix(&mut self, _mtx: &Mat4) {}
    fn draw_triangles(&mut self, verts: &[PspVertex]) {
        self.draw_calls += 1;
        self.total_tris += verts.len() / 3;
    }
    fn call_static_list(&mut self, list_id: u64) -> bool {
        self.cached_lists.contains(&list_id)
    }
    fn begin_compile_static_list(&mut self, _list_id: u64) -> bool {
        true
    }
    fn end_compile_static_list(&mut self, list_id: u64) {
        self.cached_lists.push(list_id);
    }
    fn capture_motion_blur(&mut self) {
        self.blur_captures += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::{
        clip_triangle_near_plane, compute_letterbox_scissor, mat4_lerp_scalar, mat4_mul_scalar,
        wrap_tile_12bit,
    };
    use crate::vram::{PSP_VRAM_TOTAL_BYTES, VramLayout};
    use pmm_assets::MemPakSource;
    use pmm_schema::{PAK_HEADER_SIZE, PmmPakHeader};

    fn empty_pak_mgr() -> ResourceManager<MemPakSource> {
        let hdr = PmmPakHeader::new(0, PAK_HEADER_SIZE as u32, 64, 0, 0, 64);
        ResourceManager::open(MemPakSource::new(hdr.to_le_bytes().to_vec())).unwrap()
    }

    #[test]
    fn vram_layout_fits_exact_2mib() {
        let layout = VramLayout::new_triple_buffered_lcd();
        assert_eq!(layout.total_bytes, PSP_VRAM_TOTAL_BYTES);
        assert_eq!(layout.zbuf_offset, 0x000C_C000);
        assert_eq!(layout.blur_history_offset, 0x0011_0000);
        assert_eq!(layout.tex_scratch_offset, 0x0015_4000);
        assert!(layout.tex_scratch_bytes > 680 * 1024);
    }

    #[test]
    fn near_plane_clipper_all_cases() {
        let near = 1.0f32;
        let v = |z: f32| PspVertex {
            u: 0.0,
            v: 0.0,
            color: 0xFFFF_FFFF,
            x: 0.0,
            y: 0.0,
            z,
        };

        // Case 0: all behind near (depth = -z < 1.0)
        let (_, cnt0) = clip_triangle_near_plane([v(0.5), v(0.0), v(-0.5)], near);
        assert_eq!(cnt0, 0);

        // Case 1: 1 vertex inside (-2.0), 2 behind (0.0, 0.0) -> 1 clipped tri
        let (out1, cnt1) = clip_triangle_near_plane([v(-2.0), v(0.0), v(0.0)], near);
        assert_eq!(cnt1, 1);
        assert!((out1[1].z + 1.0).abs() < 1e-4);
        assert!((out1[2].z + 1.0).abs() < 1e-4);

        // Case 2: 2 vertices inside (-2.0, -3.0), 1 behind (0.0) -> 2 clipped tris
        let (out2, cnt2) = clip_triangle_near_plane([v(-2.0), v(-3.0), v(0.0)], near);
        assert_eq!(cnt2, 2);
        assert!((out2[2].z + 1.0).abs() < 1e-4);
        assert!((out2[4].z + 1.0).abs() < 1e-4);

        // Case 3: all inside -> 1 unchanged tri
        let (_, cnt3) = clip_triangle_near_plane([v(-2.0), v(-3.0), v(-4.0)], near);
        assert_eq!(cnt3, 1);
    }

    #[test]
    fn clock_town_12bit_tile_wrapping() {
        assert_eq!(wrap_tile_12bit(0), 0);
        assert_eq!(wrap_tile_12bit(4096), 0);
        assert_eq!(wrap_tile_12bit(4097), 1);
        assert_eq!(wrap_tile_12bit(-1), 4095);
    }

    #[test]
    fn letterbox_scissor_zero_gap_all_sizes_0_to_32() {
        for bar in 0..=32 {
            let (scissor, top, bot) = compute_letterbox_scissor(480, 272, bar);
            assert_eq!(top.y1, scissor.y0);
            assert_eq!(scissor.y1, bot.y0);
            assert_eq!(top.y0, 0);
            assert_eq!(bot.y1, 272);
        }
    }

    #[test]
    fn matrix_mul_and_lerp_math() {
        let id = mat4_identity();
        let mut t = mat4_identity();
        t[3][0] = 10.0;
        t[3][1] = 20.0;
        t[3][2] = -4.0;
        assert_eq!(mat4_mul_scalar(&id, &t), t);
        let half = mat4_lerp_scalar(&id, &t, 0.5);
        assert!((half[3][0] - 5.0).abs() < 1e-5);
        assert!((half[3][1] - 10.0).abs() < 1e-5);
        assert!((half[3][2] + 2.0).abs() < 1e-5);
    }

    #[test]
    fn recursive_dl_depth_limit_negative_test() {
        let mut mgr = empty_pak_mgr();
        let mut backend = MockGfxBackend::default();
        let mut renderer = Renderer::new();

        // Self-recursive G_DL at segment 0x0E
        let self_dl = [GfxWord::new((gbi::G_DL as u32) << 24, 0x0E00_0000)];
        let bytes = unsafe {
            core::slice::from_raw_parts(self_dl.as_ptr() as *const u8, core::mem::size_of_val(&self_dl))
        };
        let mut segs = SegmentTable::default();
        segs.set_segment(0x0E, bytes);

        let mut scratch = [0u8; 64];
        let err = renderer
            .process_display_list(&self_dl, &segs, &mut mgr, &mut scratch, &mut backend, 0)
            .unwrap_err();
        assert_eq!(err, GfxError::DisplayListDepthExceeded);
    }

    #[test]
    fn static_calllist_caching_and_state_dedup() {
        let mut mgr = empty_pak_mgr();
        let mut backend = MockGfxBackend::default();
        let mut renderer = Renderer::new();
        let verts = [
            N64Vtx { ob: [0, 0, -5], flag: 0, tc: [0, 0], cn: [255, 255, 255, 255] },
            N64Vtx { ob: [5, 0, -5], flag: 0, tc: [0, 0], cn: [255, 255, 255, 255] },
            N64Vtx { ob: [0, 5, -5], flag: 0, tc: [0, 0], cn: [255, 255, 255, 255] },
        ];
        let vtx_bytes = unsafe {
            core::slice::from_raw_parts(verts.as_ptr() as *const u8, core::mem::size_of_val(&verts))
        };
        let mut segs = SegmentTable::default();
        segs.set_segment(0x06, vtx_bytes);

        let dl = [
            GfxWord::new((gbi::G_SETOTHERMODE_L as u32) << 24, 0),
            GfxWord::new((gbi::G_SETOTHERMODE_L as u32) << 24, 0), // redundant!
            GfxWord::new(((gbi::G_VTX as u32) << 24) | (3 << 12) | (3 * 2), 0x0600_0000),
            GfxWord::new(((gbi::G_TRI1 as u32) << 24) | (0 << 16) | (2 << 8) | 4, 0),
            GfxWord::new((gbi::G_ENDDL as u32) << 24, 0),
        ];
        let mut scratch = [0u8; 64];
        let list_id = ResourceId::from_otr_path("test/room0");
        for _ in 0..10 {
            renderer.begin_frame();
            renderer
                .process_static_display_list(list_id, &dl, &segs, &mut mgr, &mut scratch, &mut backend)
                .unwrap();
        }
        assert_eq!(renderer.stats.static_calllist_hits, 9);
        assert!(renderer.stats.redundant_binds_suppressed >= 2);
        assert_eq!(backend.total_tris, 1);
    }

    #[test]
    fn matrix_stack_push_pop_and_underflow_negative() {
        let mut mgr = empty_pak_mgr();
        let mut backend = MockGfxBackend::default();
        let mut renderer = Renderer::new();
        let segs = SegmentTable::default();
        let dl = [GfxWord::new((gbi::G_POPMTX as u32) << 24, 1)];
        let mut scratch = [0u8; 64];
        assert_eq!(
            renderer
                .process_display_list(&dl, &segs, &mut mgr, &mut scratch, &mut backend, 0)
                .unwrap_err(),
            GfxError::MatrixStackUnderflow
        );
    }

    #[test]
    fn motion_blur_capture_increments_counter() {
        let mut backend = MockGfxBackend::default();
        let mut renderer = Renderer::new();
        renderer.capture_motion_blur(&mut backend);
        assert_eq!(backend.blur_captures, 1);
        assert_eq!(renderer.stats.motion_blur_passes, 1);
    }

    #[test]
    fn mtx_stack_overflow_negative() {
        let mut mgr = empty_pak_mgr();
        let mut backend = MockGfxBackend::default();
        let mut renderer = Renderer::new();
        let m = N64Mtx::identity();
        let m_bytes = unsafe {
            core::slice::from_raw_parts(&m as *const N64Mtx as *const u8, core::mem::size_of::<N64Mtx>())
        };
        let mut segs = SegmentTable::default();
        segs.set_segment(0x05, m_bytes);
        let dl = [GfxWord::new(
            ((gbi::G_MTX as u32) << 24) | gbi::G_MTX_PUSH,
            0x0500_0000,
        ); 20];
        let mut scratch = [0u8; 64];
        assert_eq!(
            renderer
                .process_display_list(&dl, &segs, &mut mgr, &mut scratch, &mut backend, 0)
                .unwrap_err(),
            GfxError::MatrixStackOverflow
        );
    }

    #[test]
    fn vfpu_equivalence_and_vec4_transform() {
        let err = crate::vfpu::verify_vfpu_equivalence();
        assert!(err < 1e-4, "VFPU vs scalar error {err}");
    }
}
