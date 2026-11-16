use alloc::vec::Vec;
use core::ffi::c_void;
use core::ptr;
use pmm_gfx::math::Mat4;
use pmm_gfx::vram::{FRAMEBUFFER_16B_BYTES, PSP_BUF_WIDTH, PSP_SCREEN_HEIGHT, PSP_SCREEN_WIDTH, VramLayout};
use pmm_gfx::{DepthMode, GfxBackend, PspVertex};
use pmm_schema::{PmmTexHeader, ResourceId};
use psp::sys::{
    self, AlphaFunc, BlendFactor, BlendOp, ClearBuffer, DepthFunc, DisplayPixelFormat,
    FrontFaceDirection, GuContextType, GuPrimitive, GuState, GuSyncBehavior, GuSyncMode,
    MatrixMode, MipmapLevel, ScePspFMatrix4, ShadingModel, TextureColorComponent, TextureEffect,
    TextureFilter, TexturePixelFormat, VertexType, sceKernelDcacheWritebackInvalidateRange,
};

const DIRECT_LIST_BYTES: usize = 512 * 1024;
const STATIC_CACHE_OFFSET: usize = 2 * DIRECT_LIST_BYTES; // 1 MiB
const STATIC_CACHE_BYTES: usize = 2 * 1024 * 1024; // 2 MiB
const TEX_CACHE_OFFSET: usize = STATIC_CACHE_OFFSET + STATIC_CACHE_BYTES; // 3 MiB
const TEX_CACHE_BYTES: usize = 2 * 1024 * 1024 + 512 * 1024; // 2.5 MiB
const DYN_VTX_OFFSET: usize = TEX_CACHE_OFFSET + TEX_CACHE_BYTES; // 5.5 MiB
const DYN_VTX_HALF_BYTES: usize = 256 * 1024; // 2x 256 KiB = 512 KiB -> 6 MiB total

struct CachedTexture {
    id: ResourceId,
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
            textures: Vec::with_capacity(64),
            static_lists: Vec::with_capacity(64),
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
            sys::sceGuFrontFace(FrontFaceDirection::CounterClockwise);
            sys::sceGuShadeModel(ShadingModel::Smooth);
            sys::sceGuEnable(GuState::Texture2D);
            sys::sceGuEnable(GuState::ClipPlanes);
            sys::sceGuFinish();
            sys::sceGuSync(GuSyncMode::Finish, GuSyncBehavior::Wait);
            sys::sceDisplayWaitVblankStart();
            sys::sceGuDisplay(true);
        }
        // Clear unused blur buffer bytes cleanly
        let _ = FRAMEBUFFER_16B_BYTES;
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
            sys::sceGuClearColor(clear_color_abgr);
            sys::sceGuClearDepth(0);
            sys::sceGuClear(ClearBuffer::COLOR_BUFFER_BIT | ClearBuffer::DEPTH_BUFFER_BIT);

            sys::sceGumMatrixMode(MatrixMode::Projection);
            sys::sceGumLoadMatrix(&mat4_to_psp(proj));
            sys::sceGumMatrixMode(MatrixMode::View);
            sys::sceGumLoadMatrix(&mat4_to_psp(view));
            sys::sceGumMatrixMode(MatrixMode::Model);
            sys::sceGumLoadIdentity();
        }
    }

    pub fn end_frame_and_swap(&mut self, wait_vblank: bool) {
        unsafe {
            sys::sceGuFinish();
            sys::sceGuSync(GuSyncMode::Finish, GuSyncBehavior::Wait);
            if wait_vblank {
                sys::sceDisplayWaitVblankStart();
            }
            sys::sceGuSwapBuffers();
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
        let end = aligned + bytes;
        if end > TEX_CACHE_OFFSET + TEX_CACHE_BYTES {
            return None;
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
}

impl GfxBackend for PspGfxBackend {
    fn bind_texture(&mut self, tex_id: ResourceId, hdr: &PmmTexHeader, payload: &[u8]) -> bool {
        if let Some(existing) = self.textures.iter().find(|t| t.id == tex_id) {
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
            }
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

        unsafe {
            sys::sceGuTexMode(psm, 0, 0, swizzled);
            sys::sceGuTexImage(MipmapLevel::None, w, h, w, dst as *const c_void);
            sys::sceGuTexFunc(TextureEffect::Modulate, TextureColorComponent::Rgba);
            sys::sceGuTexFilter(TextureFilter::Linear, TextureFilter::Linear);
        }

        self.textures.push(CachedTexture {
            id: tex_id,
            ptr: dst,
            width: w,
            height: h,
            psm,
            swizzled,
        });
        true
    }

    fn set_depth_mode(&mut self, mode: DepthMode) {
        unsafe {
            if mode.test {
                sys::sceGuEnable(GuState::DepthTest);
            } else {
                sys::sceGuDisable(GuState::DepthTest);
            }
            sys::sceGuDepthMask(if mode.write { 0 } else { 1 });
            sys::sceGuDepthOffset(mode.bias);
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
        let aligned = (self.static_cursor + 63) & !63;
        if aligned + 16 * 1024 > STATIC_CACHE_OFFSET + STATIC_CACHE_BYTES {
            return false;
        }
        let ptr = unsafe { self.pool_base.add(aligned) };
        self.static_cursor = aligned;
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
        let aligned_end = (self.static_cursor + written + 63) & !63;
        self.static_cursor = aligned_end.min(STATIC_CACHE_OFFSET + STATIC_CACHE_BYTES);
        unsafe {
            sceKernelDcacheWritebackInvalidateRange(start as *const c_void, written as u32);
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
