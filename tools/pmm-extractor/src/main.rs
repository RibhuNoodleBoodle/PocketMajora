mod pak;
mod rom;
mod swizzle;
mod xml;

use pmm_schema::{PmmPakEntry, PmmPakHeader, fourcc};
use std::env;
use std::fs;
use std::path::PathBuf;
use std::process;

fn print_usage() {
    eprintln!("pmm-extractor usage:");
    eprintln!("  pmm-extractor verify-rom <rom_path>");
    eprintln!("  pmm-extractor audit-xml <xml_dir>");
    eprintln!("  pmm-extractor extract --rom <rom_path> --xml-dir <xml_dir> --out <pak_path>");
    eprintln!("  pmm-extractor build-synthetic-fixture --out <pak_path>");
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        print_usage();
        process::exit(2);
    }

    match args[1].as_str() {
        "verify-rom" => {
            if args.len() < 3 {
                print_usage();
                process::exit(2);
            }
            let mut bytes = fs::read(&args[2]).unwrap_or_else(|e| {
                eprintln!("failed to read {}: {e}", args[2]);
                process::exit(1);
            });
            match rom::verify_us_rom(&mut bytes) {
                Ok(r) => {
                    println!(
                        "ROM_VERIFY: PASS format={:?} size={} game_id={} title='{}' crc32={:#010x} sha1={}",
                        r.format, r.size, r.game_id, r.title, r.crc32, r.sha1_hex
                    );
                }
                Err(e) => {
                    eprintln!("ROM_VERIFY: FAIL {e}");
                    process::exit(1);
                }
            }
        }
        "audit-xml" => {
            if args.len() < 3 {
                print_usage();
                process::exit(2);
            }
            let dir = PathBuf::from(&args[2]);
            match xml::audit_xml_dir(&dir) {
                Ok(stats) => {
                    println!(
                        "XML_AUDIT: PASS files_scanned={} files_parsed={} parse_errors={} declared_assets={}",
                        stats.files_scanned,
                        stats.files_parsed,
                        stats.parse_errors,
                        stats.assets.len()
                    );
                    for (tag, count) in &stats.tag_counts {
                        println!("  <{tag}>: {count}");
                    }
                }
                Err(e) => {
                    eprintln!("XML_AUDIT: FAIL {e}");
                    process::exit(1);
                }
            }
        }
        "extract" => {
            let mut rom_path = None;
            let mut xml_dir = None;
            let mut out_path = None;
            let mut i = 2;
            while i < args.len() {
                match args[i].as_str() {
                    "--rom" if i + 1 < args.len() => {
                        rom_path = Some(PathBuf::from(&args[i + 1]));
                        i += 2;
                    }
                    "--xml-dir" if i + 1 < args.len() => {
                        xml_dir = Some(PathBuf::from(&args[i + 1]));
                        i += 2;
                    }
                    "--out" if i + 1 < args.len() => {
                        out_path = Some(PathBuf::from(&args[i + 1]));
                        i += 2;
                    }
                    _ => {
                        print_usage();
                        process::exit(2);
                    }
                }
            }
            let (Some(rom_path), Some(xml_dir), Some(out_path)) = (rom_path, xml_dir, out_path)
            else {
                print_usage();
                process::exit(2);
            };
            let mut rom_bytes = fs::read(&rom_path).unwrap_or_else(|e| {
                eprintln!("failed to read ROM {}: {e}", rom_path.display());
                process::exit(1);
            });
            let report = rom::verify_us_rom(&mut rom_bytes).unwrap_or_else(|e| {
                eprintln!("ROM validation failed: {e}");
                process::exit(1);
            });
            let dma = rom::parse_dmadata(&rom_bytes).unwrap_or_else(|e| {
                eprintln!("dmadata parse failed: {e}");
                process::exit(1);
            });
            let xml_stats = xml::audit_xml_dir(&xml_dir).unwrap_or_else(|e| {
                eprintln!("XML audit failed: {e}");
                process::exit(1);
            });

            // Decompress dmadata segments into virtual ROM segments table
            let filelist_path = xml_dir.join("../../extractor/filelists/mm.txt");
            let filelist: Vec<String> = fs::read_to_string(&filelist_path)
                .map(|s| s.lines().map(|l| l.trim().to_string()).collect())
                .unwrap_or_default();

            let mut builder = pak::PakBuilder::new(report.crc32);
            let mut decompressed_segments = 0usize;
            for (idx, entry) in dma.iter().enumerate() {
                let is_compressed = entry.rom_end != 0;
                let data = if is_compressed {
                    let s = entry.rom_start as usize;
                    let e = entry.rom_end as usize;
                    if e <= rom_bytes.len() && s < e && &rom_bytes[s..s + 4] == b"Yaz0" {
                        rom::yaz0_decompress(&rom_bytes[s..e]).unwrap_or_default()
                    } else {
                        Vec::new()
                    }
                } else {
                    let len = (entry.vrom_end.saturating_sub(entry.vrom_start)) as usize;
                    let s = entry.rom_start as usize;
                    if s + len <= rom_bytes.len() {
                        rom_bytes[s..s + len].to_vec()
                    } else {
                        Vec::new()
                    }
                };
                if !data.is_empty() {
                    decompressed_segments += 1;
                    let key = if let Some(name) = filelist.get(idx).filter(|n| !n.is_empty()) {
                        format!("__OTR__seg/{name}")
                    } else {
                        format!("__OTR__vrom/{:#010x}", entry.vrom_start)
                    };
                    builder.push_path(&key, fourcc::OROM, 0, data);
                }
            }
            let manifest = format!(
                "ROM_CRC32={:#010x}\nDMA_ENTRIES={}\nDECOMPRESSED={}\nXML_DECLS={}\n",
                report.crc32,
                dma.len(),
                decompressed_segments,
                xml_stats.assets.len()
            );
            builder.push_path(
                "__OTR__meta/manifest",
                fourcc::OTXT,
                PmmPakEntry::FLAG_PINNED_COMMON,
                manifest.into_bytes(),
            );
            let pak_bytes = builder.build_bytes();
            if let Some(parent) = out_path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            fs::write(&out_path, &pak_bytes).unwrap();
            println!(
                "EXTRACT: PASS segments={} xml_decls={} pak_bytes={}",
                decompressed_segments,
                xml_stats.assets.len(),
                pak_bytes.len()
            );
        }
        "build-synthetic-fixture" => {
            if args.len() < 4 || args[2] != "--out" {
                print_usage();
                process::exit(2);
            }
            let out_path = PathBuf::from(&args[3]);
            match pak::build_synthetic_fixture(&out_path) {
                Ok((entries, bytes)) => {
                    // Validate written header immediately
                    let written = fs::read(&out_path).unwrap();
                    let hdr = PmmPakHeader::from_le_bytes(&written).unwrap();
                    println!(
                        "SYNTHETIC_FIXTURE: PASS out={} entries={} bytes={} rom_crc32={:#010x}",
                        out_path.display(),
                        entries,
                        bytes,
                        hdr.rom_crc32
                    );
                }
                Err(e) => {
                    eprintln!("SYNTHETIC_FIXTURE: FAIL {e}");
                    process::exit(1);
                }
            }
        }
        _ => {
            print_usage();
            process::exit(2);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rom::RomFormat;
    use pmm_schema::{PAK_ENTRY_SIZE, PAK_HEADER_SIZE, ResourceId};

    #[test]
    fn endianness_normalization_z64_v64_n64_and_invalid_magic() {
        let z64_orig = [0x80u8, 0x37, 0x12, 0x40, 0x11, 0x22, 0x33, 0x44];
        let mut z64 = z64_orig;
        assert_eq!(rom::normalize_to_z64(&mut z64).unwrap(), RomFormat::Z64);
        assert_eq!(z64, z64_orig);

        let mut v64 = [0x37u8, 0x80, 0x40, 0x12, 0x22, 0x11, 0x44, 0x33];
        assert_eq!(rom::normalize_to_z64(&mut v64).unwrap(), RomFormat::V64);
        assert_eq!(v64, z64_orig);

        let mut n64 = [0x40u8, 0x12, 0x37, 0x80, 0x44, 0x33, 0x22, 0x11];
        assert_eq!(rom::normalize_to_z64(&mut n64).unwrap(), RomFormat::N64);
        assert_eq!(n64, z64_orig);

        let mut bad = [0xDEu8, 0xAD, 0xBE, 0xEF];
        assert!(rom::normalize_to_z64(&mut bad).is_err());
    }

    #[test]
    fn sha1_fips_180_4_known_vector() {
        assert_eq!(
            rom::sha1_hex(b"abc"),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
    }

    #[test]
    fn wrong_crc32_negative_test_reports_diagnostic() {
        let mut fake_rom = vec![0u8; rom::US_COMPRESSED_SIZE];
        fake_rom[0..4].copy_from_slice(&[0x80, 0x37, 0x12, 0x40]);
        fake_rom[0x3B..0x3F].copy_from_slice(b"NZSE");
        let err = rom::verify_us_rom(&mut fake_rom).unwrap_err();
        assert!(err.contains("CRC32 mismatch"), "unexpected err: {err}");
        assert!(err.contains("0xb428d8a7"), "missing expected CRC: {err}");
    }

    #[test]
    fn wrong_game_id_negative_test() {
        let mut fake_rom = vec![0u8; rom::US_COMPRESSED_SIZE];
        fake_rom[0..4].copy_from_slice(&[0x80, 0x37, 0x12, 0x40]);
        fake_rom[0x3B..0x3F].copy_from_slice(b"CZLE"); // OoT instead of MM
        let err = rom::verify_us_rom(&mut fake_rom).unwrap_err();
        assert!(err.contains("NZSE"));
    }

    #[test]
    fn yaz0_decompress_valid_and_truncated_negative() {
        // Compress "ABCABCABCABC" (12 bytes):
        // Group 1: 1110_0000 = 0xE0 -> 3 literals ('A','B','C'), then back-ref dist=2 (copy_off=3), count=9 (9-2=7 -> 0x70, 0x02)
        let mut stream = Vec::new();
        stream.extend_from_slice(b"Yaz0");
        stream.extend_from_slice(&12u32.to_be_bytes());
        stream.extend_from_slice(&[0u8; 8]);
        stream.push(0xE0);
        stream.extend_from_slice(b"ABC");
        stream.push(0x70);
        stream.push(0x02);

        let decoded = rom::yaz0_decompress(&stream).expect("valid yaz0");
        assert_eq!(decoded, b"ABCABCABCABC");

        // Truncated stream negative test
        assert!(rom::yaz0_decompress(&stream[..stream.len() - 2]).is_err());
        assert!(rom::yaz0_decompress(b"BAD0\x00\x00\x00\x04").is_err());
    }

    #[test]
    fn psp_16x8_texture_swizzle_roundtrip() {
        let row_bytes = 32 * 4; // 128 bytes
        let height = 32;
        let orig: Vec<u8> = (0..(row_bytes * height)).map(|i| (i & 0xFF) as u8).collect();
        let (swizzled, did_swizzle) = swizzle::swizzle_psp_texture(&orig, row_bytes, height);
        assert!(did_swizzle);
        assert_ne!(swizzled, orig);
        let unswizzled = swizzle::unswizzle_psp_texture(&swizzled, row_bytes, height);
        assert_eq!(unswizzled, orig);
    }

    #[test]
    fn n64_rgba16_to_psp5551_conversion() {
        // Pure red in N64 RGBA16: R=31,G=0,B=0,A=1 => 0xF801
        let be = 0xF801u16.to_be_bytes();
        let psp = swizzle::convert_n64_rgba16_to_psp5551(&be);
        // PSP 5551: A=1 (bit 15), B=0, G=0, R=31 => 0x801F LE
        assert_eq!(u16::from_le_bytes([psp[0], psp[1]]), 0x801F);
    }

    #[test]
    fn xml_parser_handles_bom_comments_and_file_segments() {
        let sample = "\u{FEFF}<!-- comment -->\n<Root>\n<File Name=\"gameplay_keep\" Segment=\"4\">\n<Texture Name=\"gBottleTex\" Format=\"i4\" Width=\"16\" Height=\"16\" Offset=\"0x0\" />\n<DList Name=\"gBottleContentsDL\" Offset=\"0x320\" />\n</File>\n</Root>";
        let mut stats = xml::XmlAuditStats::default();
        xml::parse_xml_text(sample, &mut stats).unwrap();
        assert_eq!(stats.assets.len(), 2);
        assert_eq!(stats.assets[0].segment, 4);
        assert_eq!(stats.assets[1].offset, 0x320);

        // Commented-out file (like GC_US/misc/locerrmsg.xml)
        let commented = "<!-- <Root><File Name=\"x\"/></Root> -->";
        xml::parse_xml_text(commented, &mut stats).unwrap();
    }

    #[test]
    fn audit_real_repo_xml_directory() {
        let xml_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../majora_code/mm/assets/xml");
        let stats = xml::audit_xml_dir(&xml_dir).expect("audit repo xml");
        assert!(stats.files_scanned >= 1320);
        assert_eq!(stats.files_scanned, stats.files_parsed);
        assert_eq!(stats.parse_errors, 0);
        assert!(stats.assets.len() > 30_000);
    }

    #[test]
    fn synthetic_fixture_builds_valid_aligned_sorted_pak() {
        let tmp = std::env::temp_dir().join("pmm_step03_test_synthetic.pak");
        let (count, size) = pak::build_synthetic_fixture(&tmp).unwrap();
        assert!(count >= 8);
        assert!(size > PAK_HEADER_SIZE);

        let bytes = fs::read(&tmp).unwrap();
        let hdr = PmmPakHeader::from_le_bytes(&bytes).unwrap();
        assert_eq!(hdr.entry_count as usize, count);
        assert_eq!(hdr.rom_crc32, 0x5445_5354);

        let mut prev_id = 0u64;
        for i in 0..count {
            let off = (hdr.index_offset as usize) + i * PAK_ENTRY_SIZE;
            let entry = PmmPakEntry::from_le_bytes(&bytes[off..off + PAK_ENTRY_SIZE]).unwrap();
            assert!(entry.id > prev_id || i == 0);
            prev_id = entry.id;
            assert_eq!(entry.offset % 16, 0);
            assert!((entry.offset as usize) + (entry.size as usize) <= bytes.len());
        }

        // Verify checker32 hash lookup exists
        let checker_id = ResourceId::from_otr_path("__OTR__textures/test/checker32").0;
        let found = (0..count).any(|i| {
            let off = (hdr.index_offset as usize) + i * PAK_ENTRY_SIZE;
            let entry = PmmPakEntry::from_le_bytes(&bytes[off..off + PAK_ENTRY_SIZE]).unwrap();
            entry.id == checker_id && (entry.flags & PmmPakEntry::FLAG_PSP_SWIZZLED) != 0
        });
        assert!(found);
        let _ = fs::remove_file(&tmp);
    }
}
