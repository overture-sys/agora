# Agora

A free, open-source (MIT) app for VSDinside / Mirabox Stream Dock devices: a lighter, faster replacement for VSD Craft that runs existing Stream Dock plugins unchanged.

**Status: phase 1 (device core), pre-alpha.** Tested on the Stream Dock M18 only. Not made by or affiliated with VSDinside, Mirabox or HotSpot.

Agora and VSD Craft can't use the dock at the same time. Close VSD Craft (Stream Dock) before starting Agora; quitting Agora from its tray icon hands the dock back.

## What works now
- M18 driver: open, key images, brightness, light ring, key and round-button presses, keepalive, hand-back on exit.
- `agora-m18`: a command-line test tool (`list`, `test`, `orient`, `buttons`).
- `agora` app: tray icon, finds and reconnects the dock, live test page (presses light up, brightness, ring colour, text on a key).

Profiles, the plugin host and the editor come in the next phases. See the blueprint in the project notes.

## Layout
```
crates/agora-device   device layer: drivers, key images, dock worker thread
crates/agora-m18      M18 test tool
src-tauri             the app (Tauri 2, Rust)
ui                    the app's screens (Svelte 5 + TypeScript)
```

## Build
Needs Rust (stable) and Node 20+. On Windows the WebView2 runtime is already part of Windows 10/11.

```
cargo test -p agora-device               # protocol tests, no dock needed
cargo run -p agora-m18 -- test           # needs an M18 plugged in
cd ui && npm install && cd ..
cargo install tauri-cli --version "^2"
cargo tauri dev                           # app with live reload
cargo tauri build                         # installer in target/release/bundle
```

Cross-building the Windows app from Linux also works:
`rustup target add x86_64-pc-windows-gnu`, install `mingw-w64`, build the UI, then
`cargo build --release -p agora --features custom-protocol --target x86_64-pc-windows-gnu`.
Ship `agora.exe` with `WebView2Loader.dll` from the same folder.

## Sources
Device protocol from our own M18 tests plus MIT / public-domain notes: bidoofgoo/streamdock-m18 (PROTOCOL.md), bitfocus/companion-surface-mirabox-stream-dock, stevemurr/streamdock. Plugin protocol from the Stream Dock SDK (sdk.key123.vip). No OpenStreamDeck (GPL) code is used.

## License
MIT. Copyright (c) 2026 overture-sys. Built by overture-sys with Claude.
