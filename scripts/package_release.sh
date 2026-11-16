#!/usr/bin/env bash
set -euo pipefail

export PATH="$HOME/.cargo/bin:$PATH"
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

cargo test -p pmm-schema -p pmm-assets -p pmm-gfx -p pmm-audio -p pmm-game -p pmm-extractor --target x86_64-unknown-linux-gnu
"$ROOT_DIR/scripts/build_eboot.sh"

PKG_ROOT="$ROOT_DIR/target/release-pkg"
STAGE_DIR="$PKG_ROOT/stage/PSP/GAME/PocketMajora"
OUT_ZIP="$PKG_ROOT/PocketMajora-psp3000.zip"

rm -rf "$PKG_ROOT"
mkdir -p "$STAGE_DIR/data"

cp "$ROOT_DIR/target/mipsel-sony-psp/release/EBOOT.PBP" "$STAGE_DIR/EBOOT.PBP"

cat > "$STAGE_DIR/README.txt" <<'EOF'
PocketMajora (Rust PSP-3000 Port of Majora's Mask)
=================================================
Hardware Requirement: PSP-2000 or PSP-3000 Slim (64 MiB RAM, MEMSIZE=1).
PSP-1000 (32 MiB) is NOT supported.

Installation:
1. Extract this archive so `EBOOT.PBP` sits at `ms0:/PSP/GAME/PocketMajora/EBOOT.PBP`.
2. On your PC, build `pmm-extractor` and extract your legally owned US
   Majora's Mask N64 ROM (`NZSE`, CRC32 `0xb428d8a7`):
     cargo run --manifest-path tools/pmm-extractor/Cargo.toml -- \
       --rom "Legend of Zelda, The - Majora's Mask (USA).z64" \
       --xml majora_code/mm/assets/xml \
       --out pmm_2s2h.pak
3. Copy `pmm_2s2h.pak` to `ms0:/PSP/GAME/PocketMajora/data/pmm_2s2h.pak`.
4. Launch PocketMajora from XMB -> Game -> Memory Stick.
EOF

cat > "$STAGE_DIR/data/PUT_PMM_2S2H_PAK_HERE.txt" <<'EOF'
Place your legally extracted `pmm_2s2h.pak` asset archive in this directory:
  PSP/GAME/PocketMajora/data/pmm_2s2h.pak
EOF

python3 - "$PKG_ROOT/stage" "$OUT_ZIP" <<'PY'
import os, sys, zipfile

stage_dir, out_zip = sys.argv[1], sys.argv[2]
forbidden_exts = ('.z64', '.n64', '.v64', '.rom', '.otr', '.o2r', '.pak')

with zipfile.ZipFile(out_zip, 'w', compression=zipfile.ZIP_DEFLATED) as zf:
    for root, _, files in os.walk(stage_dir):
        for fname in sorted(files):
            low = fname.lower()
            if any(low.endswith(ext) for ext in forbidden_exts):
                raise SystemExit(f"FORBIDDEN_ASSET_FOUND: {fname}")
            full = os.path.join(root, fname)
            rel = os.path.relpath(full, stage_dir)
            zf.write(full, rel)

with zipfile.ZipFile(out_zip, 'r') as zf:
    names = zf.namelist()
    for n in names:
        if any(n.lower().endswith(ext) for ext in forbidden_exts):
            raise SystemExit(f"FORBIDDEN_ASSET_IN_ZIP: {n}")
    assert 'PSP/GAME/PocketMajora/EBOOT.PBP' in names
    assert 'PSP/GAME/PocketMajora/README.txt' in names
    assert 'PSP/GAME/PocketMajora/data/PUT_PMM_2S2H_PAK_HERE.txt' in names
    print("ZIP_CONTENTS:", names)
PY

echo "RELEASE_ZIP_ROM_FREE_CHECK: PASS ($OUT_ZIP)"
