#!/usr/bin/env bash
set -euo pipefail

export PATH="$HOME/.cargo/bin:$PATH"
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

cargo test --manifest-path crates/pmm-schema/Cargo.toml --target x86_64-unknown-linux-gnu
"$ROOT_DIR/scripts/build_eboot.sh"

PPSSPP_GAME="/home/ribhu/snap/ppsspp-emulator/common/.config/ppsspp/PSP/GAME/PocketMajora"
PPSSPP_COMMON="/home/ribhu/snap/ppsspp-emulator/common"
mkdir -p "$PPSSPP_GAME"
cp "$ROOT_DIR/target/mipsel-sony-psp/release/EBOOT.PBP" "$PPSSPP_GAME/EBOOT.PBP"
printf '[SystemParam]\nPSPModel = 1\n' > "$PPSSPP_COMMON/slim_test.ini"
rm -f "$PPSSPP_GAME/step02_probe.log"

timeout 10s /snap/bin/ppsspp-emulator \
  --appendconfig="$PPSSPP_COMMON/slim_test.ini" \
  --log="$PPSSPP_COMMON/ppsspp_step02.log" \
  "$PPSSPP_GAME/EBOOT.PBP" >/dev/null 2>&1 &
EMU_PID=$!

for _ in $(seq 1 40); do
  if [ -s "$PPSSPP_GAME/step02_probe.log" ]; then
    sleep 0.3
    break
  fi
  sleep 0.25
done
kill "$EMU_PID" 2>/dev/null || true
wait "$EMU_PID" 2>/dev/null || true

echo "=== step02_probe.log ==="
cat "$PPSSPP_GAME/step02_probe.log"
grep -q "STATUS: PASS" "$PPSSPP_GAME/step02_probe.log"
