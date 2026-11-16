use alloc::format;
use alloc::string::String;
use psp::io::File;
use psp::sys::{self, IoOpenFlags};

use crate::platform::{crash, pak_io::PspPakSource};

pub const MIN_SLIM_FREE_BYTES: usize = 24 * 1024 * 1024;
const SETTINGS_PATH: &str = "settings.ini";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UserSettings {
    pub aspect_ratio_16_9: bool,
    pub vsync: bool,
    pub button_swap: bool,
    pub hud_margin: u8,
}

impl Default for UserSettings {
    fn default() -> Self {
        Self {
            aspect_ratio_16_9: true,
            vsync: false,
            button_swap: false,
            hud_margin: 8,
        }
    }
}

impl UserSettings {
    pub fn load_or_create() -> (Self, bool) {
        let mut settings = Self::default();
        if let Ok(f) = File::open(SETTINGS_PATH, IoOpenFlags::RD_ONLY) {
            let mut buf = [0u8; 512];
            if let Ok(n) = f.read(&mut buf)
                && let Ok(text) = core::str::from_utf8(&buf[..n])
            {
                for line in text.lines() {
                    if let Some((k, v)) = line.split_once('=') {
                        let k = k.trim();
                        let v = v.trim();
                        match k {
                            "aspect_ratio" => settings.aspect_ratio_16_9 = v != "4:3",
                            "vsync" => settings.vsync = v == "1" || v == "true",
                            "button_swap" => settings.button_swap = v == "1" || v == "true",
                            "hud_margin" => {
                                if let Ok(m) = v.parse::<u8>() {
                                    settings.hud_margin = m.min(24);
                                }
                            }
                            _ => {}
                        }
                    }
                }
                return (settings, true);
            }
        }
        let persisted = settings.save();
        (settings, persisted)
    }

    pub fn save(&self) -> bool {
        let content = format!(
            "[Video]\naspect_ratio = {}\nvsync = {}\nhud_margin = {}\n[Controls]\nbutton_swap = {}\n",
            if self.aspect_ratio_16_9 { "16:9" } else { "4:3" },
            if self.vsync { 1 } else { 0 },
            self.hud_margin,
            if self.button_swap { 1 } else { 0 },
        );
        if let Ok(f) = File::create(SETTINGS_PATH) {
            return f.write(content.as_bytes()).is_ok();
        }
        false
    }
}

/// Verify PSP Slim/3000 64 MiB DRAM (`>= 24 MiB` free at startup).
/// On PSP-1000 Fat (32 MiB), logs `DIAG_ERROR: SLIM_64MB_REQUIRED` to `crash.log` & returns `Err`.
pub fn check_ram_requirement() -> Result<usize, usize> {
    let free = unsafe { sys::sceKernelTotalFreeMemSize() } as usize;
    if free < MIN_SLIM_FREE_BYTES {
        let msg = format!(
            "DIAG_ERROR: SLIM_64MB_REQUIRED\n\
             free_bytes={free}\n\
             PSP-2000/3000 Slim 64 MiB memory (MEMSIZE=1) is required to run PocketMajora.\n\
             Detected PSP-1000 Fat memory model (< 24 MiB free). Exiting cleanly.\n"
        );
        let _ = crash::write_text_file("crash.log", &msg);
        psp::dprintln!("{}", msg);
        return Err(free);
    }
    Ok(free)
}

/// Verify `data/pmm_2s2h.pak` is present next to `EBOOT.PBP`.
/// On failure, logs `DIAG_ERROR: MISSING_ASSET_PAK` to `crash.log` & returns `Err`.
pub fn open_asset_pak(path: &str) -> Result<PspPakSource, String> {
    match PspPakSource::open(path) {
        Ok(src) => Ok(src),
        Err(err) => {
            let msg = format!(
                "DIAG_ERROR: MISSING_ASSET_PAK\n\
                 expected_path=PSP/GAME/PocketMajora/{path}\n\
                 io_error={err:?}\n\
                 Please run pmm-extractor on your legal US Majora's Mask N64 ROM and copy data/pmm_2s2h.pak.\n"
            );
            let _ = crash::write_text_file("crash.log", &msg);
            psp::dprintln!("{}", msg);
            Err(msg)
        }
    }
}
