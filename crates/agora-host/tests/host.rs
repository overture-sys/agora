//! End-to-end: a real Node plugin talking to the host over the SDK.
//! Skipped when no Node 22+ (with a global WebSocket) is on PATH.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::Command;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use agora_host::{
    builtin, Config, DeviceDesc, DialInput, HostEvent, HostHandle, LayoutOp, Look, Overlay, Source,
    BUTTON_BASE, DIAL_BASE,
};

fn node_ok() -> bool {
    Command::new("node")
        .args([
            "-e",
            "process.exit(typeof WebSocket === 'function' ? 0 : 1)",
        ])
        .status()
        .is_ok_and(|s| s.success())
}

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
}

fn start(data: &std::path::Path) -> (HostHandle, Receiver<HostEvent>) {
    let (tx, rx) = std::sync::mpsc::channel();
    let config = Config {
        plugin_dirs: vec![(fixtures(), Source::Agora)],
        data_dir: data.to_path_buf(),
        node: None,
        device: DeviceDesc {
            id: "TEST-M18".into(),
            name: "Stream Dock M18".into(),
            rows: 3,
            columns: 5,
            buttons: 3,
            dials: 2,
        },
    };
    (HostHandle::start(config, tx).unwrap(), rx)
}

/// Waits for an event matching `pred`, failing after `secs`.
fn wait_for(
    rx: &Receiver<HostEvent>,
    secs: u64,
    mut pred: impl FnMut(&HostEvent) -> bool,
) -> HostEvent {
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

/// The test plugin's actions (the list also holds Agora's own).
fn plugin_actions(host: &HostHandle) -> Vec<agora_host::ActionInfo> {
    host.actions()
        .into_iter()
        .filter(|a| a.plugin != builtin::PLUGIN)
        .collect()
}

fn http_get(port: u16, path_and_query: &str) -> String {
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    write!(
        s,
        "GET {path_and_query} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
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

    let actions = plugin_actions(&host);
    assert_eq!(actions.len(), 2);
    assert_eq!(actions[1].controllers, ["Encoder"]);
    assert!(actions[0]
        .icon
        .as_deref()
        .unwrap_or("")
        .starts_with("data:image/png;base64,"));

    host.place(0, "com.agora.testplugin", "com.agora.testplugin.counter")
        .unwrap();
    // The manifest face first, then the plugin's title after willAppear.
    wait_for(
        &rx,
        10,
        |e| matches!(e, HostEvent::Face { key: 0, face } if face.title == "hello" && face.image.is_some()),
    );

    host.key(0, true);
    host.key(0, false);
    wait_for(
        &rx,
        5,
        |e| matches!(e, HostEvent::Face { key: 0, face } if face.overlay == Overlay::Ok),
    );
    wait_for(
        &rx,
        5,
        |e| matches!(e, HostEvent::Face { key: 0, face } if face.title == "n=1"),
    );

    // Settings panel: the page is served with the connect script added.
    let info = host.key_info(0).unwrap();
    let url = info.settings_url.unwrap();
    let path = url
        .split_once(&format!("127.0.0.1:{}", host.port))
        .unwrap()
        .1;
    let page = http_get(host.port, path);
    assert!(page.contains("200 OK"), "{page}");
    assert!(page.contains("connectElgatoStreamDeckSocket||window.connectSDSocket"));
    // No escaping the plugin folder.
    assert!(http_get(
        host.port,
        "/plugin/com.agora.testplugin/..%2F..%2FCargo.toml"
    )
    .contains("404"));

    // Settings panel round trip over the socket: sendToPlugin -> sendToPropertyInspector.
    let context = url
        .split("uuid=")
        .nth(1)
        .unwrap()
        .split('&')
        .next()
        .unwrap()
        .to_string();
    let script = format!(
        "const ws=new WebSocket('ws://127.0.0.1:{port}');\
         ws.onopen=()=>{{ws.send(JSON.stringify({{event:'registerPropertyInspector',uuid:'{context}'}}));\
         ws.send(JSON.stringify({{action:'com.agora.testplugin.counter',event:'sendToPlugin',context:'{context}',payload:{{hi:1}}}}));}};\
         ws.onmessage=(e)=>{{const m=JSON.parse(e.data);if(m.event==='sendToPropertyInspector'){{console.log(JSON.stringify(m.payload));process.exit(0);}}}};\
         setTimeout(()=>process.exit(2),5000);",
        port = host.port
    );
    let out = Command::new("node").args(["-e", &script]).output().unwrap();
    assert!(
        out.status.success(),
        "PI script failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        r#"{"echo":{"hi":1}}"#
    );

    // Settings and global settings are saved to the layout file.
    std::thread::sleep(Duration::from_millis(1500));
    let saved = std::fs::read_to_string(data.path().join("layout.json")).unwrap();
    assert!(saved.contains("\"count\": 1"), "{saved}");
    assert!(saved.contains("\"pressed\": true"), "{saved}");

    // Removing the last key stops the plugin.
    host.clear(0);
    wait_for(
        &rx,
        5,
        |e| matches!(e, HostEvent::PluginStatus { status, .. } if status == "stopped"),
    );
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
        host.place(4, "com.agora.testplugin", "com.agora.testplugin.counter")
            .unwrap();
        wait_for(
            &rx,
            10,
            |e| matches!(e, HostEvent::Face { key: 4, face } if face.title == "hello"),
        );
    }
    let (host, rx) = start(data.path());
    // The plugin is started again for the saved key, and draws it.
    wait_for(
        &rx,
        10,
        |e| matches!(e, HostEvent::Face { key: 4, face } if face.title == "hello"),
    );
    assert_eq!(host.key_info(4).unwrap().name, "Counter");
}

#[test]
fn moving_a_key_keeps_the_action_and_swaps() {
    if !node_ok() {
        return;
    }
    let data = tempfile::tempdir().unwrap();
    let (host, rx) = start(data.path());
    host.place(0, "com.agora.testplugin", "com.agora.testplugin.counter")
        .unwrap();
    wait_for(
        &rx,
        10,
        |e| matches!(e, HostEvent::Face { key: 0, face } if face.title == "hello"),
    );
    let context = host.key_info(0).unwrap().settings_url.unwrap();

    // Onto an empty key: the old key goes blank and the plugin redraws the new one.
    host.move_key(0, 3);
    wait_for(
        &rx,
        5,
        |e| matches!(e, HostEvent::Face { key: 0, face } if face.image.is_none() && face.title.is_empty()),
    );
    wait_for(
        &rx,
        5,
        |e| matches!(e, HostEvent::Face { key: 3, face } if face.title == "hello"),
    );
    assert!(host.key_info(0).is_none());
    let moved = host.key_info(3).unwrap().settings_url.unwrap();
    let ctx = |u: &str| {
        u.split("uuid=")
            .nth(1)
            .unwrap()
            .split('&')
            .next()
            .unwrap()
            .to_string()
    };
    assert_eq!(ctx(&context), ctx(&moved), "same action instance");
    host.key(3, true);
    host.key(3, false);
    wait_for(
        &rx,
        5,
        |e| matches!(e, HostEvent::Face { key: 3, face } if face.title == "n=1"),
    );

    // Onto a used key: the two swap.
    host.place(1, "com.agora.testplugin", "com.agora.testplugin.counter")
        .unwrap();
    host.move_key(3, 1);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        ctx(&host.key_info(1).unwrap().settings_url.unwrap()),
        ctx(&moved)
    );
    assert!(host.key_info(3).is_some());
}

#[test]
fn hiding_and_forgetting_plugins() {
    if !node_ok() {
        return;
    }
    let data = tempfile::tempdir().unwrap();
    let (host, rx) = start(data.path());
    host.place(2, "com.agora.testplugin", "com.agora.testplugin.counter")
        .unwrap();
    wait_for(
        &rx,
        10,
        |e| matches!(e, HostEvent::Face { key: 2, face } if face.title == "hello"),
    );

    // Hiding clears its keys and takes its actions out of the list.
    host.set_hidden("com.agora.testplugin", true);
    wait_for(
        &rx,
        5,
        |e| matches!(e, HostEvent::PluginStatus { status, .. } if status == "stopped"),
    );
    assert!(host.key_info(2).is_none());
    assert!(plugin_actions(&host).is_empty());
    assert!(host.plugins()[0].hidden);
    host.set_hidden("com.agora.testplugin", false);
    assert_eq!(plugin_actions(&host).len(), 2);

    // Forgetting drops it and hands back its folder.
    let dir = host.forget("com.agora.testplugin").unwrap();
    assert!(dir.ends_with("com.agora.testplugin.sdPlugin"));
    assert!(host.plugins().is_empty());
    assert!(host
        .place(0, "com.agora.testplugin", "com.agora.testplugin.counter")
        .is_err());
}

#[test]
fn old_layouts_carry_over_to_the_first_page() {
    let data = tempfile::tempdir().unwrap();
    std::fs::write(
        data.path().join("layout.json"),
        r#"{"keys":{"3":{"context":"ABC","plugin":"com.agora.builtin","action":"website","settings":{"url":"https://example.com"}}}}"#,
    )
    .unwrap();
    let (host, _rx) = start(data.path());
    let view = host.layout().unwrap();
    assert_eq!(view.profiles.len(), 1);
    assert_eq!(view.pages.len(), 1);
    assert_eq!(
        host.key_info(3).unwrap().settings["url"],
        "https://example.com"
    );
}

#[test]
fn pages_folders_and_profiles() {
    let data = tempfile::tempdir().unwrap();
    let (host, rx) = start(data.path());
    host.place(0, builtin::PLUGIN, "website").unwrap();
    let first = host.layout().unwrap().page;

    // A second page starts empty; next/previous wrap round.
    host.layout_op(LayoutOp::AddPage).unwrap();
    let view = host.layout().unwrap();
    assert_eq!(view.pages.len(), 2);
    assert_ne!(view.page, first);
    assert!(host.key_info(0).is_none());
    host.place(1, builtin::PLUGIN, builtin::NEXT_PAGE).unwrap();
    host.key(1, true);
    host.key(1, false);
    assert_eq!(host.layout().unwrap().page, first);
    assert_eq!(host.key_info(0).unwrap().action, "website");

    // A folder opens its own page with a Back key, and is named by its settings.
    host.place(5, builtin::PLUGIN, builtin::FOLDER).unwrap();
    host.set_settings(5, serde_json::json!({ "name": "Games" }));
    wait_for(
        &rx,
        5,
        |e| matches!(e, HostEvent::Face { key: 5, face } if face.title == "Games"),
    );
    host.key(5, true);
    host.key(5, false);
    let view = host.layout().unwrap();
    assert_eq!(view.top_page, first);
    assert_eq!(view.path.len(), 1);
    assert_eq!(view.path[0].name, "Games");
    assert_eq!(host.key_info(0).unwrap().action, builtin::BACK);
    host.key(0, true);
    host.key(0, false);
    assert_eq!(host.layout().unwrap().page, first);

    // Your own title wins over the action's.
    host.set_look(
        5,
        Look {
            title: Some("Play".into()),
            ..Default::default()
        },
    );
    wait_for(
        &rx,
        5,
        |e| matches!(e, HostEvent::Face { key: 5, face } if face.title == "Play"),
    );

    // Profiles: a new one starts with one empty page; switching back keeps the old.
    host.layout_op(LayoutOp::AddProfile("Work".into())).unwrap();
    let view = host.layout().unwrap();
    assert_eq!(view.profiles.len(), 2);
    assert!(host.key_info(0).is_none());
    let work = view.profile.clone();
    let home = view
        .profiles
        .iter()
        .find(|p| p.id != work)
        .unwrap()
        .id
        .clone();
    host.layout_op(LayoutOp::SwitchProfile(home.clone()))
        .unwrap();
    assert_eq!(host.key_info(0).unwrap().action, "website");
    assert!(host.layout_op(LayoutOp::RemoveProfile(work)).is_ok());
    assert!(
        host.layout_op(LayoutOp::RemoveProfile(home)).is_err(),
        "the last profile stays"
    );

    // Removing the folder key removes its page.
    host.clear(5);
    drop(host);
    std::thread::sleep(Duration::from_millis(200));
    let saved = std::fs::read_to_string(data.path().join("layout.json")).unwrap();
    assert!(!saved.contains("Games"), "{saved}");
    assert_eq!(saved.matches("\"parent\"").count(), 0);
}

#[test]
fn plugins_hear_about_page_changes() {
    if !node_ok() {
        return;
    }
    let data = tempfile::tempdir().unwrap();
    let (host, rx) = start(data.path());
    host.place(0, "com.agora.testplugin", "com.agora.testplugin.counter")
        .unwrap();
    wait_for(
        &rx,
        10,
        |e| matches!(e, HostEvent::Face { key: 0, face } if face.title == "hello"),
    );
    host.key(0, true);
    host.key(0, false);
    wait_for(
        &rx,
        5,
        |e| matches!(e, HostEvent::Face { key: 0, face } if face.title == "n=1"),
    );
    let first = host.layout().unwrap().page;

    host.layout_op(LayoutOp::AddPage).unwrap();
    wait_for(
        &rx,
        5,
        |e| matches!(e, HostEvent::Face { key: 0, face } if face.title.is_empty() && face.image.is_none()),
    );
    // The plugin keeps running for the page that isn't showing.
    assert_eq!(host.plugins()[0].status, "running");
    host.layout_op(LayoutOp::GoToPage(first)).unwrap();
    // willAppear again: the plugin redraws its count.
    wait_for(
        &rx,
        5,
        |e| matches!(e, HostEvent::Face { key: 0, face } if face.title == "n=1"),
    );
}

#[test]
fn multi_actions_run_in_order_and_toggles_alternate() {
    let data = tempfile::tempdir().unwrap();
    let (host, rx) = start(data.path());
    let site = |u: &str| serde_json::json!({ "action": "website", "settings": { "url": u } });
    let run_url =
        |rx: &Receiver<HostEvent>| match wait_for(rx, 5, |e| matches!(e, HostEvent::Run { .. })) {
            HostEvent::Run { settings, .. } => settings["url"].as_str().unwrap().to_string(),
            _ => unreachable!(),
        };

    host.place(0, builtin::PLUGIN, builtin::MULTI).unwrap();
    host.set_settings(
        0,
        serde_json::json!({ "steps": [
            site("a"),
            { "action": "delay", "settings": { "ms": 50 } },
            { "action": "multi", "settings": {} },
            site("b"),
        ] }),
    );
    host.key(0, true);
    host.key(0, false);
    assert_eq!(run_url(&rx), "a");
    assert_eq!(run_url(&rx), "b");

    host.place(1, builtin::PLUGIN, builtin::TOGGLE).unwrap();
    host.set_settings(
        1,
        serde_json::json!({ "steps": [site("on")], "steps_b": [site("off")] }),
    );
    for want in ["on", "off", "on"] {
        host.key(1, true);
        host.key(1, false);
        assert_eq!(run_url(&rx), want);
    }
}

#[test]
fn profiles_follow_the_app_in_front() {
    let data = tempfile::tempdir().unwrap();
    let (host, _rx) = start(data.path());
    let home = host.layout().unwrap().profile;
    host.layout_op(LayoutOp::AddProfile("OBS".into())).unwrap();
    let obs = host.layout().unwrap().profile;
    host.layout_op(LayoutOp::SetProfileApps(
        obs.clone(),
        vec![r"C:\Program Files\obs\OBS64.exe".into()],
    ))
    .unwrap();
    assert_eq!(host.layout().unwrap().apps, vec!["obs64.exe"]);
    host.layout_op(LayoutOp::SwitchProfile(home.clone()))
        .unwrap();

    host.foreground_app("obs64.exe");
    assert_eq!(host.layout().unwrap().profile, obs);
    // Agora's own window doesn't count.
    host.foreground_app("agora.exe");
    assert_eq!(host.layout().unwrap().profile, obs);
    host.foreground_app("notepad.exe");
    assert_eq!(host.layout().unwrap().profile, home);
    // A profile you pick yourself stays when apps change.
    host.layout_op(LayoutOp::SwitchProfile(obs.clone()))
        .unwrap();
    host.foreground_app("notepad.exe");
    assert_eq!(host.layout().unwrap().profile, obs);
}

#[test]
fn buttons_and_knobs_hold_actions() {
    let data = tempfile::tempdir().unwrap();
    let (host, rx) = start(data.path());
    let b = BUTTON_BASE;
    let d = DIAL_BASE;

    // A round button takes a key action; one past the last doesn't exist.
    host.place(b, builtin::PLUGIN, builtin::NEXT_PAGE).unwrap();
    assert!(host
        .place(b + 3, builtin::PLUGIN, builtin::NEXT_PAGE)
        .is_err());
    // Knobs take Knob, keys don't.
    assert!(host.place(d, builtin::PLUGIN, builtin::NEXT_PAGE).is_err());
    assert!(host.place(0, builtin::PLUGIN, builtin::DIAL).is_err());
    assert!(host.place(d + 2, builtin::PLUGIN, builtin::DIAL).is_err());
    host.place(d, builtin::PLUGIN, builtin::DIAL).unwrap();
    // Knobs only swap with knobs.
    host.move_key(d, 1);
    assert!(host.key_info(d).is_some());

    // Turning left runs its steps: here, add a page then go to it via the
    // next-page key on the button.
    host.layout_op(LayoutOp::AddPage).unwrap();
    let first = host.layout().unwrap().pages[0].id.clone();
    host.layout_op(LayoutOp::GoToPage(first.clone())).unwrap();
    host.key(b, true);
    host.key(b, false);
    wait_for(&rx, 5, |e| matches!(e, HostEvent::PageChanged));
    assert_ne!(host.layout().unwrap().page, first);

    // Buttons and knobs are per page; back on the first page the knob runs
    // "previous page" when turned left.
    host.layout_op(LayoutOp::GoToPage(first.clone())).unwrap();
    host.set_settings(
        d,
        serde_json::json!({ "left": [{ "action": "next-page", "settings": {} }], "right": [], "press": [] }),
    );
    host.dial(0, DialInput::Turn(-1));
    let end = Instant::now() + Duration::from_secs(5);
    while host.layout().unwrap().page == first && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_ne!(host.layout().unwrap().page, first);

    // A swipe to the right goes back a page.
    host.swipe(1);
    let end = Instant::now() + Duration::from_secs(5);
    while host.layout().unwrap().page != first && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(host.layout().unwrap().page, first);
}

#[test]
fn plugins_hear_knobs() {
    if !node_ok() {
        eprintln!("skipping: needs Node 22+ on PATH");
        return;
    }
    let data = tempfile::tempdir().unwrap();
    let (host, rx) = start(data.path());
    host.device_connected(true);
    let d = DIAL_BASE + 1;
    assert!(host
        .place(d, "com.agora.testplugin", "com.agora.testplugin.counter")
        .is_err());
    host.place(d, "com.agora.testplugin", "com.agora.testplugin.volume")
        .unwrap();
    wait_for(
        &rx,
        10,
        |e| matches!(e, HostEvent::Face { key, face } if *key == d && face.title == "hello"),
    );
    host.dial(1, DialInput::Turn(2));
    wait_for(
        &rx,
        5,
        |e| matches!(e, HostEvent::Face { key, face } if *key == d && face.title == "Encoder 2"),
    );
    host.dial(1, DialInput::Press);
    wait_for(
        &rx,
        5,
        |e| matches!(e, HostEvent::Face { key, face } if *key == d && face.title == "down"),
    );
    host.dial(1, DialInput::Turn(-1));
    wait_for(
        &rx,
        5,
        |e| matches!(e, HostEvent::Face { key, face } if *key == d && face.title == "Encoder -1 held"),
    );
    host.dial(1, DialInput::Release);
    host.dial(1, DialInput::Tap);
    wait_for(
        &rx,
        5,
        |e| matches!(e, HostEvent::Face { key, face } if *key == d && face.title == "tap"),
    );
}
