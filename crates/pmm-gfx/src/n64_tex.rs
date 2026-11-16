use alloc::vec;
use alloc::vec::Vec;
use pmm_schema::gbi;

#[inline]
pub fn next_pow2_u16(v: u16) -> u16 {
    let mut p = 8u16; // PSP GE minimum usable aligned width >= 8..16
    let target = v.max(1).min(512);
    while p < target && p < 512 {
        p <<= 1;
    }
    p
}

/// Convert N64 big-endian RGBA5551 texel to `[r, g, b, a]` bytes (PSP ABGR little-endian word).
#[inline]
pub fn rgba5551_to_rgba8888(px: u16) -> [u8; 4] {
    let r = (((px >> 11) & 0x1F) * 255 + 15) / 31;
    let g = (((px >> 6) & 0x1F) * 255 + 15) / 31;
    let b = (((px >> 1) & 0x1F) * 255 + 15) / 31;
    let a = if (px & 1) != 0 { 255 } else { 0 };
    [r as u8, g as u8, b as u8, a]
}

/// Decode N64 big-endian RGBA16 palette (`G_LOADTLUT`) into `out[..count]` ABGR `u32` entries.
pub fn decode_be_tlut(src: &[u8], count: usize, out: &mut [u32; 256]) {
    let n = count.min(256).min(src.len() / 2);
    for i in 0..n {
        let px = u16::from_be_bytes([src[i * 2], src[i * 2 + 1]]);
        let c = rgba5551_to_rgba8888(px);
        out[i] = u32::from_le_bytes(c);
    }
}

/// Decode big-endian N64 texture payload (`fmt`, `siz`, `width`, `height`) into a
/// power-of-2 padded `RGBA8888` byte buffer `[r, g, b, a, ...]` ready for PSP `Psm8888`.
pub fn decode_n64_texture_to_rgba8888(
    src: &[u8],
    fmt: u8,
    siz: u8,
    width: u16,
    height: u16,
    tlut: &[u32; 256],
) -> Option<(u16, u16, Vec<u8>)> {
    if width == 0 || height == 0 || width > 512 || height > 512 || src.is_empty() {
        return None;
    }
    let w = width as usize;
    let h = height as usize;
    let po2_w = next_pow2_u16(width) as usize;
    let po2_h = next_pow2_u16(height) as usize;
    let mut out = vec![0u8; po2_w * po2_h * 4];

    for y in 0..h {
        for x in 0..w {
            let idx = y * w + x;
            let rgba: [u8; 4] = match (fmt, siz) {
                (gbi::G_IM_FMT_RGBA, gbi::G_IM_SIZ_16B) => {
                    let b = idx * 2;
                    if b + 2 > src.len() {
                        [255, 255, 255, 255]
                    } else {
                        rgba5551_to_rgba8888(u16::from_be_bytes([src[b], src[b + 1]]))
                    }
                }
                (gbi::G_IM_FMT_RGBA, gbi::G_IM_SIZ_32B) => {
                    let b = idx * 4;
                    if b + 4 > src.len() {
                        [255, 255, 255, 255]
                    } else {
                        [src[b], src[b + 1], src[b + 2], src[b + 3]]
                    }
                }
                (gbi::G_IM_FMT_IA, gbi::G_IM_SIZ_16B) => {
                    let b = idx * 2;
                    if b + 2 > src.len() {
                        [255, 255, 255, 255]
                    } else {
                        let i = src[b];
                        let a = src[b + 1];
                        [i, i, i, a]
                    }
                }
                (gbi::G_IM_FMT_IA, gbi::G_IM_SIZ_8B) => {
                    if idx >= src.len() {
                        [255, 255, 255, 255]
                    } else {
                        let v = src[idx];
                        let i = ((v >> 4) & 0xF) * 17;
                        let a = (v & 0xF) * 17;
                        [i, i, i, a]
                    }
                }
                (gbi::G_IM_FMT_IA, gbi::G_IM_SIZ_4B) => {
                    let b = idx / 2;
                    if b >= src.len() {
                        [255, 255, 255, 255]
                    } else {
                        let nib = if (idx & 1) == 0 {
                            src[b] >> 4
                        } else {
                            src[b] & 0xF
                        };
                        let i = ((nib >> 1) & 0x7) * 255 / 7;
                        let a = if (nib & 1) != 0 { 255 } else { 0 };
                        [i, i, i, a]
                    }
                }
                (gbi::G_IM_FMT_I, gbi::G_IM_SIZ_8B) => {
                    if idx >= src.len() {
                        [255, 255, 255, 255]
                    } else {
                        let i = src[idx];
                        [i, i, i, i]
                    }
                }
                (gbi::G_IM_FMT_I, gbi::G_IM_SIZ_4B) => {
                    let b = idx / 2;
                    if b >= src.len() {
                        [255, 255, 255, 255]
                    } else {
                        let nib = if (idx & 1) == 0 {
                            src[b] >> 4
                        } else {
                            src[b] & 0xF
                        };
                        let i = nib * 17;
                        [i, i, i, i]
                    }
                }
                (gbi::G_IM_FMT_CI, gbi::G_IM_SIZ_8B) => {
                    if idx >= src.len() {
                        [255, 255, 255, 255]
                    } else {
                        tlut[src[idx] as usize].to_le_bytes()
                    }
                }
                (gbi::G_IM_FMT_CI, gbi::G_IM_SIZ_4B) => {
                    let b = idx / 2;
                    if b >= src.len() {
                        [255, 255, 255, 255]
                    } else {
                        let nib = if (idx & 1) == 0 {
                            src[b] >> 4
                        } else {
                            src[b] & 0xF
                        };
                        tlut[nib as usize].to_le_bytes()
                    }
                }
                _ => [255, 255, 255, 255],
            };
            let dst = (y * po2_w + x) * 4;
            out[dst..dst + 4].copy_from_slice(&rgba);
        }
    }

    // Clamp-fill padded right/bottom margins to eliminate bilinear halo borders
    if po2_w > w {
        for y in 0..h {
            let edge_dst = (y * po2_w + (w - 1)) * 4;
            let edge = [out[edge_dst], out[edge_dst + 1], out[edge_dst + 2], out[edge_dst + 3]];
            for x in w..po2_w {
                let d = (y * po2_w + x) * 4;
                out[d..d + 4].copy_from_slice(&edge);
            }
        }
    }
    if po2_h > h {
        let src_row_off = (h - 1) * po2_w * 4;
        for y in h..po2_h {
            let dst_off = y * po2_w * 4;
            out.copy_within(src_row_off..src_row_off + po2_w * 4, dst_off);
        }
    }

    Some((po2_w as u16, po2_h as u16, out))
}
