use pmm_schema::crc32_ieee;

pub const US_COMPRESSED_SIZE: usize = 0x0200_0000; // 32 MiB
pub const US_DECOMPRESSED_SIZE: usize = 0x0400_0000; // 64 MiB
pub const US_COMPRESSED_CRC32: u32 = 0xB428_D8A7;
pub const US_DECOMPRESSED_SHA1: &str = "d6133ace5afaa0882cf214cf88daba39e266c078";
pub const DMADATA_OFFSET: usize = 0x1A500;
pub const DMADATA_COUNT: usize = 1552;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RomFormat {
    Z64, // Big-endian: 80 37 12 40
    V64, // Byte-swapped: 37 80 40 12
    N64, // Word-swapped LE: 40 12 37 80
}

#[derive(Debug, Clone)]
pub struct RomReport {
    pub format: RomFormat,
    pub size: usize,
    pub crc32: u32,
    pub sha1_hex: String,
    pub title: String,
    pub game_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DmaEntry {
    pub vrom_start: u32,
    pub vrom_end: u32,
    pub rom_start: u32,
    pub rom_end: u32,
}

pub fn detect_rom_format(bytes: &[u8]) -> Result<RomFormat, String> {
    if bytes.len() < 4 {
        return Err("ROM too small (< 4 bytes)".into());
    }
    match [bytes[0], bytes[1], bytes[2], bytes[3]] {
        [0x80, 0x37, 0x12, 0x40] => Ok(RomFormat::Z64),
        [0x37, 0x80, 0x40, 0x12] => Ok(RomFormat::V64),
        [0x40, 0x12, 0x37, 0x80] => Ok(RomFormat::N64),
        other => Err(format!(
            "unrecognized N64 ROM magic {:02x} {:02x} {:02x} {:02x}",
            other[0], other[1], other[2], other[3]
        )),
    }
}

pub fn normalize_to_z64(bytes: &mut [u8]) -> Result<RomFormat, String> {
    let fmt = detect_rom_format(bytes)?;
    match fmt {
        RomFormat::Z64 => {}
        RomFormat::V64 => {
            if !bytes.len().is_multiple_of(2) {
                return Err("v64 ROM length not even".into());
            }
            for chunk in bytes.chunks_exact_mut(2) {
                chunk.swap(0, 1);
            }
        }
        RomFormat::N64 => {
            if !bytes.len().is_multiple_of(4) {
                return Err("n64 ROM length not multiple of 4".into());
            }
            for chunk in bytes.chunks_exact_mut(4) {
                chunk.reverse();
            }
        }
    }
    Ok(fmt)
}

pub fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [
        0x6745_2301,
        0xEFCD_AB89,
        0x98BA_DCFE,
        0x1032_5476,
        0xC3D2_E1F0,
    ];
    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut padded = Vec::with_capacity(data.len() + 72);
    padded.extend_from_slice(data);
    padded.push(0x80);
    while (padded.len() % 64) != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());

    for block in padded.chunks_exact(64) {
        let mut w = [0u32; 80];
        for (i, word) in block.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = h;
        for (i, &wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5A82_7999u32),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1u32),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDCu32),
                _ => (b ^ c ^ d, 0xCA62_C1D6u32),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }

    let mut out = [0u8; 20];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

pub fn sha1_hex(data: &[u8]) -> String {
    let digest = sha1(data);
    let mut s = String::with_capacity(40);
    for b in digest {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

pub fn verify_us_rom(bytes: &mut [u8]) -> Result<RomReport, String> {
    let format = normalize_to_z64(bytes)?;
    if bytes.len() < 0x40 {
        return Err("ROM header truncated (< 0x40 bytes)".into());
    }
    let title = String::from_utf8_lossy(&bytes[0x20..0x34]).trim().to_string();
    let game_id = String::from_utf8_lossy(&bytes[0x3B..0x3F]).to_string();
    let crc32 = crc32_ieee(bytes);
    let sha1_hex_str = sha1_hex(bytes);

    if game_id != "NZSE" {
        return Err(format!(
            "unexpected N64 game ID '{game_id}' (expected 'NZSE' for US Majora's Mask)"
        ));
    }

    if bytes.len() == US_COMPRESSED_SIZE {
        if crc32 != US_COMPRESSED_CRC32 {
            return Err(format!(
                "US 1.0 compressed ROM CRC32 mismatch: expected {US_COMPRESSED_CRC32:#010x}, got {crc32:#010x}"
            ));
        }
    } else if bytes.len() == US_DECOMPRESSED_SIZE {
        if sha1_hex_str != US_DECOMPRESSED_SHA1 {
            return Err(format!(
                "US 1.0 decompressed ROM SHA-1 mismatch: expected {US_DECOMPRESSED_SHA1}, got {sha1_hex_str}"
            ));
        }
    } else {
        return Err(format!(
            "unsupported ROM size {} bytes (expected 32 MiB compressed or 64 MiB decompressed)",
            bytes.len()
        ));
    }

    Ok(RomReport {
        format,
        size: bytes.len(),
        crc32,
        sha1_hex: sha1_hex_str,
        title,
        game_id,
    })
}

pub fn parse_dmadata(rom: &[u8]) -> Result<Vec<DmaEntry>, String> {
    let end = DMADATA_OFFSET + DMADATA_COUNT * 16;
    if rom.len() < end {
        return Err("ROM too small to contain dmadata table".into());
    }
    let mut entries = Vec::with_capacity(DMADATA_COUNT);
    for chunk in rom[DMADATA_OFFSET..end].chunks_exact(16) {
        let vrom_start = u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        let vrom_end = u32::from_be_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]);
        let rom_start = u32::from_be_bytes([chunk[8], chunk[9], chunk[10], chunk[11]]);
        let rom_end = u32::from_be_bytes([chunk[12], chunk[13], chunk[14], chunk[15]]);
        if vrom_start == 0 && vrom_end == 0 && rom_start == 0 && rom_end == 0 {
            continue;
        }
        entries.push(DmaEntry {
            vrom_start,
            vrom_end,
            rom_start,
            rom_end,
        });
    }
    Ok(entries)
}

/// Decompress a Yaz0-compressed N64 segment (`b"Yaz0"` header).
pub fn yaz0_decompress(src: &[u8]) -> Result<Vec<u8>, String> {
    if src.len() < 16 || &src[0..4] != b"Yaz0" {
        return Err("invalid Yaz0 header".into());
    }
    let uncompressed_size = u32::from_be_bytes([src[4], src[5], src[6], src[7]]) as usize;
    let mut out = Vec::with_capacity(uncompressed_size);
    let mut pos = 16usize;

    while out.len() < uncompressed_size {
        if pos >= src.len() {
            return Err("truncated Yaz0 stream (missing group header)".into());
        }
        let mut group = src[pos];
        pos += 1;
        for _ in 0..8 {
            if out.len() >= uncompressed_size {
                break;
            }
            if (group & 0x80) != 0 {
                if pos >= src.len() {
                    return Err("truncated Yaz0 literal byte".into());
                }
                out.push(src[pos]);
                pos += 1;
            } else {
                if pos + 1 >= src.len() {
                    return Err("truncated Yaz0 back-reference".into());
                }
                let b1 = src[pos];
                let b2 = src[pos + 1];
                pos += 2;
                let dist = (((b1 & 0x0F) as usize) << 8) | (b2 as usize);
                let mut count = (b1 >> 4) as usize;
                if count == 0 {
                    if pos >= src.len() {
                        return Err("truncated Yaz0 extended length".into());
                    }
                    count = (src[pos] as usize) + 0x12;
                    pos += 1;
                } else {
                    count += 2;
                }
                let copy_off = dist + 1;
                if copy_off > out.len() {
                    return Err("Yaz0 back-reference before output start".into());
                }
                for _ in 0..count {
                    if out.len() >= uncompressed_size {
                        break;
                    }
                    let byte = out[out.len() - copy_off];
                    out.push(byte);
                }
            }
            group <<= 1;
        }
    }
    Ok(out)
}
