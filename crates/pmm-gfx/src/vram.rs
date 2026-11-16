pub const PSP_VRAM_TOTAL_BYTES: usize = 2 * 1024 * 1024;
pub const PSP_BUF_WIDTH: usize = 512;
pub const PSP_SCREEN_WIDTH: usize = 480;
pub const PSP_SCREEN_HEIGHT: usize = 272;
pub const FRAMEBUFFER_16B_BYTES: usize = PSP_BUF_WIDTH * PSP_SCREEN_HEIGHT * 2; // 278,528 bytes

/// Deterministic 2 MiB PSP eDRAM layout (triple-buffered 16-bit LCD + 16-bit Z +
/// GPU motion-blur history + ~688 KiB hot texture/TLUT scratch).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VramLayout {
    pub fb0_offset: usize,
    pub fb1_offset: usize,
    pub fb2_offset: usize,
    pub zbuf_offset: usize,
    pub blur_history_offset: usize,
    pub tex_scratch_offset: usize,
    pub tex_scratch_bytes: usize,
    pub total_bytes: usize,
}

impl VramLayout {
    pub const fn new_triple_buffered_lcd() -> Self {
        let fb0_offset = 0;
        let fb1_offset = fb0_offset + FRAMEBUFFER_16B_BYTES;
        let fb2_offset = fb1_offset + FRAMEBUFFER_16B_BYTES;
        let zbuf_offset = fb2_offset + FRAMEBUFFER_16B_BYTES;
        let blur_history_offset = zbuf_offset + FRAMEBUFFER_16B_BYTES;
        let tex_scratch_offset = blur_history_offset + FRAMEBUFFER_16B_BYTES;
        let tex_scratch_bytes = PSP_VRAM_TOTAL_BYTES - tex_scratch_offset;
        Self {
            fb0_offset,
            fb1_offset,
            fb2_offset,
            zbuf_offset,
            blur_history_offset,
            tex_scratch_offset,
            tex_scratch_bytes,
            total_bytes: PSP_VRAM_TOTAL_BYTES,
        }
    }
}
