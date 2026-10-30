//! Runs real plugins against the host and saves each key face as a PNG.
//! `cargo run -p agora-host --example try_plugins -- <plugins dir> <out dir> <plugin uuid> [secs]`

use std::path::PathBuf;
use std::time::{Duration, Instant};

use agora_host::{face, Config, DeviceDesc, HostEvent, HostHandle, Source};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (dir, out, uuid) = (PathBuf::from(&args[1]), PathBuf::from(&args[2]), args[3].clone());
    let secs: u64 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(8);
    std::fs::create_dir_all(&out).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let config = Config {
        plugin_dirs: vec![(dir, Source::Agora)],
        data_dir: out.join("data"),
        node: None,
        device: DeviceDesc { id: "TRY".into(), name: "Stream Dock M18".into(), rows: 3, columns: 5 },
    };
    let host = HostHandle::start(config, tx).unwrap();
    host.device_connected(true);
    let actions: Vec<_> = host.actions().into_iter().filter(|a| a.plugin == uuid).collect();
    for (i, a) in actions.iter().enumerate().take(15) {
        println!("key {i}: {} ({})", a.name, a.action);
        host.place(i, &a.plugin, &a.action).unwrap();
    }
    let end = Instant::now() + Duration::from_secs(secs);
    let mut pressed = false;
    while Instant::now() < end {
        if !pressed && Instant::now() + Duration::from_secs(secs / 2) > end {
            // Press the first key once halfway through.
            host.key(0, true);
            host.key(0, false);
            pressed = true;
        }
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(HostEvent::Face { key, face: f }) => {
                println!("face {key}: title={:?} image={} overlay={:?}", f.title, f.image.as_ref().map(|s| s.len()).unwrap_or(0), f.overlay);
                face::render(&f, 144).save(out.join(format!("key{key:02}.png"))).unwrap();
            }
            Ok(HostEvent::Log { message, .. }) => println!("log: {}", message.chars().take(200).collect::<String>()),
            Ok(other) => println!("{other:?}"),
            Err(_) => {}
        }
    }
    for i in 0..actions.len().min(15) {
        if let Some(k) = host.key_info(i) {
            println!("SETTINGS {i} {}", k.settings_url.unwrap_or_default());
        }
    }
    // Keep serving so settings panels can be opened, printing what they do.
    let hold: u64 = std::env::var("HOLD").ok().and_then(|s| s.parse().ok()).unwrap_or(0);
    let end = Instant::now() + Duration::from_secs(hold);
    while Instant::now() < end {
        if let Ok(ev) = rx.recv_timeout(Duration::from_millis(100)) {
            println!("{}", format!("{ev:?}").chars().take(200).collect::<String>());
        }
    }
}
