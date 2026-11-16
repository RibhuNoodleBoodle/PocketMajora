use crate::swizzle::swizzle_psp_texture;
use pmm_schema::{
    PAK_ENTRY_SIZE, PAK_HEADER_SIZE, PmmPakEntry, PmmPakHeader, PmmTexHeader, ResourceId, fourcc,
    gbi, GfxWord, N64Mtx, N64Vtx,
};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct PendingAsset {
    pub id: u64,
    pub fourcc: u32,
    pub flags: u32,
    pub payload: Vec<u8>,
}

pub struct PakBuilder {
    rom_crc32: u32,
    items: Vec<PendingAsset>,
}

impl PakBuilder {
    pub fn new(rom_crc32: u32) -> Self {
        Self {
            rom_crc32,
            items: Vec::new(),
        }
    }

    pub fn push_path(&mut self, otr_path: &str, fourcc: u32, flags: u32, payload: Vec<u8>) {
        self.items.push(PendingAsset {
            id: ResourceId::from_otr_path(otr_path).0,
            fourcc,
            flags,
            payload,
        });
    }

    pub fn build_bytes(mut self) -> Vec<u8> {
        self.items.sort_by_key(|item| item.id);
        self.items.dedup_by_key(|item| item.id);

        let entry_count = self.items.len() as u32;
        let index_offset = PAK_HEADER_SIZE as u32;
        let raw_data_start = index_offset + entry_count * (PAK_ENTRY_SIZE as u32);
        let data_offset = align_up(raw_data_start, 16);

        let mut cursor = data_offset;
        let mut entries = Vec::with_capacity(self.items.len());
        for item in &self.items {
            cursor = align_up(cursor, 16);
            let len = item.payload.len() as u32;
            entries.push(PmmPakEntry {
                id: item.id,
                fourcc: item.fourcc,
                flags: item.flags,
                offset: cursor,
                size: len,
                uncompressed_size: len,
                align: 16,
            });
            cursor += len;
        }
        let total_size = align_up(cursor, 16);
        let header = PmmPakHeader::new(
            entry_count,
            index_offset,
            data_offset,
            self.rom_crc32,
            0,
            total_size,
        );

        let mut out = vec![0u8; total_size as usize];
        out[0..PAK_HEADER_SIZE].copy_from_slice(&header.to_le_bytes());

        for (i, entry) in entries.iter().enumerate() {
            let off = (index_offset as usize) + i * PAK_ENTRY_SIZE;
            out[off..off + PAK_ENTRY_SIZE].copy_from_slice(&entry.to_le_bytes());
        }
        for (item, entry) in self.items.iter().zip(entries.iter()) {
            let start = entry.offset as usize;
            let end = start + item.payload.len();
            out[start..end].copy_from_slice(&item.payload);
        }
        out
    }
}

fn align_up(v: u32, align: u32) -> u32 {
    (v + align - 1) & !(align - 1)
}

fn vtx_slice_to_bytes(verts: &[N64Vtx]) -> Vec<u8> {
    let mut out = Vec::with_capacity(verts.len() * 16);
    for v in verts {
        for coord in v.ob {
            out.extend_from_slice(&coord.to_le_bytes());
        }
        out.extend_from_slice(&v.flag.to_le_bytes());
        for tc in v.tc {
            out.extend_from_slice(&tc.to_le_bytes());
        }
        out.extend_from_slice(&v.cn);
    }
    out
}

fn gfx_slice_to_bytes(cmds: &[GfxWord]) -> Vec<u8> {
    let mut out = Vec::with_capacity(cmds.len() * 8);
    for c in cmds {
        out.extend_from_slice(&c.w0.to_le_bytes());
        out.extend_from_slice(&c.w1.to_le_bytes());
    }
    out
}

fn build_tex_payload(width: u16, height: u16, rgba32: &[u8]) -> (Vec<u8>, u32) {
    let row_bytes = (width as usize) * 4;
    let (pixels, swizzled) = swizzle_psp_texture(rgba32, row_bytes, height as usize);
    let hdr = PmmTexHeader {
        width,
        height,
        psp_psm: PmmTexHeader::PSM_8888,
        swizzled: if swizzled { 1 } else { 0 },
        has_clut: 0,
        pad: 0,
        clut_bytes: 0,
        pixel_bytes: pixels.len() as u32,
    };
    let mut payload = Vec::with_capacity(16 + pixels.len());
    payload.extend_from_slice(&hdr.to_le_bytes());
    payload.extend_from_slice(&pixels);
    let flags = if swizzled {
        PmmPakEntry::FLAG_PSP_SWIZZLED
    } else {
        0
    };
    (payload, flags)
}

/// Generate a non-copyrighted synthetic test `pmm_2s2h.pak` (`rom_crc32 = 0x54455354` "TEST")
/// exercising all 2S2H resource categories for ROM-less CI and PSP integration tests.
pub fn build_synthetic_fixture(out_path: &Path) -> Result<(usize, usize), String> {
    let mut pak = PakBuilder::new(0x5445_5354); // "TEST"

    // 1. Procedural 32x32 checkerboard texture (swizzled!)
    let mut checker = Vec::with_capacity(32 * 32 * 4);
    for y in 0..32u32 {
        for x in 0..32u32 {
            let light = ((x / 4) + (y / 4)) & 1 == 0;
            if light {
                checker.extend_from_slice(&[0xD8, 0xC8, 0xA8, 0xFF]);
            } else {
                checker.extend_from_slice(&[0x38, 0x58, 0x78, 0xFF]);
            }
        }
    }
    let (checker_payload, checker_flags) = build_tex_payload(32, 32, &checker);
    pak.push_path(
        "__OTR__textures/test/checker32",
        fourcc::OTEX,
        checker_flags | PmmPakEntry::FLAG_PINNED_COMMON,
        checker_payload,
    );

    // 2. Procedural 32x32 sunset/emblem gradient texture (swizzled!)
    let mut grad = Vec::with_capacity(32 * 32 * 4);
    for y in 0..32u32 {
        for x in 0..32u32 {
            grad.extend_from_slice(&[(x * 8) as u8, (y * 8) as u8, 0xB0, 0xFF]);
        }
    }
    let (grad_payload, grad_flags) = build_tex_payload(32, 32, &grad);
    pak.push_path(
        "__OTR__textures/test/gradient32",
        fourcc::OTEX,
        grad_flags | PmmPakEntry::FLAG_PINNED_COMMON,
        grad_payload,
    );

    // 3. Synthetic room0 floor + wall vertex buffer (8 vertices, 32x32 N64 world coords)
    let room_verts = [
        N64Vtx { ob: [-200, 0, -200], flag: 0, tc: [0, 0], cn: [255, 255, 255, 255] },
        N64Vtx { ob: [200, 0, -200], flag: 0, tc: [1024, 0], cn: [220, 220, 255, 255] },
        N64Vtx { ob: [200, 0, 200], flag: 0, tc: [1024, 1024], cn: [255, 255, 255, 255] },
        N64Vtx { ob: [-200, 0, 200], flag: 0, tc: [0, 1024], cn: [220, 255, 220, 255] },
        // Raised Clock Tower pedestal quad
        N64Vtx { ob: [-60, 40, -60], flag: 0, tc: [0, 0], cn: [255, 210, 120, 255] },
        N64Vtx { ob: [60, 40, -60], flag: 0, tc: [512, 0], cn: [255, 160, 90, 255] },
        N64Vtx { ob: [60, 40, 60], flag: 0, tc: [512, 512], cn: [200, 120, 255, 255] },
        N64Vtx { ob: [-60, 40, 60], flag: 0, tc: [0, 512], cn: [120, 200, 255, 255] },
    ];
    pak.push_path(
        "__OTR__scenes/test/room0_vtx",
        fourcc::OVTX,
        0,
        vtx_slice_to_bytes(&room_verts),
    );

    // 4. Synthetic room0 F3DEX2 display list referencing segment 0x06 (`room0_vtx`)
    //    and texture ID hash via G_SETIMG + G_VTX + G_TRI2 + G_ENDDL
    let tex_id = ResourceId::from_otr_path("__OTR__textures/test/checker32").0;
    let vtx_seg_addr = 0x0600_0000u32;
    let room_dl = [
        GfxWord::new((gbi::G_RDPPIPESYNC as u32) << 24, 0),
        GfxWord::new(
            ((gbi::G_SETIMG as u32) << 24) | 0x0018_001F,
            (tex_id & 0xFFFF_FFFF) as u32,
        ),
        GfxWord::new(
            ((gbi::G_SETTILE as u32) << 24) | 0x0000_0000,
            (tex_id >> 32) as u32,
        ),
        GfxWord::new((gbi::G_SETTILESIZE as u32) << 24, (31 << 14) | (31 << 2)),
        // Load 8 vertices into vtx[0..8]: w0 = (G_VTX << 24) | (n << 12) | (v0 + n)*2
        GfxWord::new(((gbi::G_VTX as u32) << 24) | (8 << 12) | (8 * 2), vtx_seg_addr),
        // Draw floor (0,1,2) and (0,2,3)
        GfxWord::new(
            ((gbi::G_TRI2 as u32) << 24) | (0 << 17) | (1 << 9) | (2 << 1),
            (0 << 17) | (2 << 9) | (3 << 1),
        ),
        // Draw pedestal (4,5,6) and (4,6,7)
        GfxWord::new(
            ((gbi::G_TRI2 as u32) << 24) | (4 << 17) | (5 << 9) | (6 << 1),
            (4 << 17) | (6 << 9) | (7 << 1),
        ),
        GfxWord::new((gbi::G_ENDDL as u32) << 24, 0),
    ];
    pak.push_path(
        "__OTR__scenes/test/room0_dl",
        fourcc::ODLT,
        0,
        gfx_slice_to_bytes(&room_dl),
    );

    // 5. Identity N64Mtx
    let mtx = N64Mtx::identity();
    let mut mtx_bytes = Vec::with_capacity(64);
    for row in mtx.integer {
        for v in row {
            mtx_bytes.extend_from_slice(&v.to_le_bytes());
        }
    }
    for row in mtx.frac {
        for v in row {
            mtx_bytes.extend_from_slice(&v.to_le_bytes());
        }
    }
    pak.push_path("__OTR__scenes/test/room0_mtx", fourcc::OMTX, 0, mtx_bytes);

    // 6. Synthetic TextMM message payload
    let msg = b"Dawn of the First Day -72 Hours Remain-\0";
    pak.push_path(
        "__OTR__text/message_data_static/message_data_static",
        fourcc::OTXM,
        PmmPakEntry::FLAG_PINNED_COMMON,
        msg.to_vec(),
    );

    // 7. Synthetic Scene / Room descriptors (`Z2_TOWN`, `Z2_INSIDETOWER`)
    pak.push_path(
        "__OTR__scenes/nonmq/Z2_TOWN/Z2_TOWN",
        fourcc::OROM,
        0,
        b"SCENE:Z2_TOWN:ROOMS=1:SPAWN=0,0,0\0".to_vec(),
    );
    pak.push_path(
        "__OTR__scenes/nonmq/Z2_TOWN/Z2_TOWN_room_0",
        fourcc::OROM,
        0,
        b"ROOM:Z2_TOWN_0:DL=scenes/test/room0_dl\0".to_vec(),
    );

    // 8. Synthetic Audio test tone PCM16 (32 kHz 440 Hz sine-like square/triangle cycle)
    let mut audio_pcm = Vec::with_capacity(640 * 2);
    for i in 0..640i32 {
        let phase = i % 72;
        let sample: i16 = if phase < 36 { 4000 } else { -4000 };
        audio_pcm.extend_from_slice(&sample.to_le_bytes());
    }
    pak.push_path(
        "__OTR__audio/test_sample",
        fourcc::OSMP,
        PmmPakEntry::FLAG_PINNED_COMMON,
        audio_pcm,
    );

    let entry_count = pak.items.len();
    let bytes = pak.build_bytes();
    if let Some(parent) = out_path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create_dir_all failed: {e}"))?;
    }
    fs::write(out_path, &bytes).map_err(|e| format!("write {} failed: {e}", out_path.display()))?;
    Ok((entry_count, bytes.len()))
}
