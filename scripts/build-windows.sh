#!/usr/bin/env bash
# Cross-compiles the program for Windows 10/11 (x86_64-pc-windows-msvc)
# using the project-local LLVM + cargo-xwin toolchain and an xwin cache inside
# the project. Linux build artifacts are not touched.
set -euo pipefail

_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$_root"

# shellcheck source=../env.sh
. "$_root/env.sh"

export PATH="$_root/toolchain/llvm/bin:$PATH"
export XWIN_CACHE_DIR="$_root/toolchain/xwin-cache"
# Static MSVC runtime: the EXE then needs no Visual C++ Redistributable.
# The CRT archives ship without PDB paths; ignore that harmless linker warning.
export RUSTFLAGS="${RUSTFLAGS:-} -C target-feature=+crt-static -C link-arg=/ignore:4099"
export CC_x86_64_pc_windows_msvc="clang-cl"
export AR_x86_64_pc_windows_msvc="llvm-lib"
export CXX_x86_64_pc_windows_msvc="clang-cl"

exec cargo xwin build --release --target x86_64-pc-windows-msvc "$@"
