# pocketmajora

**pocketmajora** is a native PlayStation Portable port of *The Legend of Zelda: Majora's Mask* (N64 US), built around MM's decompiled C game code and an Allegrex / `sceGu` / Media Engine PSP backend.

> **Hardware Requirement:** Requires **64 MiB PSP expanded memory** (PSP-2000, PSP-3000, PSP Go, PSP Street, or PPSSPP configured for Slim memory). The EBOOT requests `MEMSIZE=1`; PSP-1000 (32 MiB) is not supported.

## Installation

1. Build or download the release package (`mm-psp-n64-us.zip`) and extract `MM_PSP/` into `PSP/GAME/` on your PSP storage:
   - Memory Stick / PSP Go M2: `ms0:/PSP/GAME/MM_PSP/`
   - PSP Go internal storage: `ef0:/PSP/GAME/MM_PSP/`
2. Supply your own **compressed, big-endian US *Majora's Mask* ROM** (32 MiB, CRC32 `B428D8A7`) at:
   - `PSP/GAME/MM_PSP/data/basrom.z64` (`baserom.z64` is also accepted)
3. Launch **pocketmajora** from the XMB Game menu.

```text
PSP/GAME/MM_PSP/
├── EBOOT.PBP
├── README.txt
├── PSPSDK-LICENSE.txt
├── Plugins/
│   └── dvemgr.prx
└── data/
    └── basrom.z64              # user-supplied ROM
```

On first launch, **pocketmajora** validates the ROM and builds `data/segments/oot_psp_assets.bin`. Subsequent boots reuse the generated asset pack directly; delete it to regenerate assets from `basrom.z64`.

## Features & Settings

- **Native 16:9 & Aspect Selection:** Perspective projection and HUD anchoring adapt cleanly to PSP widescreen, with centered 4:3 message/pause overlays and optional 4:3 mode in `HOME > Video Settings`.
- **Frame Rate Options:** Supports 20 FPS (default) and interpolated 60 FPS presentation (`HOME > Video Settings > Frame Rate`), saving preferences to `video.ini` and button mappings to `controls.ini`.
- **Media Engine Audio & TV Out:** Synthesizes 32 kHz audio via the PSP Media Engine by default, and supports PSP TV output when `Plugins/dvemgr.prx` is present.
- **Saves & Hardware Diagnostics:** Flash saves are written to `mm-psp-save.bin` next to `EBOOT.PBP`, and startup/object/rendering diagnostics are recorded to `mm-psp-diag.log`.

## Building from Source

Building the EBOOT requires PSPSDK, `intraFont`, the VME-capable PSP Media Engine library, GNU Make, MIPS binutils, and Python 3 (`PyYAML`, `colorama`). No ROM is required at compile time.

```sh
python3 -m venv .venv
.venv/bin/python3 -m pip install 'PyYAML>=6.0.1,<7.0.0' 'colorama>=0.4.3'
./psp.sh VERSION=n64-us
python3 tools/package_psp.py build/psp-port/n64-us build/mm-psp-n64-us.zip
```

Run the host regression suite with:

```sh
python3 tools/run_psp_tests.py
```

For full technical details on the asset pipeline, renderer, audio backend, and controls, see [the PSP port guide](docs/psp-port.md).
