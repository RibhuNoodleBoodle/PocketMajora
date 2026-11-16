#!/usr/bin/env bash
set -euo pipefail

export PATH="$HOME/.cargo/bin:$PATH"
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

echo "=== [1/4] Host Unit Tests & Release Packaging ==="
"$ROOT_DIR/scripts/package_release.sh"

PPSSPP_GAME="/home/ribhu/snap/ppsspp-emulator/common/.config/ppsspp/PSP/GAME/PocketMajora"
PPSSPP_COMMON="/home/ribhu/snap/ppsspp-emulator/common"
mkdir -p "$PPSSPP_GAME/data"
cp "$ROOT_DIR/target/mipsel-sony-psp/release/EBOOT.PBP" "$PPSSPP_GAME/EBOOT.PBP"

run_ppsspp_wait() {
  local ini_file="$1"
  local watch_file="$2"
  local timeout_sec="$3"
  local log_name="$4"
  rm -f "$watch_file"
  timeout "${timeout_sec}s" /snap/bin/ppsspp-emulator \
    --appendconfig="$ini_file" \
    --log="$PPSSPP_COMMON/$log_name" \
    "$PPSSPP_GAME/EBOOT.PBP" >/dev/null 2>&1 &
  local pid=$!
  local max_iters=$((timeout_sec * 4 - 4))
  for _ in $(seq 1 "$max_iters"); do
    if [ -s "$watch_file" ]; then
      sleep 0.3
      break
    fi
    sleep 0.25
  done
  kill "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true
}

echo "=== [2/4] Test 10A: PSP-1000 Fat (PSPModel = 0) Negative Guard ==="
printf '[SystemParam]\nPSPModel = 0\n' > "$PPSSPP_COMMON/fat_test.ini"
run_ppsspp_wait "$PPSSPP_COMMON/fat_test.ini" "$PPSSPP_GAME/crash.log" 10 "ppsspp_step10a.log"
cat "$PPSSPP_GAME/crash.log"
grep -q "DIAG_ERROR: SLIM_64MB_REQUIRED" "$PPSSPP_GAME/crash.log"

echo "=== [3/4] Test 10B: Missing data/pmm_2s2h.pak Negative Guard ==="
printf '[SystemParam]\nPSPModel = 1\n' > "$PPSSPP_COMMON/slim_test.ini"
rm -f "$PPSSPP_GAME/data/pmm_2s2h.pak"
run_ppsspp_wait "$PPSSPP_COMMON/slim_test.ini" "$PPSSPP_GAME/crash.log" 10 "ppsspp_step10b.log"
cat "$PPSSPP_GAME/crash.log"
grep -q "DIAG_ERROR: MISSING_ASSET_PAK" "$PPSSPP_GAME/crash.log"

echo "=== [4/4] Test 10C: Full End-to-End 40 FPS Benchmark on PPSSPP Slim ==="
if [ -f "$ROOT_DIR/data/pmm_2s2h.pak" ]; then
  cp "$ROOT_DIR/data/pmm_2s2h.pak" "$PPSSPP_GAME/data/pmm_2s2h.pak"
else
  cp "$ROOT_DIR/target/test_synthetic.pak" "$PPSSPP_GAME/data/pmm_2s2h.pak"
fi
rm -f "$PPSSPP_GAME/mm-psp-save.bin" "$PPSSPP_GAME/settings.ini" "$PPSSPP_GAME/crash.log"
run_ppsspp_wait "$PPSSPP_COMMON/slim_test.ini" "$PPSSPP_GAME/step10_final_verification.log" 42 "ppsspp_step10c.log"

echo "=== settings.ini ==="
cat "$PPSSPP_GAME/settings.ini"
echo "=== step10_final_verification.log ==="
cat "$PPSSPP_GAME/step10_final_verification.log"
grep -q "STATUS: PASS" "$PPSSPP_GAME/step10_final_verification.log"
