#![no_std]

extern crate alloc;

use core::fmt;

/// 2S2H-compatible FourCC resource type identifiers
/// (derived from `majora_code/mm/2s2h/resource/type/2shResourceType.h`).
pub mod fourcc {
    pub const OARR: u32 = 0x4F41_5252; // Array
    pub const OANM: u32 = 0x4F41_4E4D; // Animation
    pub const OPAM: u32 = 0x4F50_414D; // PlayerAnimation
    pub const OROM: u32 = 0x4F52_4F4D; // Scene / Room
    pub const OCOL: u32 = 0x4F43_4F4C; // CollisionHeader
    pub const OSKL: u32 = 0x4F53_4B4C; // Skeleton
    pub const OSLB: u32 = 0x4F53_4C42; // SkeletonLimb
    pub const OPTH: u32 = 0x4F50_5448; // Path
    pub const OCUT: u32 = 0x4F43_5654; // Cutscene
    pub const OTXT: u32 = 0x4F54_5854; // Text
    pub const OTXM: u32 = 0x4F54_584D; // TextMM
    pub const OAUD: u32 = 0x4F41_5544; // Audio
    pub const OSMP: u32 = 0x4F53_4D50; // AudioSample
    pub const OSFT: u32 = 0x4F53_4654; // AudioSoundFont
    pub const OSEQ: u32 = 0x4F53_4551; // AudioSequence
    pub const OBGI: u32 = 0x4F42_4749; // Background
    pub const ORCM: u32 = 0x4F52_434D; // SceneCommand
    pub const OTAN: u32 = 0x4F54_414E; // TexAnim
    pub const OKFA: u32 = 0x4F4B_4641; // CKeyFrameAnim
    pub const OKFS: u32 = 0x4F4B_4653; // CKeyFrameSkel

    // PocketMajora packed specializations
    pub const ODLT: u32 = 0x4F44_4C54; // DisplayList
    pub const OTEX: u32 = 0x4F54_4558; // Texture
    pub const OVTX: u32 = 0x4F56_5458; // Vertex buffer
    pub const OMTX: u32 = 0x4F4D_5458; // Matrix
}

/// 64-bit deterministic resource identifier derived from normalized `__OTR__` paths.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct ResourceId(pub u64);

impl ResourceId {
    /// Compute FNV-1a 64-bit hash over normalized resource path bytes,
    /// stripping a leading `"__OTR__"` prefix if present.
    pub const fn from_otr_path(path: &str) -> Self {
        let bytes = path.as_bytes();
        let prefix = b"__OTR__";
        let start = if bytes.len() >= prefix.len()
            && bytes[0] == prefix[0]
            && bytes[1] == prefix[1]
            && bytes[2] == prefix[2]
            && bytes[3] == prefix[3]
            && bytes[4] == prefix[4]
            && bytes[5] == prefix[5]
            && bytes[6] == prefix[6]
        {
            prefix.len()
        } else {
            0
        };
        Self(fnv1a_64_slice(bytes, start))
    }
}

impl fmt::Debug for ResourceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ResourceId({:#018x})", self.0)
    }
}

/// Strip `"__OTR__"` prefix if present.
pub fn normalize_otr_path(path: &str) -> &str {
    path.strip_prefix("__OTR__").unwrap_or(path)
}

/// Deterministic `const fn` FNV-1a 64-bit hash.
pub const fn fnv1a_64(bytes: &[u8]) -> u64 {
    fnv1a_64_slice(bytes, 0)
}

const fn fnv1a_64_slice(bytes: &[u8], start: usize) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0100_0000_01b3;
    let mut hash = FNV_OFFSET;
    let mut i = start;
    while i < bytes.len() {
        hash ^= bytes[i] as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
        i += 1;
    }
    hash
}

/// Standard IEEE 802.3 CRC32 (`0xEDB88320` reflected polynomial).
pub const fn crc32_ieee(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    let mut i = 0;
    while i < bytes.len() {
        crc ^= bytes[i] as u32;
        let mut bit = 0;
        while bit < 8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320u32 & mask);
            bit += 1;
        }
        i += 1;
    }
    !crc
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaError {
    BufferTooSmall,
    InvalidMagic,
    UnsupportedVersion(u32),
    UnalignedPayloadOffset(u32),
    InvalidAlignment(u32),
    CorruptLayout,
}

impl fmt::Display for SchemaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BufferTooSmall => f.write_str("buffer too small"),
            Self::InvalidMagic => f.write_str("invalid pmm_2s2h.pak magic"),
            Self::UnsupportedVersion(v) => write!(f, "unsupported pak version {v}"),
            Self::UnalignedPayloadOffset(off) => write!(f, "unaligned payload offset {off:#x}"),
            Self::InvalidAlignment(a) => write!(f, "invalid entry alignment {a}"),
            Self::CorruptLayout => f.write_str("corrupt pak header layout"),
        }
    }
}

pub const PAK_MAGIC: [u8; 8] = *b"PMM2S2H\x01";
pub const PAK_VERSION: u32 = 1;
pub const PAK_HEADER_SIZE: usize = 64;
pub const PAK_ENTRY_SIZE: usize = 32;

/// 64-byte little-endian header at offset 0 of `pmm_2s2h.pak`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct PmmPakHeader {
    pub magic: [u8; 8],
    pub version: u32,
    pub entry_count: u32,
    pub index_offset: u32,
    pub data_offset: u32,
    pub rom_crc32: u32,
    pub flags: u32,
    pub total_size: u32,
    pub reserved: [u8; 28],
}

impl PmmPakHeader {
    pub const fn new(
        entry_count: u32,
        index_offset: u32,
        data_offset: u32,
        rom_crc32: u32,
        flags: u32,
        total_size: u32,
    ) -> Self {
        Self {
            magic: PAK_MAGIC,
            version: PAK_VERSION,
            entry_count,
            index_offset,
            data_offset,
            rom_crc32,
            flags,
            total_size,
            reserved: [0u8; 28],
        }
    }

    pub fn to_le_bytes(&self) -> [u8; PAK_HEADER_SIZE] {
        let mut out = [0u8; PAK_HEADER_SIZE];
        out[0..8].copy_from_slice(&self.magic);
        out[8..12].copy_from_slice(&self.version.to_le_bytes());
        out[12..16].copy_from_slice(&self.entry_count.to_le_bytes());
        out[16..20].copy_from_slice(&self.index_offset.to_le_bytes());
        out[20..24].copy_from_slice(&self.data_offset.to_le_bytes());
        out[24..28].copy_from_slice(&self.rom_crc32.to_le_bytes());
        out[28..32].copy_from_slice(&self.flags.to_le_bytes());
        out[32..36].copy_from_slice(&self.total_size.to_le_bytes());
        out[36..64].copy_from_slice(&self.reserved);
        out
    }

    pub fn from_le_bytes(buf: &[u8]) -> Result<Self, SchemaError> {
        if buf.len() < PAK_HEADER_SIZE {
            return Err(SchemaError::BufferTooSmall);
        }
        let mut magic = [0u8; 8];
        magic.copy_from_slice(&buf[0..8]);
        if magic != PAK_MAGIC {
            return Err(SchemaError::InvalidMagic);
        }
        let version = u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]);
        if version != PAK_VERSION {
            return Err(SchemaError::UnsupportedVersion(version));
        }
        let entry_count = u32::from_le_bytes([buf[12], buf[13], buf[14], buf[15]]);
        let index_offset = u32::from_le_bytes([buf[16], buf[17], buf[18], buf[19]]);
        let data_offset = u32::from_le_bytes([buf[20], buf[21], buf[22], buf[23]]);
        let rom_crc32 = u32::from_le_bytes([buf[24], buf[25], buf[26], buf[27]]);
        let flags = u32::from_le_bytes([buf[28], buf[29], buf[30], buf[31]]);
        let total_size = u32::from_le_bytes([buf[32], buf[33], buf[34], buf[35]]);
        if (index_offset as usize) < PAK_HEADER_SIZE || data_offset < index_offset {
            return Err(SchemaError::CorruptLayout);
        }
        if !data_offset.is_multiple_of(16) {
            return Err(SchemaError::UnalignedPayloadOffset(data_offset));
        }
        let mut reserved = [0u8; 28];
        reserved.copy_from_slice(&buf[36..64]);
        Ok(Self {
            magic,
            version,
            entry_count,
            index_offset,
            data_offset,
            rom_crc32,
            flags,
            total_size,
            reserved,
        })
    }
}

/// 32-byte index entry inside `pmm_2s2h.pak` (sorted ascending by `id`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct PmmPakEntry {
    pub id: u64,
    pub fourcc: u32,
    pub flags: u32,
    pub offset: u32,
    pub size: u32,
    pub uncompressed_size: u32,
    pub align: u32,
}

impl PmmPakEntry {
    pub const FLAG_PSP_SWIZZLED: u32 = 1 << 0;
    pub const FLAG_COMPRESSED: u32 = 1 << 1;
    pub const FLAG_PINNED_COMMON: u32 = 1 << 2;

    pub fn to_le_bytes(&self) -> [u8; PAK_ENTRY_SIZE] {
        let mut out = [0u8; PAK_ENTRY_SIZE];
        out[0..8].copy_from_slice(&self.id.to_le_bytes());
        out[8..12].copy_from_slice(&self.fourcc.to_le_bytes());
        out[12..16].copy_from_slice(&self.flags.to_le_bytes());
        out[16..20].copy_from_slice(&self.offset.to_le_bytes());
        out[20..24].copy_from_slice(&self.size.to_le_bytes());
        out[24..28].copy_from_slice(&self.uncompressed_size.to_le_bytes());
        out[28..32].copy_from_slice(&self.align.to_le_bytes());
        out
    }

    pub fn from_le_bytes(buf: &[u8]) -> Result<Self, SchemaError> {
        if buf.len() < PAK_ENTRY_SIZE {
            return Err(SchemaError::BufferTooSmall);
        }
        let id = u64::from_le_bytes([
            buf[0], buf[1], buf[2], buf[3], buf[4], buf[5], buf[6], buf[7],
        ]);
        let fourcc = u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]);
        let flags = u32::from_le_bytes([buf[12], buf[13], buf[14], buf[15]]);
        let offset = u32::from_le_bytes([buf[16], buf[17], buf[18], buf[19]]);
        let size = u32::from_le_bytes([buf[20], buf[21], buf[22], buf[23]]);
        let uncompressed_size = u32::from_le_bytes([buf[24], buf[25], buf[26], buf[27]]);
        let align = u32::from_le_bytes([buf[28], buf[29], buf[30], buf[31]]);

        if align < 16 || !align.is_power_of_two() {
            return Err(SchemaError::InvalidAlignment(align));
        }
        if !offset.is_multiple_of(16) {
            return Err(SchemaError::UnalignedPayloadOffset(offset));
        }

        Ok(Self {
            id,
            fourcc,
            flags,
            offset,
            size,
            uncompressed_size,
            align,
        })
    }
}

/// F3DEX2 GBI opcodes used by Majora's Mask (`GBI_UCODE = F3DEX_GBI_2`).
pub mod gbi {
    pub const G_NOOP: u8 = 0x00;
    pub const G_VTX: u8 = 0x01;
    pub const G_MODIFYVTX: u8 = 0x02;
    pub const G_CULLDL: u8 = 0x03;
    pub const G_BRANCH_Z: u8 = 0x04;
    pub const G_TRI1: u8 = 0x05;
    pub const G_TRI2: u8 = 0x06;
    pub const G_QUAD: u8 = 0x07;
    pub const G_SPECIAL_3: u8 = 0xD3;
    pub const G_SPECIAL_2: u8 = 0xD4;
    pub const G_SPECIAL_1: u8 = 0xD5;
    pub const G_DMA_IO: u8 = 0xD6;
    pub const G_TEXTURE: u8 = 0xD7;
    pub const G_POPMTX: u8 = 0xD8;
    pub const G_GEOMETRYMODE: u8 = 0xD9;
    pub const G_MTX: u8 = 0xDA;
    pub const G_MOVEWORD: u8 = 0xDB;
    pub const G_SEGMENT: u8 = 0xDB;
    pub const G_MOVEMEM: u8 = 0xDC;
    pub const G_LOAD_UCODE: u8 = 0xDD;
    pub const G_DL: u8 = 0xDE;
    pub const G_ENDDL: u8 = 0xDF;
    pub const G_SPNOOP: u8 = 0xE0;
    pub const G_RDPHALF_1: u8 = 0xE1;
    pub const G_SETOTHERMODE_L: u8 = 0xE2;
    pub const G_SETOTHERMODE_H: u8 = 0xE3;
    pub const G_TEXRECT: u8 = 0xE4;
    pub const G_TEXRECTFLIP: u8 = 0xE5;
    pub const G_RDPLOADSYNC: u8 = 0xE6;
    pub const G_RDPPIPESYNC: u8 = 0xE7;
    pub const G_RDPTILESYNC: u8 = 0xE8;
    pub const G_RDPFULLSYNC: u8 = 0xE9;
    pub const G_SETKEYGB: u8 = 0xEA;
    pub const G_SETKEYR: u8 = 0xEB;
    pub const G_SETCONVERT: u8 = 0xEC;
    pub const G_SETSCISSOR: u8 = 0xED;
    pub const G_SETPRIMDEPTH: u8 = 0xEE;
    pub const G_RDPSETOTHERMODE: u8 = 0xEF;
    pub const G_LOADTLUT: u8 = 0xF0;
    pub const G_RDPHALF_2: u8 = 0xF1;
    pub const G_SETTILESIZE: u8 = 0xF2;
    pub const G_LOADBLOCK: u8 = 0xF3;
    pub const G_LOADTILE: u8 = 0xF4;
    pub const G_SETTILE: u8 = 0xF5;
    pub const G_FILLRECT: u8 = 0xF6;
    pub const G_SETFILLCOLOR: u8 = 0xF7;
    pub const G_SETFOGCOLOR: u8 = 0xF8;
    pub const G_SETBLENDCOLOR: u8 = 0xF9;
    pub const G_SETPRIMCOLOR: u8 = 0xFA;
    pub const G_SETENVCOLOR: u8 = 0xFB;
    pub const G_SETCOMBINE: u8 = 0xFC;
    pub const G_SETIMG: u8 = 0xFD;
    pub const G_SETZIMG: u8 = 0xFE;
    pub const G_SETCIMG: u8 = 0xFF;

    // Matrix flags
    pub const G_MTX_MODELVIEW: u32 = 0x00;
    pub const G_MTX_PROJECTION: u32 = 0x04;
    pub const G_MTX_MUL: u32 = 0x00;
    pub const G_MTX_LOAD: u32 = 0x02;
    pub const G_MTX_NOPUSH: u32 = 0x00;
    pub const G_MTX_PUSH: u32 = 0x01;

    // G_MW_SEGMENT index
    pub const G_MW_SEGMENT: u8 = 0x06;

    // GeometryMode bits (`F3DEX2`)
    pub const G_ZBUFFER: u32 = 0x0000_0001;
    pub const G_SHADE: u32 = 0x0000_0004;
    pub const G_CULL_FRONT: u32 = 0x0000_0200;
    pub const G_CULL_BACK: u32 = 0x0000_0400;
    pub const G_FOG: u32 = 0x0001_0000;
    pub const G_LIGHTING: u32 = 0x0002_0000;
    pub const G_TEXTURE_GEN: u32 = 0x0004_0000;
    pub const G_SHADING_SMOOTH: u32 = 0x0020_0000;

    // Image formats & sizes (`G_SETTIMG` / `G_SETTILE`)
    pub const G_IM_FMT_RGBA: u8 = 0;
    pub const G_IM_FMT_YUV: u8 = 1;
    pub const G_IM_FMT_CI: u8 = 2;
    pub const G_IM_FMT_IA: u8 = 3;
    pub const G_IM_FMT_I: u8 = 4;

    pub const G_IM_SIZ_4B: u8 = 0;
    pub const G_IM_SIZ_8B: u8 = 1;
    pub const G_IM_SIZ_16B: u8 = 2;
    pub const G_IM_SIZ_32B: u8 = 3;

    pub const G_TX_MIRROR: u8 = 0x1;
    pub const G_TX_CLAMP: u8 = 0x2;
}

/// 64-bit F3DEX2 display-list command word in little-endian runtime format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(C)]
pub struct GfxWord {
    pub w0: u32,
    pub w1: u32,
}

impl GfxWord {
    pub const fn new(w0: u32, w1: u32) -> Self {
        Self { w0, w1 }
    }

    #[inline]
    pub const fn opcode(self) -> u8 {
        ((self.w0 >> 24) & 0xFF) as u8
    }

    #[inline]
    pub fn from_be_bytes(buf: &[u8]) -> Option<Self> {
        if buf.len() < 8 {
            return None;
        }
        Some(Self {
            w0: u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]),
            w1: u32::from_be_bytes([buf[4], buf[5], buf[6], buf[7]]),
        })
    }
}

/// 16-byte N64 F3DEX2 vertex (`Vtx`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(C, align(4))]
pub struct N64Vtx {
    pub ob: [i16; 3],
    pub flag: u16,
    pub tc: [i16; 2],
    pub cn: [u8; 4], // R, G, B, A (or normal x, y, z, a)
}

impl N64Vtx {
    #[inline]
    pub fn from_be_bytes(buf: &[u8]) -> Option<Self> {
        if buf.len() < 16 {
            return None;
        }
        Some(Self {
            ob: [
                i16::from_be_bytes([buf[0], buf[1]]),
                i16::from_be_bytes([buf[2], buf[3]]),
                i16::from_be_bytes([buf[4], buf[5]]),
            ],
            flag: u16::from_be_bytes([buf[6], buf[7]]),
            tc: [
                i16::from_be_bytes([buf[8], buf[9]]),
                i16::from_be_bytes([buf[10], buf[11]]),
            ],
            cn: [buf[12], buf[13], buf[14], buf[15]],
        })
    }
}

/// 64-byte N64 16.16 fixed-point matrix (`Mtx`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C, align(8))]
pub struct N64Mtx {
    pub integer: [[i16; 4]; 4],
    pub frac: [[u16; 4]; 4],
}

impl Default for N64Mtx {
    fn default() -> Self {
        Self::identity()
    }
}

impl N64Mtx {
    pub const fn identity() -> Self {
        Self {
            integer: [[1, 0, 0, 0], [0, 1, 0, 0], [0, 0, 1, 0], [0, 0, 0, 1]],
            frac: [[0; 4]; 4],
        }
    }

    /// Convert N64 16.16 fixed-point matrix to row-major `[[f32; 4]; 4]`.
    pub fn to_f32_4x4(&self) -> [[f32; 4]; 4] {
        let mut out = [[0.0f32; 4]; 4];
        let mut r = 0;
        while r < 4 {
            let mut c = 0;
            while c < 4 {
                let fixed = ((self.integer[r][c] as i32) << 16) | (self.frac[r][c] as i32);
                out[r][c] = (fixed as f32) * (1.0 / 65536.0);
                c += 1;
            }
            r += 1;
        }
        out
    }

    /// Construct an `N64Mtx` from row-major `[[f32; 4]; 4]`.
    pub fn from_f32_4x4(m: &[[f32; 4]; 4]) -> Self {
        let mut integer = [[0i16; 4]; 4];
        let mut frac = [[0u16; 4]; 4];
        let mut r = 0;
        while r < 4 {
            let mut c = 0;
            while c < 4 {
                let fixed = (m[r][c] * 65536.0) as i32;
                integer[r][c] = (fixed >> 16) as i16;
                frac[r][c] = (fixed & 0xFFFF) as u16;
                c += 1;
            }
            r += 1;
        }
        Self { integer, frac }
    }

    pub fn from_be_bytes(buf: &[u8]) -> Option<Self> {
        if buf.len() < 64 {
            return None;
        }
        let mut integer = [[0i16; 4]; 4];
        let mut frac = [[0u16; 4]; 4];
        for r in 0..4 {
            for c in 0..4 {
                let ioff = (r * 4 + c) * 2;
                let foff = 32 + ioff;
                integer[r][c] = i16::from_be_bytes([buf[ioff], buf[ioff + 1]]);
                frac[r][c] = u16::from_be_bytes([buf[foff], buf[foff + 1]]);
            }
        }
        Some(Self { integer, frac })
    }
}

/// Texture payload header stored inside `OTEX` entries of `pmm_2s2h.pak`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C, align(16))]
pub struct PmmTexHeader {
    pub width: u16,
    pub height: u16,
    pub psp_psm: u8,
    pub swizzled: u8,
    pub has_clut: u8,
    pub pad: u8,
    pub clut_bytes: u32,
    pub pixel_bytes: u32,
}

impl PmmTexHeader {
    pub const PSM_5650: u8 = 0;
    pub const PSM_5551: u8 = 1;
    pub const PSM_4444: u8 = 2;
    pub const PSM_8888: u8 = 3;
    pub const PSM_T4: u8 = 4;
    pub const PSM_T8: u8 = 5;

    pub fn to_le_bytes(&self) -> [u8; 16] {
        let mut out = [0u8; 16];
        out[0..2].copy_from_slice(&self.width.to_le_bytes());
        out[2..4].copy_from_slice(&self.height.to_le_bytes());
        out[4] = self.psp_psm;
        out[5] = self.swizzled;
        out[6] = self.has_clut;
        out[7] = self.pad;
        out[8..12].copy_from_slice(&self.clut_bytes.to_le_bytes());
        out[12..16].copy_from_slice(&self.pixel_bytes.to_le_bytes());
        out
    }

    pub fn from_le_bytes(buf: &[u8]) -> Result<Self, SchemaError> {
        if buf.len() < 16 {
            return Err(SchemaError::BufferTooSmall);
        }
        Ok(Self {
            width: u16::from_le_bytes([buf[0], buf[1]]),
            height: u16::from_le_bytes([buf[2], buf[3]]),
            psp_psm: buf[4],
            swizzled: buf[5],
            has_clut: buf[6],
            pad: buf[7],
            clut_bytes: u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]),
            pixel_bytes: u32::from_le_bytes([buf[12], buf[13], buf[14], buf[15]]),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::size_of;

    #[test]
    fn otr_path_normalization_matches_stripped_hash() {
        let with_prefix = ResourceId::from_otr_path("__OTR__objects/gameplay_keep/gBottleDL");
        let without_prefix = ResourceId::from_otr_path("objects/gameplay_keep/gBottleDL");
        assert_eq!(with_prefix, without_prefix);
        assert_ne!(with_prefix.0, 0);
        assert_eq!(
            normalize_otr_path("__OTR__scenes/nonmq/Z2_CLOCKTOWER/Z2_CLOCKTOWER"),
            "scenes/nonmq/Z2_CLOCKTOWER/Z2_CLOCKTOWER"
        );
    }

    #[test]
    fn crc32_ieee_known_vector() {
        assert_eq!(crc32_ieee(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32_ieee(b""), 0x0000_0000);
    }

    #[test]
    fn pak_header_roundtrip_and_negative_checks() {
        assert_eq!(size_of::<PmmPakHeader>(), PAK_HEADER_SIZE);
        let hdr = PmmPakHeader::new(42, 64, 64 + 42 * 32, 0xB428_D8A7, 0, 1024 * 1024);
        let bytes = hdr.to_le_bytes();
        let parsed = PmmPakHeader::from_le_bytes(&bytes).expect("valid header");
        assert_eq!(parsed, hdr);

        // Negative: bad magic
        let mut bad_magic = bytes;
        bad_magic[0] = b'X';
        assert_eq!(
            PmmPakHeader::from_le_bytes(&bad_magic),
            Err(SchemaError::InvalidMagic)
        );

        // Negative: unaligned data_offset
        let bad_off = PmmPakHeader::new(1, 64, 100, 0, 0, 256).to_le_bytes();
        assert_eq!(
            PmmPakHeader::from_le_bytes(&bad_off),
            Err(SchemaError::UnalignedPayloadOffset(100))
        );
    }

    #[test]
    fn pak_entry_roundtrip_and_alignment_negative_check() {
        assert_eq!(size_of::<PmmPakEntry>(), PAK_ENTRY_SIZE);
        let entry = PmmPakEntry {
            id: ResourceId::from_otr_path("__OTR__objects/gameplay_keep/gBottleDL").0,
            fourcc: fourcc::ODLT,
            flags: PmmPakEntry::FLAG_PSP_SWIZZLED,
            offset: 4096,
            size: 512,
            uncompressed_size: 512,
            align: 16,
        };
        let bytes = entry.to_le_bytes();
        assert_eq!(PmmPakEntry::from_le_bytes(&bytes), Ok(entry));

        let mut unaligned = entry;
        unaligned.offset = 4098;
        assert_eq!(
            PmmPakEntry::from_le_bytes(&unaligned.to_le_bytes()),
            Err(SchemaError::UnalignedPayloadOffset(4098))
        );

        let mut bad_align = entry;
        bad_align.align = 4;
        assert_eq!(
            PmmPakEntry::from_le_bytes(&bad_align.to_le_bytes()),
            Err(SchemaError::InvalidAlignment(4))
        );
    }

    #[test]
    fn gfx_word_vtx_mtx_sizes_and_opcodes() {
        assert_eq!(size_of::<GfxWord>(), 8);
        assert_eq!(size_of::<N64Vtx>(), 16);
        assert_eq!(size_of::<N64Mtx>(), 64);
        assert_eq!(size_of::<PmmTexHeader>(), 16);

        let dl_cmd = GfxWord::new(((gbi::G_DL as u32) << 24) | 0x0001_0000, 0x0600_1234);
        assert_eq!(dl_cmd.opcode(), gbi::G_DL);

        let end_cmd = GfxWord::new((gbi::G_ENDDL as u32) << 24, 0);
        assert_eq!(end_cmd.opcode(), gbi::G_ENDDL);
    }

    #[test]
    fn n64_mtx_fixed_point_conversion() {
        let mut m = [[0.0f32; 4]; 4];
        m[0][0] = 1.0;
        m[1][1] = 2.5;
        m[2][2] = -0.25;
        m[3][0] = 123.5;
        m[3][1] = -45.25;
        m[3][3] = 1.0;

        let n64 = N64Mtx::from_f32_4x4(&m);
        let back = n64.to_f32_4x4();
        assert!((back[0][0] - 1.0).abs() < 1e-4);
        assert!((back[1][1] - 2.5).abs() < 1e-4);
        assert!((back[2][2] + 0.25).abs() < 1e-4);
        assert!((back[3][0] - 123.5).abs() < 1e-4);
        assert!((back[3][1] + 45.25).abs() < 1e-4);
        assert!((back[3][3] - 1.0).abs() < 1e-4);
    }
}
