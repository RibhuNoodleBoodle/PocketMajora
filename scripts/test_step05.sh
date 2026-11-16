#!/usr/bin/env bash
set -euo pipefail

export PATH="$HOME/.cargo/bin:$PATH"
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

cargo test --manifest-path crates/pmm-gfx/Cargo.toml --target x86_64-unknown-linux-gnu
cargo run --manifest-path tools/pmm-extractor/Cargo.toml --target x86_64-unknown-linux-gnu -- build-synthetic-fixture --out target/test_synthetic.pak
"$ROOT_DIR/scripts/build_eboot.sh"

PPSSPP_GAME="/home/ribhu/snap/ppsspp-emulator/common/.config/ppsspp/PSP/GAME/PocketMajora"
PPSSPP_COMMON="/home/ribhu/snap/ppsspp-emulator/common"
mkdir -p "$PPSSPP_GAME/data"
cp "$ROOT_DIR/target/mipsel-sony-psp/release/EBOOT.PBP" "$PPSSPP_GAME/EBOOT.PBP"
cp "$ROOT_DIR/target/test_synthetic.pak" "$PPSSPP_GAME/data/pmm_2s2h.pak"
printf '[SystemParam]\nPSPModel = 1\n' > "$PPSSPP_COMMON/slim_test.ini"
rm -f "$PPSSPP_GAME/step05_gfx.log"

timeout 12s /snap/bin/ppsspp-emulator \
  --appendconfig="$PPSSPP_COMMON/slim_test.ini" \
  --log="$PPSSPP_COMMON/ppsspp_step05.log" \
  "$PPSSPP_GAME/EBOOT.PBP" >/dev/null 2>&1 &
EMU_PID=$!

for _ in $(seq 1 48); do
  if [ -s "$PPSSPP_GAME/step05_gfx.log" ]; then
    sleep 0.3
    break
  fi
  sleep 0.25
done
kill "$EMU_PID" 2>/dev/null || true
wait "$EMU_PID" 2>/dev/null || true

echo "=== step05_gfx.log ==="
cat "$PPSSPP_GAME/step05_gfx.log"
grep -q "STATUS: PASS" "$PPSSPP_GAME/step05_gfx.log"
