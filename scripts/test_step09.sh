#!/usr/bin/env bash
set -euo pipefail

export PATH="$HOME/.cargo/bin:$PATH"
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

cargo test -p pmm-schema -p pmm-assets -p pmm-gfx -p pmm-audio -p pmm-game -p pmm-extractor --target x86_64-unknown-linux-gnu
"$ROOT_DIR/scripts/build_eboot.sh"

PPSSPP_GAME="/home/ribhu/snap/ppsspp-emulator/common/.config/ppsspp/PSP/GAME/PocketMajora"
PPSSPP_COMMON="/home/ribhu/snap/ppsspp-emulator/common"
mkdir -p "$PPSSPP_GAME/data"
cp "$ROOT_DIR/target/mipsel-sony-psp/release/EBOOT.PBP" "$PPSSPP_GAME/EBOOT.PBP"
if [ -f "$ROOT_DIR/data/pmm_2s2h.pak" ]; then
  cp "$ROOT_DIR/data/pmm_2s2h.pak" "$PPSSPP_GAME/data/pmm_2s2h.pak"
else
  cp "$ROOT_DIR/target/test_synthetic.pak" "$PPSSPP_GAME/data/pmm_2s2h.pak"
fi
printf '[SystemParam]\nPSPModel = 1\n' > "$PPSSPP_COMMON/slim_test.ini"
rm -f "$PPSSPP_GAME/step09_clocktown.log" "$PPSSPP_GAME/mm-psp-save.bin"

timeout 30s /snap/bin/ppsspp-emulator \
  --appendconfig="$PPSSPP_COMMON/slim_test.ini" \
  --log="$PPSSPP_COMMON/ppsspp_step09.log" \
  "$PPSSPP_GAME/EBOOT.PBP" >/dev/null 2>&1 &
EMU_PID=$!

for _ in $(seq 1 110); do
  if [ -s "$PPSSPP_GAME/step09_clocktown.log" ]; then
    sleep 0.3
    break
  fi
  sleep 0.25
done
kill "$EMU_PID" 2>/dev/null || true
wait "$EMU_PID" 2>/dev/null || true

echo "=== step09_clocktown.log ==="
cat "$PPSSPP_GAME/step09_clocktown.log"
grep -q "STATUS: PASS" "$PPSSPP_GAME/step09_clocktown.log"
