use alloc::vec::Vec;
use core::ffi::c_void;
use core::ptr;
use pmm_gfx::math::Mat4;
use pmm_gfx::vram::{PSP_BUF_WIDTH, PSP_SCREEN_HEIGHT, PSP_SCREEN_WIDTH, VramLayout};
use pmm_gfx::{DepthMode, GfxBackend, PspVertex};
use pmm_schema::{PmmTexHeader, ResourceId};
use psp::sys::{
    self, AlphaFunc, BlendFactor, BlendOp, ClearBuffer, DepthFunc, DisplayMode,
    DisplayPixelFormat, DisplaySetBufSync, FrontFaceDirection, GuContextType, GuPrimitive,
    GuState, GuSyncBehavior, GuSyncMode, GuTexWrapMode, MatrixMode, MipmapLevel, ScePspFMatrix4,
    ShadingModel, TextureColorComponent, TextureEffect, TextureFilter, TexturePixelFormat,
    VertexType, sceKernelDcacheWritebackInvalidateRange,
};

const DIRECT_LIST_BYTES: usize = 512 * 1024;
const STATIC_CACHE_OFFSET: usize = 2 * DIRECT_LIST_BYTES; // 1 MiB
const STATIC_CACHE_BYTES: usize = 2 * 1024 * 1024; // 2 MiB
const TEX_CACHE_OFFSET: usize = STATIC_CACHE_OFFSET + STATIC_CACHE_BYTES; // 3 MiB
const TEX_CACHE_BYTES: usize = 2 * 1024 * 1024 + 512 * 1024; // 2.5 MiB
const DYN_VTX_OFFSET: usize = TEX_CACHE_OFFSET + TEX_CACHE_BYTES; // 5.5 MiB
const DYN_VTX_HALF_BYTES: usize = 256 * 1024; // 2x 256 KiB = 512 KiB -> 6 MiB total

struct CachedTexture {
    key: u64,
    ptr: *const u8,
    width: i32,
    height: i32,
    psm: TexturePixelFormat,
    swizzled: i32,
}

struct CachedStaticList {
    id: u64,
    list_ptr: *const c_void,
}

/// PSP hardware GU backend backed by Pool 3 (`RendererCache` 6 MiB) and
/// the 2 MiB eDRAM triple-buffered layout (`VramLayout`).
pub struct PspGfxBackend {
    pool_base: *mut u8,
    vram_layout: VramLayout,
    edram_base: *mut u8,
    frame_idx: usize,
    static_cursor: usize,
    tex_cursor: usize,
    dyn_vtx_cursor: usize,
    compiling_list_start: Option<*mut u8>,
    textures: Vec<CachedTexture>,
    static_lists: Vec<CachedStaticList>,
}

impl PspGfxBackend {
    /// # Safety
    /// `pool_base` must point to a 64-byte aligned, 6 MiB Pool 3 buffer that outlives this backend.
    pub unsafe fn new(pool_base: *mut u8, pool_len: usize) -> Self {
        assert!(pool_len >= DYN_VTX_OFFSET + 2 * DYN_VTX_HALF_BYTES);
        let vram_layout = VramLayout::new_triple_buffered_lcd();
        let edram_base = unsafe { sys::sceGeEdramGetAddr() };

        let backend = Self {
            pool_base,
            vram_layout,
            edram_base,
            frame_idx: 0,
            static_cursor: STATIC_CACHE_OFFSET,
            tex_cursor: TEX_CACHE_OFFSET,
            dyn_vtx_cursor: 0,
            compiling_list_start: None,
            textures: Vec::with_capacity(128),
            static_lists: Vec::with_capacity(128),
        };
        backend.init_gu();
        backend
    }

    pub fn vram_layout(&self) -> VramLayout {
        self.vram_layout
    }

    fn direct_list_ptr(&self) -> *mut c_void {
        let off = (self.frame_idx & 1) * DIRECT_LIST_BYTES;
        let cached = unsafe { self.pool_base.add(off) as usize };
        (cached | 0x4000_0000) as *mut c_void // Uncached pointer for direct GE list writes
    }

    fn fb_rel_ptr(&self, slot: usize) -> *mut c_void {
        let off = match slot % 3 {
            0 => self.vram_layout.fb0_offset,
            1 => self.vram_layout.fb1_offset,
            _ => self.vram_layout.fb2_offset,
        };
        off as *mut c_void
    }

    fn fb_abs_ptr(&self, slot: usize) -> *mut c_void {
        let off = match slot % 3 {
            0 => self.vram_layout.fb0_offset,
            1 => self.vram_layout.fb1_offset,
            _ => self.vram_layout.fb2_offset,
        };
        unsafe { self.edram_base.add(off) as *mut c_void }
    }

    fn init_gu(&self) {
        unsafe {
            sys::sceGumLoadIdentity();
            sys::sceGuInit();
            sys::sceGuStart(GuContextType::Direct, self.direct_list_ptr());
            sys::sceGuDrawBuffer(
                DisplayPixelFormat::Psm5650,
                self.fb_rel_ptr(0),
                PSP_BUF_WIDTH as i32,
            );
            sys::sceGuDispBuffer(
                PSP_SCREEN_WIDTH as i32,
                PSP_SCREEN_HEIGHT as i32,
                self.fb_rel_ptr(1),
                PSP_BUF_WIDTH as i32,
            );
            sys::sceGuDepthBuffer(
                self.vram_layout.zbuf_offset as *mut c_void,
                PSP_BUF_WIDTH as i32,
            );
            sys::sceGuOffset(
                2048 - (PSP_SCREEN_WIDTH as u32 / 2),
                2048 - (PSP_SCREEN_HEIGHT as u32 / 2),
            );
            sys::sceGuViewport(2048, 2048, PSP_SCREEN_WIDTH as i32, PSP_SCREEN_HEIGHT as i32);
            sys::sceGuDepthRange(65535, 0);
            sys::sceGuScissor(0, 0, PSP_SCREEN_WIDTH as i32, PSP_SCREEN_HEIGHT as i32);
            sys::sceGuEnable(GuState::ScissorTest);
            sys::sceGuDepthFunc(DepthFunc::GreaterOrEqual);
            sys::sceGuEnable(GuState::DepthTest);
            sys::sceGuDepthMask(0); // 0 = depth write enabled on PSP GE
            sys::sceGuFrontFace(FrontFaceDirection::CounterClockwise);
            sys::sceGuDisable(GuState::CullFace);
            sys::sceGuDisable(GuState::Lighting);
            sys::sceGuDisable(GuState::Texture2D);
            sys::sceGuShadeModel(ShadingModel::Smooth);
            sys::sceGuEnable(GuState::ClipPlanes);
            sys::sceGuFinish();
            sys::sceGuSync(GuSyncMode::Finish, GuSyncBehavior::Wait);
            let _ = sys::sceDisplaySetMode(
                DisplayMode::Lcd,
                PSP_SCREEN_WIDTH,
                PSP_SCREEN_HEIGHT,
            );
            let _ = sys::sceDisplaySetFrameBuf(
                self.fb_abs_ptr(1) as *const u8,
                PSP_BUF_WIDTH,
                DisplayPixelFormat::Psm5650,
                DisplaySetBufSync::NextFrame,
            );
            sys::sceGuDisplay(true);
        }
    }

    pub fn begin_frame(&mut self, clear_color_abgr: u32, proj: &Mat4, view: &Mat4) {
        self.dyn_vtx_cursor = 0;
        let draw_slot = self.frame_idx % 3;
        unsafe {
            sys::sceGuStart(GuContextType::Direct, self.direct_list_ptr());
            sys::sceGuDrawBufferList(
                DisplayPixelFormat::Psm5650,
                self.fb_rel_ptr(draw_slot),
                PSP_BUF_WIDTH as i32,
            );
            // Disable scissor & enable depth writes BEFORE clearing so stale Z-target scissors
            // never skip border rows and Z-buffer resets cleanly to 0 across all 480x272 pixels.
            sys::sceGuDisable(GuState::ScissorTest);
            sys::sceGuDepthMask(0);
            sys::sceGuClearColor(clear_color_abgr);
            sys::sceGuClearDepth(0);
            sys::sceGuClear(ClearBuffer::COLOR_BUFFER_BIT | ClearBuffer::DEPTH_BUFFER_BIT);

            sys::sceGuScissor(0, 0, PSP_SCREEN_WIDTH as i32, PSP_SCREEN_HEIGHT as i32);
            sys::sceGuEnable(GuState::ScissorTest);
            sys::sceGuDepthFunc(DepthFunc::GreaterOrEqual);
            sys::sceGuEnable(GuState::DepthTest);
            sys::sceGuDepthOffset(0);
            sys::sceGuDisable(GuState::Texture2D);
            sys::sceGuDisable(GuState::CullFace);
            sys::sceGuDisable(GuState::Blend);

            self.set_matrices_inner(proj, view);
        }
    }

    /// Update Projection & View matrices cleanly mid-frame (e.g., 3D perspective -> 2D HUD ortho).
    pub fn set_matrices(&self, proj: &Mat4, view: &Mat4) {
        unsafe {
            self.set_matrices_inner(proj, view);
        }
    }

    unsafe fn set_matrices_inner(&self, proj: &Mat4, view: &Mat4) {
        unsafe {
            sys::sceGumMatrixMode(MatrixMode::Projection);
            sys::sceGumLoadMatrix(&mat4_to_psp(proj));
            sys::sceGumMatrixMode(MatrixMode::View);
            sys::sceGumLoadMatrix(&mat4_to_psp(view));
            sys::sceGumMatrixMode(MatrixMode::Model);
            sys::sceGumLoadIdentity();
        }
    }

    pub fn end_frame_and_swap(&mut self, wait_vblank: bool) {
        let drawn_slot = self.frame_idx % 3;
        unsafe {
            sys::sceGuFinish();
            sys::sceGuSync(GuSyncMode::Finish, GuSyncBehavior::Wait);
            if wait_vblank {
                sys::sceDisplayWaitVblankStart();
            }
            // Explicit triple-buffer scanout matching `draw_slot` (fixes 2-buffer `sceGuSwapBuffers` drift).
            let _ = sys::sceDisplaySetFrameBuf(
                self.fb_abs_ptr(drawn_slot) as *const u8,
                PSP_BUF_WIDTH,
                DisplayPixelFormat::Psm5650,
                if wait_vblank {
                    DisplaySetBufSync::NextFrame
                } else {
                    DisplaySetBufSync::Immediate
                },
            );
        }
        self.frame_idx += 1;
    }

    pub fn shutdown(&self) {
        unsafe {
            sys::sceGuTerm();
        }
    }

    fn alloc_in_static(&mut self, bytes: usize, align: usize) -> Option<*mut u8> {
        let aligned = (self.static_cursor + align - 1) & !(align - 1);
        let end = aligned + bytes;
        if end > STATIC_CACHE_OFFSET + STATIC_CACHE_BYTES {
            return None;
        }
        self.static_cursor = end;
        Some(unsafe { self.pool_base.add(aligned) })
    }

    fn alloc_in_tex(&mut self, bytes: usize) -> Option<*mut u8> {
        let aligned = (self.tex_cursor + 63) & !63;
        let mut end = aligned + bytes;
        if end > TEX_CACHE_OFFSET + TEX_CACHE_BYTES {
            if bytes > TEX_CACHE_BYTES {
                return None;
            }
            // Flush TMU and recycle texture cache cleanly rather than dropping textures
            unsafe { sys::sceGuTexFlush() };
            self.textures.clear();
            self.tex_cursor = TEX_CACHE_OFFSET;
            end = TEX_CACHE_OFFSET + bytes;
            self.tex_cursor = end;
            return Some(unsafe { self.pool_base.add(TEX_CACHE_OFFSET) });
        }
        self.tex_cursor = end;
        Some(unsafe { self.pool_base.add(aligned) })
    }

    fn alloc_dyn_vtx(&mut self, bytes: usize) -> Option<*mut u8> {
        let half_base = DYN_VTX_OFFSET + (self.frame_idx & 1) * DYN_VTX_HALF_BYTES;
        let aligned = (self.dyn_vtx_cursor + 15) & !15;
        let end = aligned + bytes;
        if end > DYN_VTX_HALF_BYTES {
            return None;
        }
        self.dyn_vtx_cursor = end;
        Some(unsafe { self.pool_base.add(half_base + aligned) })
    }

    fn map_psm(psm: u8) -> TexturePixelFormat {
        match psm {
            PmmTexHeader::PSM_5650 => TexturePixelFormat::Psm5650,
            PmmTexHeader::PSM_5551 => TexturePixelFormat::Psm5551,
            PmmTexHeader::PSM_4444 => TexturePixelFormat::Psm4444,
            PmmTexHeader::PSM_T4 => TexturePixelFormat::PsmT4,
            PmmTexHeader::PSM_T8 => TexturePixelFormat::PsmT8,
            _ => TexturePixelFormat::Psm8888,
        }
    }

    fn bind_cached_tex(&self, existing: &CachedTexture) {
        unsafe {
            sys::sceGuTexMode(existing.psm, 0, 0, existing.swizzled);
            sys::sceGuTexImage(
                MipmapLevel::None,
                existing.width,
                existing.height,
                existing.width,
                existing.ptr as *const c_void,
            );
            sys::sceGuTexFunc(TextureEffect::Modulate, TextureColorComponent::Rgba);
            sys::sceGuTexFilter(TextureFilter::Linear, TextureFilter::Linear);
            sys::sceGuTexFlush();
        }
    }
}

impl GfxBackend for PspGfxBackend {
    fn set_texture_enabled(&mut self, enabled: bool) {
        unsafe {
            if enabled {
                sys::sceGuEnable(GuState::Texture2D);
            } else {
                sys::sceGuDisable(GuState::Texture2D);
            }
        }
    }

    fn set_tex_sampler(&mut self, clamp_s: bool, clamp_t: bool, nearest: bool) {
        let u = if clamp_s { GuTexWrapMode::Clamp } else { GuTexWrapMode::Repeat };
        let v = if clamp_t { GuTexWrapMode::Clamp } else { GuTexWrapMode::Repeat };
        let min_f = if nearest { TextureFilter::Nearest } else { TextureFilter::Linear };
        let mag_f = if nearest { TextureFilter::Nearest } else { TextureFilter::Linear };
        unsafe {
            sys::sceGuTexWrap(u, v);
            sys::sceGuTexFilter(min_f, mag_f);
        }
    }

    fn set_cull_mode(&mut self, cull_back: bool, cull_front: bool) {
        unsafe {
            if cull_front && !cull_back {
                sys::sceGuFrontFace(FrontFaceDirection::Clockwise);
                sys::sceGuEnable(GuState::CullFace);
            } else if cull_back {
                sys::sceGuFrontFace(FrontFaceDirection::CounterClockwise);
                sys::sceGuEnable(GuState::CullFace);
            } else {
                sys::sceGuDisable(GuState::CullFace);
            }
        }
    }

    fn set_prim_env_tint(&mut self, tint_abgr: u32) {
        unsafe {
            sys::sceGuTexEnvColor(tint_abgr);
        }
    }

    fn set_scissor(&mut self, x0: i32, y0: i32, x1: i32, y1: i32) {
        let sx0 = x0.clamp(0, PSP_SCREEN_WIDTH as i32);
        let sy0 = y0.clamp(0, PSP_SCREEN_HEIGHT as i32);
        let sx1 = x1.clamp(sx0, PSP_SCREEN_WIDTH as i32);
        let sy1 = y1.clamp(sy0, PSP_SCREEN_HEIGHT as i32);
        unsafe {
            sys::sceGuScissor(sx0, sy0, sx1, sy1);
            sys::sceGuEnable(GuState::ScissorTest);
        }
    }

    fn upload_rgba8888_texture(&mut self, tex_key: u64, w: u16, h: u16, rgba: &[u8]) -> bool {
        if let Some(existing) = self.textures.iter().find(|t| t.key == tex_key) {
            self.bind_cached_tex(existing);
            return false;
        }
        let Some(dst) = self.alloc_in_tex(rgba.len()) else {
            return false;
        };
        unsafe {
            ptr::copy_nonoverlapping(rgba.as_ptr(), dst, rgba.len());
            sceKernelDcacheWritebackInvalidateRange(dst as *const c_void, rgba.len() as u32);
        }
        let cached = CachedTexture {
            key: tex_key,
            ptr: dst,
            width: w as i32,
            height: h as i32,
            psm: TexturePixelFormat::Psm8888,
            swizzled: 0,
        };
        self.bind_cached_tex(&cached);
        self.textures.push(cached);
        true
    }

    fn bind_texture(&mut self, tex_id: ResourceId, hdr: &PmmTexHeader, payload: &[u8]) -> bool {
        if let Some(existing) = self.textures.iter().find(|t| t.key == tex_id.0) {
            self.bind_cached_tex(existing);
            return false;
        }

        let pix_len = (hdr.pixel_bytes as usize).min(payload.len());
        let Some(dst) = self.alloc_in_tex(pix_len) else {
            return false;
        };
        unsafe {
            ptr::copy_nonoverlapping(payload.as_ptr(), dst, pix_len);
            sceKernelDcacheWritebackInvalidateRange(dst as *const c_void, pix_len as u32);
        }

        let psm = Self::map_psm(hdr.psp_psm);
        let swizzled = hdr.swizzled as i32;
        let w = hdr.width as i32;
        let h = hdr.height as i32;

        let cached = CachedTexture {
            key: tex_id.0,
            ptr: dst,
            width: w,
            height: h,
            psm,
            swizzled,
        };
        self.bind_cached_tex(&cached);
        self.textures.push(cached);
        true
    }

    fn set_depth_mode(&mut self, mode: DepthMode) {
        unsafe {
            if mode.test {
                sys::sceGuEnable(GuState::DepthTest);
                sys::sceGuDepthFunc(DepthFunc::GreaterOrEqual);
            } else {
                sys::sceGuDisable(GuState::DepthTest);
            }
            // PSP GE: `0` = depth writes enabled, `1` = depth writes masked/disabled
            sys::sceGuDepthMask(if mode.write { 0 } else { 1 });
            sys::sceGuDepthOffset(mode.bias * 16);
        }
    }

    fn set_blend_xlu(&mut self, xlu: bool) {
        unsafe {
            if xlu {
                sys::sceGuEnable(GuState::Blend);
                sys::sceGuBlendFunc(
                    BlendOp::Add,
                    BlendFactor::SrcAlpha,
                    BlendFactor::OneMinusSrcAlpha,
                    0,
                    0,
                );
                sys::sceGuDisable(GuState::AlphaTest);
            } else {
                sys::sceGuDisable(GuState::Blend);
                sys::sceGuEnable(GuState::AlphaTest);
                sys::sceGuAlphaFunc(AlphaFunc::Greater, 0x08, 0xFF);
            }
        }
    }

    fn set_model_matrix(&mut self, mtx: &Mat4) {
        unsafe {
            sys::sceGumMatrixMode(MatrixMode::Model);
            sys::sceGumLoadMatrix(&mat4_to_psp(mtx));
        }
    }

    fn draw_triangles(&mut self, verts: &[PspVertex]) {
        if verts.is_empty() {
            return;
        }
        let bytes = core::mem::size_of_val(verts);
        let ptr = if self.compiling_list_start.is_some() {
            self.alloc_in_static(bytes, 16)
        } else {
            self.alloc_dyn_vtx(bytes)
        };
        let Some(dst) = ptr else {
            return;
        };
        unsafe {
            ptr::copy_nonoverlapping(verts.as_ptr() as *const u8, dst, bytes);
            sceKernelDcacheWritebackInvalidateRange(dst as *const c_void, bytes as u32);
            sys::sceGumDrawArray(
                GuPrimitive::Triangles,
                VertexType::TEXTURE_32BITF
                    | VertexType::COLOR_8888
                    | VertexType::VERTEX_32BITF
                    | VertexType::TRANSFORM_3D,
                verts.len() as i32,
                ptr::null_mut(),
                dst as *const c_void,
            );
        }
    }

    fn call_static_list(&mut self, list_id: u64) -> bool {
        if let Some(found) = self.static_lists.iter().find(|l| l.id == list_id) {
            unsafe {
                sys::sceGuCallList(found.list_ptr);
            }
            true
        } else {
            false
        }
    }

    fn begin_compile_static_list(&mut self, _list_id: u64) -> bool {
        const CMD_RESERVE: usize = 8 * 1024;
        let aligned = (self.static_cursor + 63) & !63;
        if aligned + CMD_RESERVE + 16 * 1024 > STATIC_CACHE_OFFSET + STATIC_CACHE_BYTES {
            return false;
        }
        let ptr = unsafe { self.pool_base.add(aligned) };
        // Advance `static_cursor` past the reserved 8 KiB command buffer so vertices
        // written via `alloc_in_static` never overlap the active GE Call list!
        self.static_cursor = aligned + CMD_RESERVE;
        self.compiling_list_start = Some(ptr);
        unsafe {
            sys::sceGuStart(GuContextType::Call, ptr as *mut c_void);
        }
        true
    }

    fn end_compile_static_list(&mut self, list_id: u64) {
        let Some(start) = self.compiling_list_start.take() else {
            return;
        };
        let written = unsafe { sys::sceGuFinish() }.max(0) as usize;
        let clamped_written = written.min(8 * 1024);
        unsafe {
            sceKernelDcacheWritebackInvalidateRange(start as *const c_void, clamped_written as u32);
        }
        self.static_lists.push(CachedStaticList {
            id: list_id,
            list_ptr: start as *const c_void,
        });
    }

    fn capture_motion_blur(&mut self) {
        let cur_fb = self.fb_abs_ptr(self.frame_idx);
        let blur_dst =
            unsafe { self.edram_base.add(self.vram_layout.blur_history_offset) as *mut c_void };
        unsafe {
            sys::sceGuCopyImage(
                DisplayPixelFormat::Psm5650,
                0,
                0,
                PSP_SCREEN_WIDTH as i32,
                PSP_SCREEN_HEIGHT as i32,
                PSP_BUF_WIDTH as i32,
                cur_fb,
                0,
                0,
                PSP_BUF_WIDTH as i32,
                blur_dst,
            );
            sys::sceGuTexSync();
        }
    }
}

fn mat4_to_psp(m: &Mat4) -> ScePspFMatrix4 {
    use psp::sys::ScePspFVector4;
    ScePspFMatrix4 {
        x: ScePspFVector4 { x: m[0][0], y: m[0][1], z: m[0][2], w: m[0][3] },
        y: ScePspFVector4 { x: m[1][0], y: m[1][1], z: m[1][2], w: m[1][3] },
        z: ScePspFVector4 { x: m[2][0], y: m[2][1], z: m[2][2], w: m[2][3] },
        w: ScePspFVector4 { x: m[3][0], y: m[3][1], z: m[3][2], w: m[3][3] },
    }
}
