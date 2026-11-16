#![no_std]
#![cfg_attr(all(target_arch = "mips", feature = "psp"), feature(asm_experimental_arch))]

extern crate alloc;

pub mod math;
pub mod n64_tex;
pub mod vfpu;
pub mod vram;

use alloc::vec::Vec;
use core::fmt;
use math::{Mat4, mat4_identity, mat4_mul};
use n64_tex::{decode_be_tlut, decode_n64_texture_to_rgba8888};
use pmm_assets::{PakSource, ResourceManager, SegError, SegmentTable};
use pmm_schema::{GfxWord, N64Mtx, N64Vtx, PmmTexHeader, ResourceId, gbi};

pub const MAX_DL_DEPTH: usize = 18;
pub const MAX_MTX_STACK: usize = 16;
const MAX_BE_CMDS_PER_DL: usize = 4096;

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

    fn set_texture_enabled(&mut self, _enabled: bool) {}
    fn set_tex_sampler(&mut self, _clamp_s: bool, _clamp_t: bool, _nearest: bool) {}
    fn set_cull_mode(&mut self, _cull_back: bool, _cull_front: bool) {}
    fn set_prim_env_tint(&mut self, _tint_abgr: u32) {}
    fn set_scissor(&mut self, _x0: i32, _y0: i32, _x1: i32, _y1: i32) {}
    fn upload_rgba8888_texture(&mut self, _tex_key: u64, _w: u16, _h: u16, _rgba: &[u8]) -> bool {
        false
    }
}

#[derive(Default)]
struct ActiveGpuState {
    bound_tex_id: Option<ResourceId>,
    bound_rom_tex: Option<u64>,
    depth_mode: Option<DepthMode>,
    blend_xlu: Option<bool>,
    tex_enabled: Option<bool>,
    cull_mode: Option<(bool, bool)>,
}

pub struct Renderer {
    state: ActiveGpuState,
    mtx_stack: Vec<Mat4>,
    vtx_cache: [PspVertex; 32],
    tex_scale_u: f32,
    tex_scale_v: f32,
    uv_scale_s: f32,
    uv_scale_t: f32,
    pending_tex_id_lo: u32,
    pending_tex_id: Option<ResourceId>,
    timg_addr: u32,
    timg_fmt: u8,
    timg_siz: u8,
    tile_fmt: u8,
    tile_siz: u8,
    tile_cms: u8,
    tile_cmt: u8,
    tlut_rgba: [u32; 256],
    geometry_mode: u32,
    texture_enabled: bool,
    prim_color: [u8; 4],
    env_color: [u8; 4],
    combine_uses_shade: bool,
    combine_uses_prim: bool,
    overlay_mode: bool,
    tex_namespace: u64,
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
            uv_scale_s: 1.0,
            uv_scale_t: 1.0,
            pending_tex_id_lo: 0,
            pending_tex_id: None,
            timg_addr: 0,
            timg_fmt: 0,
            timg_siz: 0,
            tile_fmt: 0,
            tile_siz: 0,
            tile_cms: 0,
            tile_cmt: 0,
            tlut_rgba: [0xFFFF_FFFF; 256],
            geometry_mode: gbi::G_ZBUFFER | gbi::G_SHADE | gbi::G_CULL_BACK,
            texture_enabled: false,
            prim_color: [255, 255, 255, 255],
            env_color: [255, 255, 255, 255],
            combine_uses_shade: true,
            combine_uses_prim: false,
            overlay_mode: false,
            tex_namespace: 0,
            static_list_tris: Vec::new(),
            stats: RenderStats::default(),
        }
    }

    pub fn set_tex_namespace(&mut self, ns: u64) {
        self.tex_namespace = ns;
    }

    pub fn push_model_matrix<B: GfxBackend>(&mut self, mtx: &Mat4, backend: &mut B) -> Result<(), GfxError> {
        if self.mtx_stack.len() >= MAX_MTX_STACK {
            return Err(GfxError::MatrixStackOverflow);
        }
        self.mtx_stack.push(*mtx);
        backend.set_model_matrix(mtx);
        Ok(())
    }

    pub fn pop_model_matrix<B: GfxBackend>(&mut self, backend: &mut B) -> Result<(), GfxError> {
        if self.mtx_stack.len() <= 1 {
            return Err(GfxError::MatrixStackUnderflow);
        }
        self.mtx_stack.pop();
        backend.set_model_matrix(self.mtx_stack.last().unwrap());
        Ok(())
    }

    pub fn begin_frame(&mut self) {
        self.state = ActiveGpuState::default();
        self.mtx_stack.clear();
        self.mtx_stack.push(mat4_identity());
        self.geometry_mode = gbi::G_ZBUFFER | gbi::G_SHADE | gbi::G_CULL_BACK;
        self.texture_enabled = false;
        self.uv_scale_s = 1.0;
        self.uv_scale_t = 1.0;
        self.prim_color = [255, 255, 255, 255];
        self.env_color = [255, 255, 255, 255];
        self.combine_uses_shade = true;
        self.combine_uses_prim = false;
        self.overlay_mode = false;
    }

    /// Put renderer in 2D overlay mode (disables depth test, depth writes, face culling, and texturing).
    pub fn set_overlay_mode<B: GfxBackend>(&mut self, overlay: bool, backend: &mut B) {
        self.overlay_mode = overlay;
        if overlay {
            self.apply_texture_enabled(false, backend);
            self.apply_cull_mode(false, false, backend);
            self.apply_depth_mode(
                DepthMode {
                    write: false,
                    test: false,
                    bias: 0,
                },
                backend,
            );
        }
    }

    fn apply_texture_enabled<B: GfxBackend>(&mut self, enabled: bool, backend: &mut B) {
        if self.state.tex_enabled != Some(enabled) {
            backend.set_texture_enabled(enabled);
            self.state.tex_enabled = Some(enabled);
        }
    }

    fn apply_depth_mode<B: GfxBackend>(&mut self, desired: DepthMode, backend: &mut B) {
        if self.state.depth_mode == Some(desired) {
            self.stats.redundant_binds_suppressed += 1;
        } else {
            backend.set_depth_mode(desired);
            self.state.depth_mode = Some(desired);
        }
    }

    fn apply_cull_mode<B: GfxBackend>(&mut self, cull_back: bool, cull_front: bool, backend: &mut B) {
        let desired = (cull_back, cull_front);
        if self.state.cull_mode != Some(desired) {
            backend.set_cull_mode(cull_back, cull_front);
            self.state.cull_mode = Some(desired);
        }
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
        self.process_dl_inner(cmds, None, segs, res, tex_scratch, backend, depth)
    }

    /// Execute a real big-endian N64 ROM display list mapped via `SegmentTable` (`0x02xxxxxx`, `0x03xxxxxx`).
    pub fn process_be_display_list<B: GfxBackend, S: PakSource>(
        &mut self,
        seg_addr: u32,
        segs: &SegmentTable,
        res: &mut ResourceManager<S>,
        tex_scratch: &mut [u8],
        backend: &mut B,
        depth: usize,
    ) -> Result<(), GfxError> {
        let Ok((ptr, rem)) = segs.resolve(seg_addr) else {
            return Ok(()); // Tolerate unmapped keep/overlay branches without crashing scene draw
        };
        let bytes = unsafe { core::slice::from_raw_parts(ptr, rem) };
        self.process_dl_inner(&[], Some(bytes), segs, res, tex_scratch, backend, depth)
    }

    /// Cached static execution for real N64 big-endian room display lists (`RoomShape` entries).
    pub fn process_static_be_display_list<B: GfxBackend, S: PakSource>(
        &mut self,
        list_id: u64,
        seg_addr: u32,
        segs: &SegmentTable,
        res: &mut ResourceManager<S>,
        tex_scratch: &mut [u8],
        backend: &mut B,
    ) -> Result<(), GfxError> {
        if backend.call_static_list(list_id) {
            self.stats.static_calllist_hits += 1;
            if let Some((_, tris)) = self.static_list_tris.iter().find(|(id, _)| *id == list_id) {
                self.stats.triangles_submitted += *tris;
            }
            return Ok(());
        }
        let tris_before = self.stats.triangles_submitted;
        let compiling = backend.begin_compile_static_list(list_id);
        let r = self.process_be_display_list(seg_addr, segs, res, tex_scratch, backend, 0);
        if compiling {
            backend.end_compile_static_list(list_id);
            let compiled_tris = self.stats.triangles_submitted.saturating_sub(tris_before);
            self.static_list_tris.push((list_id, compiled_tris));
            let _ = backend.call_static_list(list_id);
        }
        r
    }

    fn decode_vertex_color(&self, cn: [u8; 4]) -> u32 {
        let (r, g, b, a) = if (self.geometry_mode & gbi::G_LIGHTING) != 0 {
            // Directional sunlight + warm ambient over N64 signed vertex normal `(nx, ny, nz)`
            let nx = (cn[0] as i8) as f32 / 127.0;
            let ny = (cn[1] as i8) as f32 / 127.0;
            let nz = (cn[2] as i8) as f32 / 127.0;
            let ndotl = (nx * 0.45 + ny * 0.78 + nz * 0.43).max(0.0);
            let lum = (0.42 + 0.58 * ndotl).min(1.0);
            let pr = if self.combine_uses_prim { self.prim_color[0] as f32 } else { 255.0 };
            let pg = if self.combine_uses_prim { self.prim_color[1] as f32 } else { 255.0 };
            let pb = if self.combine_uses_prim { self.prim_color[2] as f32 } else { 255.0 };
            (
                (pr * lum).clamp(0.0, 255.0) as u8,
                (pg * lum).clamp(0.0, 255.0) as u8,
                (pb * lum).clamp(0.0, 255.0) as u8,
                cn[3],
            )
        } else if self.combine_uses_prim && !self.combine_uses_shade {
            (self.prim_color[0], self.prim_color[1], self.prim_color[2], self.prim_color[3])
        } else {
            (cn[0], cn[1], cn[2], cn[3])
        };
        u32::from_le_bytes([r, g, b, a])
    }

    fn process_dl_inner<B: GfxBackend, S: PakSource>(
        &mut self,
        le_cmds: &[GfxWord],
        be_bytes: Option<&[u8]>,
        segs: &SegmentTable,
        res: &mut ResourceManager<S>,
        tex_scratch: &mut [u8],
        backend: &mut B,
        depth: usize,
    ) -> Result<(), GfxError> {
        if depth > MAX_DL_DEPTH {
            return Err(GfxError::DisplayListDepthExceeded);
        }

        let is_be = be_bytes.is_some();
        let max_iters = match be_bytes {
            Some(b) => (b.len() / 8).min(MAX_BE_CMDS_PER_DL),
            None => le_cmds.len(),
        };

        let mut tri_batch = Vec::with_capacity(64);

        for idx in 0..max_iters {
            let cmd = match be_bytes {
                Some(b) => match GfxWord::from_be_bytes(&b[idx * 8..]) {
                    Some(c) => c,
                    None => break,
                },
                None => le_cmds[idx],
            };
            self.stats.commands_decoded += 1;

            match cmd.opcode() {
                gbi::G_ENDDL => {
                    self.flush_triangles(&mut tri_batch, backend);
                    return Ok(());
                }
                gbi::G_DL => {
                    self.flush_triangles(&mut tri_batch, backend);
                    let branch = ((cmd.w0 >> 16) & 0xFF) != 0;
                    if is_be {
                        if let Ok((ptr, rem)) = segs.resolve(cmd.w1) {
                            let child = unsafe { core::slice::from_raw_parts(ptr, rem) };
                            self.process_dl_inner(&[], Some(child), segs, res, tex_scratch, backend, depth + 1)?;
                        }
                    } else {
                        let (ptr, rem) = segs.resolve(cmd.w1)?;
                        let count = rem / core::mem::size_of::<GfxWord>();
                        let child = unsafe { core::slice::from_raw_parts(ptr as *const GfxWord, count) };
                        self.process_dl_inner(child, None, segs, res, tex_scratch, backend, depth + 1)?;
                    }
                    if branch {
                        return Ok(());
                    }
                }
                gbi::G_MTX => {
                    self.flush_triangles(&mut tri_batch, backend);
                    let flags = cmd.w0 & 0xFF;
                    let m = if is_be {
                        let Ok((ptr, rem)) = segs.resolve(cmd.w1) else { continue; };
                        if rem < 64 { continue; }
                        let slice = unsafe { core::slice::from_raw_parts(ptr, 64) };
                        N64Mtx::from_be_bytes(slice).unwrap_or_default().to_f32_4x4()
                    } else {
                        let (ptr, rem) = segs.resolve(cmd.w1)?;
                        if rem < core::mem::size_of::<N64Mtx>() {
                            return Err(GfxError::InvalidVertexSlice);
                        }
                        let n64_mtx = unsafe { &*(ptr as *const N64Mtx) };
                        n64_mtx.to_f32_4x4()
                    };
                    let push = (flags & gbi::G_MTX_PUSH) != 0;
                    let load = (flags & gbi::G_MTX_LOAD) != 0;
                    if push {
                        if self.mtx_stack.len() >= MAX_MTX_STACK {
                            return Err(GfxError::MatrixStackOverflow);
                        }
                        let top = *self.mtx_stack.last().unwrap();
                        self.mtx_stack.push(top);
                    }
                    let top = self.mtx_stack.last_mut().unwrap();
                    // Row-vector convention: `M_combined = child * parent` (`m * top`)
                    *top = if load { m } else { mat4_mul(&m, top) };
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
                gbi::G_TEXTURE => {
                    // F3DEX2 encodes `on` in bits 1..7 of `w0` (G_ON => `0x02`, G_OFF => `0x00`).
                    let on = (cmd.w0 & 0xFF) != 0;
                    let s_raw = ((cmd.w1 >> 16) & 0xFFFF) as f32;
                    let t_raw = (cmd.w1 & 0xFFFF) as f32;
                    self.uv_scale_s = if s_raw == 0.0 || s_raw >= 65535.0 { 1.0 } else { s_raw / 65536.0 };
                    self.uv_scale_t = if t_raw == 0.0 || t_raw >= 65535.0 { 1.0 } else { t_raw / 65536.0 };
                    if self.texture_enabled != on {
                        self.flush_triangles(&mut tri_batch, backend);
                        self.texture_enabled = on;
                        if !on {
                            self.apply_texture_enabled(false, backend);
                        }
                    }
                }
                gbi::G_GEOMETRYMODE => {
                    let clear_bits = (!cmd.w0) & 0x00FF_FFFF;
                    let set_bits = cmd.w1;
                    let next_geo = (self.geometry_mode & !clear_bits) | set_bits;
                    if next_geo != self.geometry_mode {
                        self.flush_triangles(&mut tri_batch, backend);
                        self.geometry_mode = next_geo;
                        if !self.overlay_mode {
                            let cull_back = (next_geo & gbi::G_CULL_BACK) != 0;
                            let cull_front = (next_geo & gbi::G_CULL_FRONT) != 0;
                            self.apply_cull_mode(cull_back, cull_front, backend);
                            let ztest = (next_geo & gbi::G_ZBUFFER) != 0;
                            let prev_mode = self.state.depth_mode.unwrap_or_default();
                            self.apply_depth_mode(
                                DepthMode {
                                    write: prev_mode.write && ztest,
                                    test: ztest,
                                    bias: prev_mode.bias,
                                },
                                backend,
                            );
                        }
                    }
                }
                gbi::G_SETPRIMCOLOR => {
                    let r = ((cmd.w1 >> 24) & 0xFF) as u8;
                    let g = ((cmd.w1 >> 16) & 0xFF) as u8;
                    let b = ((cmd.w1 >> 8) & 0xFF) as u8;
                    let a = (cmd.w1 & 0xFF) as u8;
                    self.prim_color = [r, g, b, a];
                    self.combine_uses_prim = true;
                    backend.set_prim_env_tint(u32::from_le_bytes([r, g, b, a]));
                }
                gbi::G_SETENVCOLOR => {
                    let r = ((cmd.w1 >> 24) & 0xFF) as u8;
                    let g = ((cmd.w1 >> 16) & 0xFF) as u8;
                    let b = ((cmd.w1 >> 8) & 0xFF) as u8;
                    let a = (cmd.w1 & 0xFF) as u8;
                    self.env_color = [r, g, b, a];
                }
                gbi::G_SETCOMBINE => {
                    // Check if SHADE input is referenced in color combiner fields
                    self.combine_uses_shade = ((cmd.w0 & 0x0F0F_FFFF) != 0x0F0F_FFFF) || !is_be;
                }
                gbi::G_SETIMG => {
                    self.pending_tex_id_lo = cmd.w1;
                    self.timg_addr = cmd.w1;
                    self.timg_fmt = ((cmd.w0 >> 21) & 0x7) as u8;
                    self.timg_siz = ((cmd.w0 >> 19) & 0x3) as u8;
                }
                gbi::G_LOADTLUT => {
                    let count = (((cmd.w1 >> 14) & 0x3FF) + 1) as usize;
                    if let Ok((ptr, rem)) = segs.resolve(self.timg_addr) {
                        let bytes = unsafe { core::slice::from_raw_parts(ptr, rem) };
                        decode_be_tlut(bytes, count, &mut self.tlut_rgba);
                    }
                }
                gbi::G_SETTILE => {
                    let hi = cmd.w1;
                    let tile_idx = (cmd.w1 >> 24) & 0x7;
                    if !is_be && hi != 0 {
                        let full = ((hi as u64) << 32) | (self.pending_tex_id_lo as u64);
                        self.pending_tex_id = Some(ResourceId(full));
                    }
                    if tile_idx == 0 {
                        self.tile_fmt = ((cmd.w0 >> 21) & 0x7) as u8;
                        self.tile_siz = ((cmd.w0 >> 19) & 0x3) as u8;
                        self.tile_cmt = ((cmd.w1 >> 18) & 0x3) as u8;
                        self.tile_cms = ((cmd.w1 >> 8) & 0x3) as u8;
                    }
                }
                gbi::G_SETTILESIZE => {
                    if let Some(tex_id) = self.pending_tex_id {
                        self.flush_triangles(&mut tri_batch, backend);
                        self.apply_texture_enabled(true, backend);
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
                                self.tex_scale_u = self.uv_scale_s / ((hdr.width.max(1) as f32) * 32.0);
                                self.tex_scale_v = self.uv_scale_t / ((hdr.height.max(1) as f32) * 32.0);
                                self.state.bound_tex_id = Some(tex_id);
                            }
                        }
                    } else if is_be && self.timg_addr != 0 && self.texture_enabled {
                        let uls = ((cmd.w0 >> 12) & 0xFFF) >> 2;
                        let ult = (cmd.w0 & 0xFFF) >> 2;
                        let lrs = ((cmd.w1 >> 12) & 0xFFF) >> 2;
                        let lrt = (cmd.w1 & 0xFFF) >> 2;
                        let w = (lrs.saturating_sub(uls) + 1) as u16;
                        let h = (lrt.saturating_sub(ult) + 1) as u16;
                        let tlut_sig = if self.tile_fmt == gbi::G_IM_FMT_CI {
                            (self.tlut_rgba[0] ^ self.tlut_rgba[1].rotate_left(13)) as u64
                        } else {
                            0
                        };
                        let key = self.tex_namespace.wrapping_mul(0x9E37_79B9_7F4A_7C15)
                            ^ (tlut_sig << 16)
                            ^ ((self.tile_fmt as u64) << 40)
                            ^ ((self.tile_siz as u64) << 32)
                            ^ (self.timg_addr as u64);
                        self.flush_triangles(&mut tri_batch, backend);
                        if self.state.bound_rom_tex == Some(key) {
                            self.apply_texture_enabled(true, backend);
                            self.stats.redundant_binds_suppressed += 1;
                        } else if let Ok((ptr, rem)) = segs.resolve(self.timg_addr) {
                            let src = unsafe { core::slice::from_raw_parts(ptr, rem) };
                            if let Some((pw, ph, rgba)) = decode_n64_texture_to_rgba8888(
                                src,
                                self.tile_fmt,
                                self.tile_siz,
                                w,
                                h,
                                &self.tlut_rgba,
                            ) {
                                if backend.upload_rgba8888_texture(key, pw, ph, &rgba) {
                                    self.stats.textures_uploaded += 1;
                                }
                                let clamp_s = (self.tile_cms & gbi::G_TX_CLAMP) != 0;
                                let clamp_t = (self.tile_cmt & gbi::G_TX_CLAMP) != 0;
                                backend.set_tex_sampler(clamp_s, clamp_t, false);
                                self.apply_texture_enabled(true, backend);
                                self.tex_scale_u = self.uv_scale_s / ((pw as f32) * 32.0);
                                self.tex_scale_v = self.uv_scale_t / ((ph as f32) * 32.0);
                                self.state.bound_rom_tex = Some(key);
                            } else {
                                self.apply_texture_enabled(false, backend);
                            }
                        } else {
                            self.apply_texture_enabled(false, backend);
                        }
                    }
                }
                gbi::G_SETOTHERMODE_L => {
                    let xlu = (cmd.w1 & 0x0010_0000) != 0;
                    let dec = (cmd.w1 & 0x0C00) == 0x0C00;
                    if !self.overlay_mode {
                        let ztest = (self.geometry_mode & gbi::G_ZBUFFER) != 0;
                        let desired_depth = DepthMode {
                            write: !xlu && ztest,
                            test: ztest,
                            bias: if dec { -2 } else { 0 },
                        };
                        if self.state.depth_mode != Some(desired_depth) {
                            self.flush_triangles(&mut tri_batch, backend);
                        }
                        self.apply_depth_mode(desired_depth, backend);
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
                    if n > 32 || v0 + n > 32 {
                        return Err(GfxError::InvalidVertexSlice);
                    }
                    if is_be {
                        let Ok((ptr, rem)) = segs.resolve(cmd.w1) else {
                            for i in 0..n {
                                self.vtx_cache[v0 + i] = PspVertex::default();
                            }
                            continue;
                        };
                        if rem < n * 16 {
                            return Err(GfxError::InvalidVertexSlice);
                        }
                        let raw = unsafe { core::slice::from_raw_parts(ptr, n * 16) };
                        for i in 0..n {
                            let v = N64Vtx::from_be_bytes(&raw[i * 16..]).unwrap_or_default();
                            let color = self.decode_vertex_color(v.cn);
                            self.vtx_cache[v0 + i] = PspVertex {
                                u: (v.tc[0] as f32) * self.tex_scale_u,
                                v: (v.tc[1] as f32) * self.tex_scale_v,
                                color,
                                x: v.ob[0] as f32,
                                y: v.ob[1] as f32,
                                z: v.ob[2] as f32,
                            };
                        }
                    } else {
                        let (ptr, rem) = segs.resolve(cmd.w1)?;
                        if rem < n * core::mem::size_of::<N64Vtx>() {
                            return Err(GfxError::InvalidVertexSlice);
                        }
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
            if !self.texture_enabled && self.pending_tex_id.is_none() {
                self.apply_texture_enabled(false, backend);
            }
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
    pub tex_enabled_calls: usize,
    pub cull_changes: usize,
    pub rom_textures_uploaded: usize,
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
    fn set_texture_enabled(&mut self, _enabled: bool) {
        self.tex_enabled_calls += 1;
    }
    fn set_cull_mode(&mut self, _cull_back: bool, _cull_front: bool) {
        self.cull_changes += 1;
    }
    fn upload_rgba8888_texture(&mut self, _tex_key: u64, _w: u16, _h: u16, _rgba: &[u8]) -> bool {
        self.rom_textures_uploaded += 1;
        true
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

        let (_, cnt0) = clip_triangle_near_plane([v(0.5), v(0.0), v(-0.5)], near);
        assert_eq!(cnt0, 0);

        let (out1, cnt1) = clip_triangle_near_plane([v(-2.0), v(0.0), v(0.0)], near);
        assert_eq!(cnt1, 1);
        assert!((out1[1].z + 1.0).abs() < 1e-4);
        assert!((out1[2].z + 1.0).abs() < 1e-4);

        let (out2, cnt2) = clip_triangle_near_plane([v(-2.0), v(-3.0), v(0.0)], near);
        assert_eq!(cnt2, 2);
        assert!((out2[2].z + 1.0).abs() < 1e-4);
        assert!((out2[4].z + 1.0).abs() < 1e-4);

        let (_, cnt3) = clip_triangle_near_plane([v(-2.0), v(-3.0), v(-4.0)], near);
        assert_eq!(cnt3, 1);
    }

    #[test]
    fn clock_town_12bit_tile_wrapping() {
        assert_eq!(wrap_tile_12bit(4096), 0);
        assert_eq!(wrap_tile_12bit(-1), 4095);
        assert_eq!(wrap_tile_12bit(123), 123);
    }

    #[test]
    fn letterbox_scissor_zero_gap_all_sizes_0_to_32() {
        for bar in 0..=32 {
            let (scene, top, bottom) = compute_letterbox_scissor(480, 272, bar);
            assert_eq!(top.y1, scene.y0);
            assert_eq!(scene.y1, bottom.y0);
            assert!(scene.y0 <= scene.y1);
        }
    }

    #[test]
    fn matrix_mul_and_lerp_math() {
        let mut a = mat4_identity();
        a[3][0] = 10.0;
        let mut b = mat4_identity();
        b[3][0] = 20.0;
        let mid = mat4_lerp_scalar(&a, &b, 0.5);
        assert!((mid[3][0] - 15.0).abs() < 1e-5);

        let c = mat4_mul_scalar(&a, &b);
        assert!((c[3][0] - 30.0).abs() < 1e-5);
    }

    #[test]
    fn static_calllist_caching_and_state_dedup() {
        let mut mgr = empty_pak_mgr();
        let mut backend = MockGfxBackend::default();
        let mut renderer = Renderer::new();
        let verts = [N64Vtx::default(); 3];
        let v_bytes = unsafe {
            core::slice::from_raw_parts(verts.as_ptr() as *const u8, core::mem::size_of_val(&verts))
        };
        let mut segs = SegmentTable::default();
        segs.set_segment(0x04, v_bytes);

        let dl = [
            GfxWord::new(((gbi::G_SETOTHERMODE_L as u32) << 24) | 1, 0),
            GfxWord::new(((gbi::G_SETOTHERMODE_L as u32) << 24) | 1, 0),
            GfxWord::new(((gbi::G_VTX as u32) << 24) | (3 << 12) | 6, 0x0400_0000),
            GfxWord::new(((gbi::G_TRI1 as u32) << 24) | (2 << 8) | 4, 0),
            GfxWord::new((gbi::G_ENDDL as u32) << 24, 0),
        ];
        let list_id = ResourceId::from_otr_path("__OTR__scenes/test/room0_dl");
        let mut scratch = [0u8; 4096];

        renderer.process_static_display_list(list_id, &dl, &segs, &mut mgr, &mut scratch, &mut backend).unwrap();
        assert_eq!(renderer.stats.static_calllist_hits, 0);
        assert!(renderer.stats.redundant_binds_suppressed >= 1);

        renderer.process_static_display_list(list_id, &dl, &segs, &mut mgr, &mut scratch, &mut backend).unwrap();
        assert_eq!(renderer.stats.static_calllist_hits, 1);
    }

    #[test]
    fn recursive_dl_depth_limit_negative_test() {
        let mut mgr = empty_pak_mgr();
        let mut backend = MockGfxBackend::default();
        let mut renderer = Renderer::new();
        let dl = [
            GfxWord::new((gbi::G_DL as u32) << 24, 0x0400_0000),
            GfxWord::new((gbi::G_ENDDL as u32) << 24, 0),
        ];
        let dl_bytes = unsafe {
            core::slice::from_raw_parts(dl.as_ptr() as *const u8, core::mem::size_of_val(&dl))
        };
        let mut segs = SegmentTable::default();
        segs.set_segment(0x04, dl_bytes);
        let mut scratch = [0u8; 64];
        let err = renderer
            .process_display_list(&dl, &segs, &mut mgr, &mut scratch, &mut backend, 0)
            .unwrap_err();
        assert_eq!(err, GfxError::DisplayListDepthExceeded);
    }

    #[test]
    fn motion_blur_capture_increments_counter() {
        let mut backend = MockGfxBackend::default();
        let mut renderer = Renderer::new();
        renderer.capture_motion_blur(&mut backend);
        renderer.capture_motion_blur(&mut backend);
        assert_eq!(backend.blur_captures, 2);
        assert_eq!(renderer.stats.motion_blur_passes, 2);
    }

    #[test]
    fn matrix_stack_push_pop_and_underflow_negative() {
        let mut mgr = empty_pak_mgr();
        let mut backend = MockGfxBackend::default();
        let mut renderer = Renderer::new();
        let segs = SegmentTable::default();
        let dl_underflow = [GfxWord::new((gbi::G_POPMTX as u32) << 24, 0)];
        let mut scratch = [0u8; 64];
        assert_eq!(
            renderer
                .process_display_list(&dl_underflow, &segs, &mut mgr, &mut scratch, &mut backend, 0)
                .unwrap_err(),
            GfxError::MatrixStackUnderflow
        );
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

    #[test]
    fn be_rom_display_list_lights_normals_and_decodes_n64_textures() {
        let mut mgr = empty_pak_mgr();
        let mut backend = MockGfxBackend::default();
        let mut renderer = Renderer::new();

        // Build segment 0x03 buffer holding:
        // [0x00..0x80]: 8x8 IA16 texture (intensity=200, alpha=255)
        // [0x80..0xC0]: 4 big-endian N64Vtx with upward normal (0, 127, 0)
        // [0xC0..0x100]: Big-endian F3DEX2 DL
        let mut seg3 = alloc::vec![0u8; 0x140];
        for i in 0..64 {
            seg3[i * 2] = 200;
            seg3[i * 2 + 1] = 255;
        }
        // 4 BE vertices at 0x80..0xC0
        for v in 0..4usize {
            let off = 0x80 + v * 16;
            seg3[off..off + 2].copy_from_slice(&((v as i16) * 10).to_be_bytes());
            seg3[off + 12] = 0;   // nx
            seg3[off + 13] = 127; // ny (+1.0)
            seg3[off + 14] = 0;   // nz
            seg3[off + 15] = 255; // alpha
        }
        // Build BE F3DEX2 commands at 0xC0
        let cmds: [(u32, u32); 8] = [
            (0xD700_0002, 0xFFFF_FFFF), // G_TEXTURE ON, scale 1.0
            (0xD900_0000, 0x0023_0405), // G_GEOMETRYMODE ZBUFFER|SHADE|CULL_BACK|LIGHTING
            (0xFA00_00FF, 0xC864_32FF), // G_SETPRIMCOLOR (200, 100, 50, 255)
            (0xFD70_0000, 0x0300_0000), // G_SETTIMG IA16 @ seg3+0
            (0xF570_0000, 0x0000_0000), // G_SETTILE fmt=3 siz=2
            (0xF200_0000, 0x001C_001C), // G_SETTILESIZE 8x8 (lrs=7*4=28=0x1c)
            (0x0100_4008, 0x0300_0080), // G_VTX n=4 @ seg3+0x80
            (0x0600_0204, 0x0000_0406), // G_TRI2
        ];
        for (i, (w0, w1)) in cmds.iter().enumerate() {
            let p = 0xC0 + i * 8;
            seg3[p..p + 4].copy_from_slice(&w0.to_be_bytes());
            seg3[p + 4..p + 8].copy_from_slice(&w1.to_be_bytes());
        }
        let end_p = 0xC0 + cmds.len() * 8;
        seg3[end_p..end_p + 4].copy_from_slice(&0xDF00_0000u32.to_be_bytes());

        let mut segs = SegmentTable::default();
        segs.set_segment(0x03, &seg3);
        let mut scratch = [0u8; 4096];
        renderer.process_be_display_list(0x0300_00C0, &segs, &mut mgr, &mut scratch, &mut backend, 0).unwrap();

        assert_eq!(backend.rom_textures_uploaded, 1);
        assert_eq!(backend.total_tris, 2);
        // Check directional lighting + prim color applied cleanly (no raw normal=0 black/garbage R channel)
        let r_lit = (renderer.vtx_cache[0].color & 0xFF) as u8;
        assert!(r_lit > 120, "expected lit warm prim color R > 120, got {r_lit}");
    }
}
