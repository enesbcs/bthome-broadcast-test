# BTHome Temperature Broadcaster

A small program that broadcasts the machine temperature over Bluetooth LE as a
BTHome v2 advertisement. It updates every 10 seconds and keeps advertising in
the background.

## Running

### Windows 10/11

Double-click `bthome-broadcast-test.exe`. The graphical window opens and shows
the current temperature, the active source, the BLE name and the packet.

If no real temperature sensor is available, the program asks whether it may
fetch the local weather forecast:

- **Accept** – use the area weather forecast as the fallback.
- **Reject** – use clearly marked random test data instead.

Use the **Quit** button or close the window to stop broadcasting.

Console mode:

```powershell
.\bthome-broadcast-test.exe --console
```

In console mode the weather forecast is only used if you pass
`--forecast-enabled`; otherwise test data is the last resort.

### Linux

```bash
./bthome-broadcast-test
```

BlueZ and a Bluetooth LE adapter are required. Press `Ctrl+C` to stop.

### Useful options

```text
--console           Windows: run in console mode instead of the GUI
--forecast-enabled  Windows console: allow the area weather forecast
--diagnose          Show the detected temperature sources and BLE status
--dry-run --once    Print one BTHome packet without advertising
--no-acpi           Disable the ACPI temperature source
--log               Write a rotating log file
--help              Show all options
```

## Building

Rust 1.89 or newer is required.

Linux:

```bash
cargo build --release
# result: target/release/bthome-broadcast-test
```

Static Linux binary (musl):

```bash
rustup target add x86_64-unknown-linux-musl
cargo build --release --target x86_64-unknown-linux-musl
# result: target/x86_64-unknown-linux-musl/release/bthome-broadcast-test
```

Windows (on Windows, with the MSVC Rust toolchain):

```powershell
cargo build --release
# result: target\release\bthome-broadcast-test.exe
```

Windows cross-build from Linux (requires the project's local LLVM/cargo-xwin
toolchain):

```bash
./scripts/build-windows.sh
# result: target/x86_64-pc-windows-msvc/release/bthome-broadcast-test.exe
```
