#!/usr/bin/env bash
set -euo pipefail

export PATH="$HOME/.cargo/bin:$PATH"
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

cargo psp --release

REL_DIR="$ROOT_DIR/target/mipsel-sony-psp/release"
mksfo -d MEMSIZE=1 "PocketMajora" "$REL_DIR/PARAM.SFO"
pack-pbp "$REL_DIR/EBOOT.PBP" \
  "$REL_DIR/PARAM.SFO" \
  NULL NULL NULL NULL NULL \
  "$REL_DIR/pocket-majora.prx" NULL

ls -lh "$REL_DIR/EBOOT.PBP" "$REL_DIR/PARAM.SFO"
