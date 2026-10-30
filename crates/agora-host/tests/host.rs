//! End-to-end: a real Node plugin talking to the host over the SDK.
//! Skipped when no Node 22+ (with a global WebSocket) is on PATH.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::Command;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use agora_host::{Config, DeviceDesc, HostEvent, HostHandle, Overlay, Source};

fn node_ok() -> bool {
    Command::new("node")
        .args(["-e", "process.exit(typeof WebSocket === 'function' ? 0 : 1)"])
        .status()
        .is_ok_and(|s| s.success())
}

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures")
}

fn start(data: &std::path::Path) -> (HostHandle, Receiver<HostEvent>) {
    let (tx, rx) = std::sync::mpsc::channel();
    let config = Config {
        plugin_dirs: vec![(fixtures(), Source::Agora)],
        data_dir: data.to_path_buf(),
        node: None,
        device: DeviceDesc { id: "TEST-M18".into(), name: "Stream Dock M18".into(), rows: 3, columns: 5 },
    };
    (HostHandle::start(config, tx).unwrap(), rx)
}

/// Waits for an event matching `pred`, failing after `secs`.
fn wait_for(rx: &Receiver<HostEvent>, secs: u64, mut pred: impl FnMut(&HostEvent) -> bool) -> HostEvent {
    let end = Instant::now() + Duration::from_secs(secs);
    let mut seen = Vec::new();
    while Instant::now() < end {
        if let Ok(ev) = rx.recv_timeout(Duration::from_millis(100)) {
            if pred(&ev) {
                return ev;
            }
            seen.push(format!("{ev:?}").chars().take(160).collect::<String>());
        }
    }
    panic!("event not seen; got:\n{}", seen.join("\n"));
}

fn http_get(port: u16, path_and_query: &str) -> String {
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    write!(s, "GET {path_and_query} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n").unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    out
}

#[test]
fn node_plugin_round_trip() {
    if !node_ok() {
        eprintln!("skipping: needs Node 22+ on PATH");
        return;
    }
    let data = tempfile::tempdir().unwrap();
    let (host, rx) = start(data.path());
    host.device_connected(true);

    let actions = host.actions();
    assert_eq!(actions.len(), 1);
    assert!(actions[0].icon.as_deref().unwrap_or("").starts_with("data:image/png;base64,"));

    host.place(0, "com.agora.testplugin", "com.agora.testplugin.counter").unwrap();
    // The manifest face first, then the plugin's title after willAppear.
    wait_for(&rx, 10, |e| matches!(e, HostEvent::Face { key: 0, face } if face.title == "hello" && face.image.is_some()));

    host.key(0, true);
    host.key(0, false);
    wait_for(&rx, 5, |e| matches!(e, HostEvent::Face { key: 0, face } if face.overlay == Overlay::Ok));
    wait_for(&rx, 5, |e| matches!(e, HostEvent::Face { key: 0, face } if face.title == "n=1"));

    // Settings panel: the page is served with the connect script added.
    let info = host.key_info(0).unwrap();
    let url = info.settings_url.unwrap();
    let path = url.split_once(&format!("127.0.0.1:{}", host.port)).unwrap().1;
    let page = http_get(host.port, path);
    assert!(page.contains("200 OK"), "{page}");
    assert!(page.contains("connectElgatoStreamDeckSocket||window.connectSDSocket"));
    // No escaping the plugin folder.
    assert!(http_get(host.port, "/plugin/com.agora.testplugin/..%2F..%2FCargo.toml").contains("404"));

    // Settings panel round trip over the socket: sendToPlugin -> sendToPropertyInspector.
    let context = url.split("uuid=").nth(1).unwrap().split('&').next().unwrap().to_string();
    let script = format!(
        "const ws=new WebSocket('ws://127.0.0.1:{port}');\
         ws.onopen=()=>{{ws.send(JSON.stringify({{event:'registerPropertyInspector',uuid:'{context}'}}));\
         ws.send(JSON.stringify({{action:'com.agora.testplugin.counter',event:'sendToPlugin',context:'{context}',payload:{{hi:1}}}}));}};\
         ws.onmessage=(e)=>{{const m=JSON.parse(e.data);if(m.event==='sendToPropertyInspector'){{console.log(JSON.stringify(m.payload));process.exit(0);}}}};\
         setTimeout(()=>process.exit(2),5000);",
        port = host.port
    );
    let out = Command::new("node").args(["-e", &script]).output().unwrap();
    assert!(out.status.success(), "PI script failed: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), r#"{"echo":{"hi":1}}"#);

    // Settings and global settings are saved to the layout file.
    std::thread::sleep(Duration::from_millis(1500));
    let saved = std::fs::read_to_string(data.path().join("layout.json")).unwrap();
    assert!(saved.contains("\"count\": 1"), "{saved}");
    assert!(saved.contains("\"pressed\": true"), "{saved}");

    // Removing the last key stops the plugin.
    host.clear(0);
    wait_for(&rx, 5, |e| matches!(e, HostEvent::PluginStatus { status, .. } if status == "stopped"));
    drop(host);
}

#[test]
fn layout_survives_restart() {
    if !node_ok() {
        return;
    }
    let data = tempfile::tempdir().unwrap();
    {
        let (host, rx) = start(data.path());
        host.place(4, "com.agora.testplugin", "com.agora.testplugin.counter").unwrap();
        wait_for(&rx, 10, |e| matches!(e, HostEvent::Face { key: 4, face } if face.title == "hello"));
    }
    let (host, rx) = start(data.path());
    // The plugin is started again for the saved key, and draws it.
    wait_for(&rx, 10, |e| matches!(e, HostEvent::Face { key: 4, face } if face.title == "hello"));
    assert_eq!(host.key_info(4).unwrap().name, "Counter");
}

#[test]
fn moving_a_key_keeps_the_action_and_swaps() {
    if !node_ok() {
        return;
    }
    let data = tempfile::tempdir().unwrap();
    let (host, rx) = start(data.path());
    host.place(0, "com.agora.testplugin", "com.agora.testplugin.counter").unwrap();
    wait_for(&rx, 10, |e| matches!(e, HostEvent::Face { key: 0, face } if face.title == "hello"));
    let context = host.key_info(0).unwrap().settings_url.unwrap();

    // Onto an empty key: the old key goes blank and the plugin redraws the new one.
    host.move_key(0, 3);
    wait_for(&rx, 5, |e| matches!(e, HostEvent::Face { key: 0, face } if face.image.is_none() && face.title.is_empty()));
    wait_for(&rx, 5, |e| matches!(e, HostEvent::Face { key: 3, face } if face.title == "hello"));
    assert!(host.key_info(0).is_none());
    let moved = host.key_info(3).unwrap().settings_url.unwrap();
    let ctx = |u: &str| u.split("uuid=").nth(1).unwrap().split('&').next().unwrap().to_string();
    assert_eq!(ctx(&context), ctx(&moved), "same action instance");
    host.key(3, true);
    host.key(3, false);
    wait_for(&rx, 5, |e| matches!(e, HostEvent::Face { key: 3, face } if face.title == "n=1"));

    // Onto a used key: the two swap.
    host.place(1, "com.agora.testplugin", "com.agora.testplugin.counter").unwrap();
    host.move_key(3, 1);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(ctx(&host.key_info(1).unwrap().settings_url.unwrap()), ctx(&moved));
    assert!(host.key_info(3).is_some());
}

#[test]
fn hiding_and_forgetting_plugins() {
    if !node_ok() {
        return;
    }
    let data = tempfile::tempdir().unwrap();
    let (host, rx) = start(data.path());
    host.place(2, "com.agora.testplugin", "com.agora.testplugin.counter").unwrap();
    wait_for(&rx, 10, |e| matches!(e, HostEvent::Face { key: 2, face } if face.title == "hello"));

    // Hiding clears its keys and takes its actions out of the list.
    host.set_hidden("com.agora.testplugin", true);
    wait_for(&rx, 5, |e| matches!(e, HostEvent::PluginStatus { status, .. } if status == "stopped"));
    assert!(host.key_info(2).is_none());
    assert!(host.actions().is_empty());
    assert!(host.plugins()[0].hidden);
    host.set_hidden("com.agora.testplugin", false);
    assert_eq!(host.actions().len(), 1);

    // Forgetting drops it and hands back its folder.
    let dir = host.forget("com.agora.testplugin").unwrap();
    assert!(dir.ends_with("com.agora.testplugin.sdPlugin"));
    assert!(host.plugins().is_empty());
    assert!(host.place(0, "com.agora.testplugin", "com.agora.testplugin.counter").is_err());
}
