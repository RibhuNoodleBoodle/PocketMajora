# PocketMajora (Rust PSP-3000 Port of *The Legend of Zelda: Majora's Mask*)

PocketMajora is a clean-room, **Rust-first** native PlayStation Portable (PSP-2000 / PSP-3000, 64 MiB Slim `MEMSIZE=1`) homebrew port of original US *The Legend of Zelda: Majora's Mask*, grounded conceptually in [2Ship2Harkinian](majora_code/) and built on [`AndrewAltimit/rust-psp`](tools/rust-psp/).

## Architecture & Performance Target
- **40.0 FPS Presentation / 20.0 Hz Authentic N64 Logic:** Renders **2 matrix-interpolated GU frames (`fraction = 0.5` and `1.0`, `<= 25,000 us` / `25.0 ms` frame latency)** per deterministic **20.0 Hz N64 game tick**, preserving 3-Day Clock and actor schedule accuracy while keeping 600 Hz audio & GU throughput smooth within the 333/166 MHz Allegrex + VFPU budget.
- **Zero-ROM Distribution Policy:** No copyrighted Nintendo ROMs, `.z64` images, or `.pak` asset packs are committed to Git or shipped in releases. Players extract their own legally owned US *Majora's Mask* ROM (`NZSE`, CRC32 `0xb428d8a7`, SHA-1 `d6133ace5afaa0882cf214cf88daba39e266c078`) using `pmm-extractor`.

## Workspace Crates
- `crates/pmm-schema`: Pak binary format (`PMM2`), `OTR` path hashing, F3DEX2 `GfxWord`, `N64Vtx`, `N64Mtx`, `PmmTexHeader`.
- `crates/pmm-assets`: Bounded scene/room slab arenas (`zero leak` on transitions), N64 segment table, `ResourceManager`.
- `crates/pmm-gfx`: F3DEX2 -> PSP GE interpreter, static `sceGuCallList` caching, triple-buffered 2 MiB VRAM layout, Allegrex VFPU math (`vmmul.q`, `vtfm4.q`, `lerp`).
- `crates/pmm-audio`: Software VADPCM decoder, 16-voice envelope/reverb 32 kHz mixer, 32 -> 44.1 kHz stereo resampler.
- `crates/pmm-game`: `FrameInterpolator`, `THGA` `GfxPool`, `PadMgr`, `SysFlashrom`, `TitleSetup`, `ConsoleLogo`, `FileSelect`, `DayTelop`, `PlayState`, `BgCheck`, `ThreeDayClock`, `Player`, `Camera`, Clock Town actors, and HUD.
- `tools/pmm-extractor`: Host asset compiler reading a legal US ROM + 2S2H XML definitions into `data/pmm_2s2h.pak`.

## Building & Extracting Assets
```bash
# 1. Extract your legally owned US Majora's Mask ROM
cargo run --manifest-path tools/pmm-extractor/Cargo.toml -- \
  --rom "Legend of Zelda, The - Majora's Mask (USA).z64" \
  --xml majora_code/mm/assets/xml \
  --out data/pmm_2s2h.pak

# 2. Run all host unit tests
cargo test -p pmm-schema -p pmm-assets -p pmm-gfx -p pmm-audio -p pmm-game -p pmm-extractor --target x86_64-unknown-linux-gnu

# 3. Build EBOOT.PBP (requires MEMSIZE=1 patched mksfo from tools/rust-psp)
./scripts/build_eboot.sh

# 4. Package ROM-free PSP-3000 release archive
./scripts/package_release.sh
```

## PSP-3000 Installation
Copy the contents of `target/release-pkg/PocketMajora-psp3000.zip` (`PSP/GAME/PocketMajora/`) to your Memory Stick (`ms0:/PSP/GAME/PocketMajora/` or `ef0:/PSP/GAME/PocketMajora/`), and place your extracted `pmm_2s2h.pak` at `PSP/GAME/PocketMajora/data/pmm_2s2h.pak`. PSP-1000 (32 MiB) is unsupported and displays an on-screen diagnostic if launched.

## Controls (PSP -> N64)
- **Analog Nub**: Movement
- **Cross (`X`)**: `A` Button (Interact / Roll / Confirm)
- **Square (`[]`)**: `B` Button (Sword)
- **Triangle (`/\`) / D-Pad**: `C-Buttons` (or held `SELECT` + D-Pad for N64 D-Pad)
- **L Trigger**: `Z` Target (24 px letterbox)
- **R Trigger**: Shield (`R`)
- **START**: Pause / Start

## Credits
- Conceptual & source reference: [2Ship2Harkinian](majora_code/) (CC0-1.0)
- Rust PSP SDK: [`AndrewAltimit/rust-psp`](tools/rust-psp/) (MIT)
