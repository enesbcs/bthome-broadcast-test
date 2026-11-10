#!/usr/bin/env bash
# Optional local toolchain shim.
#
# If a project-local toolchain exists in ./toolchain (the layout used for the
# Windows cross-build), it takes precedence over any system-wide rustup/cargo
# installation. That directory is machine-specific and is not part of the
# repository, so on a fresh clone this script simply keeps the system toolchain.
#
#   . ./env.sh
set -euo pipefail

_ble_root="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

if [[ -d "$_ble_root/toolchain/rustup" && -d "$_ble_root/toolchain/cargo" ]]; then
    export RUSTUP_HOME="$_ble_root/toolchain/rustup"
    export CARGO_HOME="$_ble_root/toolchain/cargo"
    export PATH="$CARGO_HOME/bin:$PATH"

    # Keep runtime state (logs) inside the project during development.
    export XDG_STATE_HOME="$_ble_root/.state"
fi

unset _ble_root
