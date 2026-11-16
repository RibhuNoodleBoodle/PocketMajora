/// Swizzle linear texture bytes into PSP GE 16x8 byte tile layout (`GU_TRUE`).
/// When `row_bytes < 16` or `height < 8` or not aligned to 16x8, returns linear bytes
/// and `false` (not swizzled) so small textures remain valid on PSP GE.
pub fn swizzle_psp_texture(src: &[u8], row_bytes: usize, height: usize) -> (Vec<u8>, bool) {
    if row_bytes < 16 || height < 8 || !row_bytes.is_multiple_of(16) || !height.is_multiple_of(8) {
        return (src.to_vec(), false);
    }
    let expected = row_bytes * height;
    if src.len() < expected {
        return (src.to_vec(), false);
    }

    let mut dst = vec![0u8; expected];
    let block_x_count = row_bytes / 16;
    let block_y_count = height / 8;

    let mut dst_off = 0usize;
    for by in 0..block_y_count {
        for bx in 0..block_x_count {
            for row in 0..8 {
                let src_y = by * 8 + row;
                let src_off = src_y * row_bytes + bx * 16;
                dst[dst_off..dst_off + 16].copy_from_slice(&src[src_off..src_off + 16]);
                dst_off += 16;
            }
        }
    }
    (dst, true)
}

/// Inverse of `swizzle_psp_texture` for verification tests.
pub fn unswizzle_psp_texture(src: &[u8], row_bytes: usize, height: usize) -> Vec<u8> {
    let mut dst = vec![0u8; row_bytes * height];
    let block_x_count = row_bytes / 16;
    let block_y_count = height / 8;
    let mut src_off = 0usize;
    for by in 0..block_y_count {
        for bx in 0..block_x_count {
            for row in 0..8 {
                let dst_y = by * 8 + row;
                let dst_off = dst_y * row_bytes + bx * 16;
                dst[dst_off..dst_off + 16].copy_from_slice(&src[src_off..src_off + 16]);
                src_off += 16;
            }
        }
    }
    dst
}

/// Convert big-endian N64 RGBA16 (RRRRRGGGGGBBBBBA) to PSP little-endian 5551 (ABBBBBGGGGGRRRRR).
pub fn convert_n64_rgba16_to_psp5551(be_bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(be_bytes.len());
    for pair in be_bytes.chunks_exact(2) {
        let px = u16::from_be_bytes([pair[0], pair[1]]);
        let r = (px >> 11) & 0x1F;
        let g = (px >> 6) & 0x1F;
        let b = (px >> 1) & 0x1F;
        let a = px & 0x01;
        let psp_px = (a << 15) | (b << 10) | (g << 5) | r;
        out.extend_from_slice(&psp_px.to_le_bytes());
    }
    out
}
