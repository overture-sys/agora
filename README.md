# Agora

A free, open-source (MIT) app for VSDinside / Mirabox Stream Dock devices: a lighter, faster replacement for VSD Craft that runs existing Stream Dock plugins unchanged.

**Status: phase 2 (plugin host), pre-alpha.** Tested on the Stream Dock M18 only. Not made by or affiliated with VSDinside, Mirabox or HotSpot.

Agora and VSD Craft can't use the dock at the same time. Close VSD Craft (Stream Dock) before starting Agora; quitting Agora from its tray icon hands the dock back.

## What works now
- M18 driver: open, key images, brightness, light ring, key and round-button presses, keepalive, hand-back on exit.
- `agora-m18`: a command-line test tool (`list`, `test`, `orient`, `buttons`).
- `agora` app: tray icon, finds and reconnects the dock, brightness and ring colour.
- Plugin host: runs Stream Dock plugins unchanged (Node.js, HTML and .exe plugins). Click a key and pick an action, or drag an action onto a key. The plugin draws the key, gets the presses, and its own settings panel opens next to the keys. Tested with Winged Keys and Shannon Keys.

Plugins are read from `%APPDATA%\Agora\plugins` first, then from VSD Craft's plugin folder (`%APPDATA%\HotSpot\StreamDock\plugins`), which Agora never changes. Key layout and plugin settings are saved in `%APPDATA%\Agora\layout.json`; plugin output goes to `%APPDATA%\Agora\logs`.

Node.js plugins need Node.js 20 or newer. Agora uses, in order: the `AGORA_NODE` environment variable, `node\node.exe` next to `agora.exe`, then `node` on PATH.

Profiles, pages, folders, multi-actions and the round buttons come in the next phases. See the blueprint in the project notes.

## Layout
```
crates/agora-device   device layer: drivers, key images, dock worker thread
crates/agora-host     plugin host: manifests, Stream Dock SDK over WebSocket, key faces
crates/agora-m18      M18 test tool
src-tauri             the app (Tauri 2, Rust)
ui                    the app's screens (Svelte 5 + TypeScript)
```

## Build
Needs Rust (stable) and Node 20+. On Windows the WebView2 runtime is already part of Windows 10/11.

```
cargo test -p agora-device               # protocol tests, no dock needed
cargo test -p agora-host                 # plugin host tests (Node 22+ on PATH for the end-to-end ones)
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
