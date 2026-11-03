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
            ..Default::default()
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
            let mut line = format!("{ev:?}");
            // Pictures say nothing in a failure report.
            while let Some(at) = line.find("Some(\"data:") {
                let end = line[at..].find("\")").map_or(line.len(), |e| at + e + 2);
                line.replace_range(at..end, "<picture>");
            }
            seen.push(line.chars().take(200).collect::<String>());
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
        "GET {path_and_query} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
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
    assert_eq!(actions.len(), 3);
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
        |e| matches!(e, HostEvent::Face { key: 0, face, .. } if face.title == "hello" && face.image.is_some()),
    );

    host.key(0, true);
    host.key(0, false);
    wait_for(
        &rx,
        5,
        |e| matches!(e, HostEvent::Face { key: 0, face, .. } if face.overlay == Overlay::Ok),
    );
    wait_for(
        &rx,
        5,
        |e| matches!(e, HostEvent::Face { key: 0, face, .. } if face.title == "n=1"),
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
            |e| matches!(e, HostEvent::Face { key: 4, face, .. } if face.title == "hello"),
        );
    }
    let (host, rx) = start(data.path());
    // The plugin is started again for the saved key, and draws it.
    wait_for(
        &rx,
        10,
        |e| matches!(e, HostEvent::Face { key: 4, face, .. } if face.title == "hello"),
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
        |e| matches!(e, HostEvent::Face { key: 0, face, .. } if face.title == "hello"),
    );
    let context = host.key_info(0).unwrap().settings_url.unwrap();

    // Onto an empty key: the old key goes blank and the plugin redraws the new one.
    host.move_key(0, 3);
    wait_for(
        &rx,
        5,
        |e| matches!(e, HostEvent::Face { key: 0, face, .. } if face.image.is_none() && face.title.is_empty()),
    );
    wait_for(
        &rx,
        5,
        |e| matches!(e, HostEvent::Face { key: 3, face, .. } if face.title == "hello"),
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
        |e| matches!(e, HostEvent::Face { key: 3, face, .. } if face.title == "n=1"),
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
        |e| matches!(e, HostEvent::Face { key: 2, face, .. } if face.title == "hello"),
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
    assert_eq!(plugin_actions(&host).len(), 3);

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
        |e| matches!(e, HostEvent::Face { key: 5, face, .. } if face.title == "Games"),
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
        |e| matches!(e, HostEvent::Face { key: 5, face, .. } if face.title == "Play"),
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
        |e| matches!(e, HostEvent::Face { key: 0, face, .. } if face.title == "hello"),
    );
    host.key(0, true);
    host.key(0, false);
    wait_for(
        &rx,
        5,
        |e| matches!(e, HostEvent::Face { key: 0, face, .. } if face.title == "n=1"),
    );
    let first = host.layout().unwrap().page;

    host.layout_op(LayoutOp::AddPage).unwrap();
    wait_for(
        &rx,
        5,
        |e| matches!(e, HostEvent::Face { key: 0, face, .. } if face.title.is_empty() && face.image.is_none()),
    );
    // The plugin keeps running for the page that isn't showing.
    assert_eq!(host.plugins()[0].status, "running");
    host.layout_op(LayoutOp::GoToPage(first)).unwrap();
    // willAppear again: the plugin redraws its count.
    wait_for(
        &rx,
        5,
        |e| matches!(e, HostEvent::Face { key: 0, face, .. } if face.title == "n=1"),
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
    wait_for(&rx, 5, |e| matches!(e, HostEvent::PageChanged { .. }));
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
        |e| matches!(e, HostEvent::Face { key, face, .. } if *key == d && face.title == "hello"),
    );
    host.dial(1, DialInput::Turn(2));
    wait_for(
        &rx,
        5,
        |e| matches!(e, HostEvent::Face { key, face, .. } if *key == d && face.title == "Encoder 2"),
    );
    host.dial(1, DialInput::Press);
    wait_for(
        &rx,
        5,
        |e| matches!(e, HostEvent::Face { key, face, .. } if *key == d && face.title == "down"),
    );
    host.dial(1, DialInput::Turn(-1));
    wait_for(
        &rx,
        5,
        |e| matches!(e, HostEvent::Face { key, face, .. } if *key == d && face.title == "Encoder -1 held"),
    );
    host.dial(1, DialInput::Release);
    host.dial(1, DialInput::Tap);
    wait_for(
        &rx,
        5,
        |e| matches!(e, HostEvent::Face { key, face, .. } if *key == d && face.title == "tap"),
    );
}

#[test]
fn removals_can_be_undone_until_something_else_changes() {
    let data = tempfile::tempdir().unwrap();
    let (host, _rx) = start(data.path());

    // A removed key comes back with its settings.
    host.place(2, builtin::PLUGIN, builtin::NEXT_PAGE).unwrap();
    host.clear(2);
    assert!(host.key_info(2).is_none());
    assert!(host.undo());
    assert_eq!(host.key_info(2).unwrap().action, builtin::NEXT_PAGE);
    // Only once.
    assert!(!host.undo());

    // A removed page comes back, keys and all.
    host.layout_op(LayoutOp::AddPage).unwrap();
    let second = host.layout().unwrap().page.clone();
    host.place(4, builtin::PLUGIN, builtin::PREVIOUS_PAGE).unwrap();
    host.layout_op(LayoutOp::RemovePage(second.clone())).unwrap();
    assert_eq!(host.layout().unwrap().pages.len(), 1);
    assert!(host.undo());
    let view = host.layout().unwrap();
    assert_eq!(view.pages.len(), 2);
    assert_eq!(view.page, second);
    assert!(host.key_info(4).is_some());

    // Anything else in between ends the chance to undo.
    host.clear(4);
    host.place(5, builtin::PLUGIN, builtin::NEXT_PAGE).unwrap();
    assert!(!host.undo());
    assert!(host.key_info(4).is_none());

    // One call answers for the whole page.
    let o = host.overview().unwrap();
    assert_eq!(o.layout.page, second);
    assert_eq!(o.keys.iter().map(|k| k.key).collect::<Vec<_>>(), vec![5]);
}

#[test]
fn information_actions_go_on_keys_and_plugins_switch_profiles() {
    if !node_ok() {
        eprintln!("skipping: needs Node 22+ on PATH");
        return;
    }
    let data = tempfile::tempdir().unwrap();
    let (host, rx) = start(data.path());
    host.device_connected(true);
    host.layout_op(LayoutOp::AddProfile("Second".into())).unwrap();
    let first = host.layout().unwrap().profiles[0].id.clone();
    host.layout_op(LayoutOp::SwitchProfile(first.clone())).unwrap();

    // An "Information" action shows in the list for keys and goes on one.
    let monitor = plugin_actions(&host)
        .into_iter()
        .find(|a| a.action == "com.agora.testplugin.monitor")
        .unwrap();
    assert_eq!(monitor.controllers, vec!["Keypad".to_string()]);
    host.place(0, "com.agora.testplugin", "com.agora.testplugin.monitor")
        .unwrap();
    wait_for(
        &rx,
        10,
        |e| matches!(e, HostEvent::Face { key, face, .. } if *key == 0 && face.title == "hello"),
    );

    // Its key press asks for the profile called "Second".
    host.key(0, true);
    host.key(0, false);
    let end = Instant::now() + Duration::from_secs(5);
    while host.layout().unwrap().profile == first && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(50));
    }
    let view = host.layout().unwrap();
    let second = view.profiles.iter().find(|p| p.name == "Second").unwrap();
    assert_eq!(view.profile, second.id);
}

#[test]
fn monitor_keys_fill_in_live_readings() {
    let data = tempfile::tempdir().unwrap();
    let (host, rx) = start(data.path());
    host.device_connected(true);
    host.place(3, builtin::PLUGIN, "memory").unwrap();
    let first = match wait_for(&rx, 5, |e| matches!(e, HostEvent::Face { key: 3, .. })) {
        HostEvent::Face { face, .. } => face.image,
        _ => unreachable!(),
    };
    // The sampler reads every 2 seconds; the face then changes from "–".
    wait_for(&rx, 8, |e| {
        matches!(e, HostEvent::Face { key: 3, face, .. } if face.image.is_some() && face.image != first)
    });
}

#[test]
fn page_indicator_and_next_profile() {
    let data = tempfile::tempdir().unwrap();
    let (host, rx) = start(data.path());
    host.device_connected(true);
    host.layout_op(LayoutOp::AddPage).unwrap();
    let first_page = host.layout().unwrap().pages[0].id.clone();
    host.layout_op(LayoutOp::GoToPage(first_page)).unwrap();
    host.place(4, builtin::PLUGIN, builtin::PAGE_INDICATOR).unwrap();
    wait_for(&rx, 5, |e| matches!(e, HostEvent::Face { key: 4, face, .. } if face.title == "1 / 2"));

    host.layout_op(LayoutOp::AddProfile("Second".into())).unwrap();
    let ids: Vec<String> = host.layout().unwrap().profiles.iter().map(|p| p.id.clone()).collect();
    host.layout_op(LayoutOp::SwitchProfile(ids[0].clone())).unwrap();
    host.place(0, builtin::PLUGIN, builtin::SWITCH_PROFILE).unwrap();
    host.set_settings(0, serde_json::json!({ "profile": "next" }));
    host.key(0, true);
    host.key(0, false);
    let end = Instant::now() + Duration::from_secs(5);
    while host.layout().unwrap().profile != ids[1] {
        assert!(Instant::now() < end, "didn't move to the next profile");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn hold_actions_counters_timers_and_hotkey_switch() {
    let data = tempfile::tempdir().unwrap();
    let (host, rx) = start(data.path());
    host.device_connected(true);

    // A tap runs the key's action; a hold runs the hold steps instead.
    host.place(0, builtin::PLUGIN, "website").unwrap();
    host.set_settings(0, serde_json::json!({ "url": "https://tap.example" }));
    host.set_hold(
        0,
        vec![serde_json::json!({ "action": "website", "settings": { "url": "https://hold.example" } })],
    );
    host.key(0, true);
    host.key(0, false);
    wait_for(&rx, 5, |e| matches!(e, HostEvent::Run { settings, .. } if settings["url"] == "https://tap.example"));
    host.key(0, true);
    std::thread::sleep(Duration::from_millis(800));
    host.key(0, false);
    wait_for(&rx, 5, |e| matches!(e, HostEvent::Run { settings, .. } if settings["url"] == "https://hold.example"));

    // A counter counts taps and a hold resets it.
    host.place(1, builtin::PLUGIN, "counter").unwrap();
    for _ in 0..3 {
        host.key(1, true);
        host.key(1, false);
    }
    let end = Instant::now() + Duration::from_secs(5);
    while host.key_info(1).unwrap().settings["count"] != 3 {
        assert!(Instant::now() < end, "counter didn't reach 3");
        std::thread::sleep(Duration::from_millis(50));
    }
    host.key(1, true);
    std::thread::sleep(Duration::from_millis(800));
    host.key(1, false);
    let end = Instant::now() + Duration::from_secs(5);
    while host.key_info(1).unwrap().settings["count"] != 0 {
        assert!(Instant::now() < end, "counter didn't reset");
        std::thread::sleep(Duration::from_millis(50));
    }

    // Hotkey switch alternates its two hotkeys.
    host.place(2, builtin::PLUGIN, builtin::HOTKEY_SWITCH).unwrap();
    host.set_settings(2, serde_json::json!({ "keys": "Ctrl+A", "keys_b": "Ctrl+B" }));
    host.key(2, true);
    host.key(2, false);
    wait_for(&rx, 5, |e| matches!(e, HostEvent::Run { action, settings, .. } if action == "hotkey" && settings["keys"] == "Ctrl+A"));
    host.key(2, true);
    host.key(2, false);
    wait_for(&rx, 5, |e| matches!(e, HostEvent::Run { action, settings, .. } if action == "hotkey" && settings["keys"] == "Ctrl+B"));

    // A one-second timer runs out and says so.
    host.place(3, builtin::PLUGIN, "timer").unwrap();
    host.set_settings(3, serde_json::json!({ "minutes": 0, "seconds": 1 }));
    host.key(3, true);
    host.key(3, false);
    wait_for(&rx, 5, |e| matches!(e, HostEvent::Face { key: 3, face, .. } if face.image.as_deref().is_some_and(|i| {
        let b = i.trim_start_matches("data:image/svg+xml;base64,");
        String::from_utf8(base64::Engine::decode(&base64::engine::general_purpose::STANDARD, b).unwrap()).unwrap().contains("DONE")
    })));

    // Text an action sends back shows on its key.
    host.place(5, builtin::PLUGIN, "run-command").unwrap();
    host.key(5, true);
    host.key(5, false);
    let context = loop {
        if let HostEvent::Run { context, action, .. } = rx.recv_timeout(Duration::from_secs(5)).unwrap() {
            if action == "run-command" {
                break context;
            }
        }
    };
    host.action_result(&context, true, Some("hello there".into()));
    wait_for(&rx, 5, |e| matches!(e, HostEvent::Face { key: 5, face, .. } if face.title == "hello there"));
}

#[test]
fn copy_paste_duplicate_and_move_to_page() {
    let data = tempfile::tempdir().unwrap();
    let (host, _rx) = start(data.path());

    host.place(0, builtin::PLUGIN, "website").unwrap();
    host.set_settings(0, serde_json::json!({ "url": "https://a.example" }));
    assert!(host.paste(1).is_err(), "nothing copied yet");
    assert!(host.copy(5).is_err(), "empty key");
    host.copy(0).unwrap();
    host.paste(1).unwrap();
    host.duplicate(0, 2).unwrap();
    assert_eq!(host.key_info(1).unwrap().settings["url"], "https://a.example");
    assert_eq!(host.key_info(2).unwrap().settings["url"], "https://a.example");
    // Copies are their own actions.
    host.set_settings(1, serde_json::json!({ "url": "https://b.example" }));
    assert_eq!(host.key_info(0).unwrap().settings["url"], "https://a.example");

    // Folders can't be copied.
    host.place(3, builtin::PLUGIN, builtin::FOLDER).unwrap();
    assert!(host.copy(3).is_err());

    // Move key 1 to a new page: it lands on that page's first free key.
    host.layout_op(LayoutOp::AddPage).unwrap();
    let view = host.layout().unwrap();
    let first = view.pages[0].id.clone();
    let second = view.pages[1].id.clone();
    host.layout_op(LayoutOp::GoToPage(first.clone())).unwrap();
    assert_eq!(host.move_to_page(1, &second).unwrap(), 0);
    assert!(host.key_info(1).is_none());
    host.layout_op(LayoutOp::GoToPage(second)).unwrap();
    assert_eq!(host.key_info(0).unwrap().settings["url"], "https://b.example");
}

#[test]
fn redrawing_everything_repaints_empty_keys_too() {
    // After a screen saver, every key screen must be drawn again, including
    // the empty ones, or the saver's last frame stays on them.
    let data = tempfile::tempdir().unwrap();
    let (host, rx) = start(data.path());
    std::thread::sleep(Duration::from_millis(300));
    while rx.try_recv().is_ok() {}
    host.redraw_all();
    let mut seen = std::collections::BTreeSet::new();
    let end = Instant::now() + Duration::from_secs(5);
    while seen.len() < 15 && Instant::now() < end {
        if let Ok(HostEvent::Face { key, .. }) = rx.recv_timeout(Duration::from_millis(100)) {
            if key < 15 {
                seen.insert(key);
            }
        }
    }
    assert_eq!(seen.len(), 15, "keys redrawn: {seen:?}");
}

// ---- several devices -------------------------------------------------------

fn desc(id: &str, model: &str, rows: usize, columns: usize) -> DeviceDesc {
    DeviceDesc {
        id: id.into(),
        model: model.into(),
        name: format!("Test {model}"),
        rows,
        columns,
        ..Default::default()
    }
}

/// A host whose model last used is the M18 (as the app passes it).
fn start_m18(data: &std::path::Path) -> (HostHandle, Receiver<HostEvent>) {
    let (tx, rx) = std::sync::mpsc::channel();
    let config = Config {
        plugin_dirs: vec![(fixtures(), Source::Agora)],
        data_dir: data.to_path_buf(),
        node: None,
        device: desc("M18", "M18", 3, 5),
    };
    (HostHandle::start(config, tx).unwrap(), rx)
}

fn saved(data: &std::path::Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(data.join("layout.json")).unwrap()).unwrap()
}

const OLD_LAYOUT: &str = r#"{"profiles":[{"id":"P1","name":"Devin","current":"G1","pages":[{"id":"G1","name":"Page 1","keys":{"2":{"context":"K2","plugin":"com.agora.builtin","action":"website","settings":{"url":"https://repp.example"}}}}]}],"active":"P1","global_settings":{},"hidden":[]}"#;

#[test]
fn an_old_layout_goes_to_the_first_dock_of_its_model_and_is_backed_up() {
    let data = tempfile::tempdir().unwrap();
    std::fs::write(data.path().join("layout.json"), OLD_LAYOUT).unwrap();
    let (mut host, rx) = start_m18(data.path());
    // Kept as it was before anything is saved over it.
    assert_eq!(
        std::fs::read_to_string(data.path().join("layout.before-multi-device.json")).unwrap(),
        OLD_LAYOUT
    );

    // Another model doesn't take it: it gets a layout of its own.
    host.set_device(desc("N3-77", "N3", 2, 3));
    host.device_connected_on("N3-77", true);
    // The M18, known by its serial now, does.
    host.set_device(desc("M18-ABC", "M18", 3, 5));
    host.device_connected_on("M18-ABC", true);
    wait_for(&rx, 5, |e| {
        matches!(e, HostEvent::Face { device, key: 2, face } if device == "M18-ABC" && face.image.is_some())
    });
    let devices = host.devices();
    assert_eq!(devices.len(), 2);
    assert_eq!(devices[0].device.id, "M18-ABC");
    assert!(devices[0].selected && devices[0].connected);
    assert_eq!(host.key_info(2).unwrap().settings["url"], "https://repp.example");
    assert_eq!(host.layout().unwrap().profiles[0].name, "Devin");
    host.shutdown();

    // Saved where older Agoras still find it, with the M18 named.
    let file = saved(data.path());
    assert_eq!(file["device"]["id"], "M18-ABC");
    assert_eq!(file["profiles"][0]["name"], "Devin");
    assert_eq!(
        file["profiles"][0]["pages"][0]["keys"]["2"]["settings"]["url"],
        "https://repp.example"
    );
    assert_eq!(file["devices"]["N3-77"]["device"]["model"], "N3");
    assert_eq!(file["devices"]["N3-77"]["profiles"][0]["name"], "Default");
}

#[test]
fn each_device_keeps_its_own_keys_and_pages() {
    let data = tempfile::tempdir().unwrap();
    std::fs::write(data.path().join("layout.json"), OLD_LAYOUT).unwrap();
    let (mut host, rx) = start_m18(data.path());
    host.set_device(desc("M18-ABC", "M18", 3, 5));
    host.device_connected_on("M18-ABC", true);
    host.set_device(desc("MINI-1", "Mini", 2, 3));
    host.device_connected_on("MINI-1", true);

    // The editor still shows the M18 until told otherwise.
    assert!(host.key_info(0).is_none());
    host.select_device("MINI-1");
    assert!(host.key_info(2).is_none(), "the Mini starts empty");
    host.place(0, builtin::PLUGIN, "website").unwrap();
    host.set_settings(0, serde_json::json!({ "url": "https://mini.example" }));
    host.layout_op(LayoutOp::AddPage).unwrap();
    let pages = host.layout().unwrap().pages;
    assert_eq!(pages.len(), 2);
    host.layout_op(LayoutOp::GoToPage(pages[0].id.clone())).unwrap();

    // A press on the Mini runs the Mini's key, wherever the editor is.
    host.select_device("M18-ABC");
    assert_eq!(host.layout().unwrap().pages.len(), 1);
    host.key_on("MINI-1", 0, true);
    host.key_on("MINI-1", 0, false);
    wait_for(&rx, 5, |e| {
        matches!(e, HostEvent::Run { device, settings, .. } if device == "MINI-1" && settings["url"] == "https://mini.example")
    });
    host.key_on("M18-ABC", 2, true);
    host.key_on("M18-ABC", 2, false);
    wait_for(&rx, 5, |e| {
        matches!(e, HostEvent::Run { device, settings, .. } if device == "M18-ABC" && settings["url"] == "https://repp.example")
    });
    // A device that never connected is ignored.
    host.key_on("NOPE", 0, true);
    assert_eq!(host.key_info(2).unwrap().settings["url"], "https://repp.example");
    host.shutdown();

    // Both survive a restart, each on its own device.
    let (host, _rx) = start_m18(data.path());
    assert_eq!(host.devices().len(), 2);
    assert_eq!(host.key_info(2).unwrap().settings["url"], "https://repp.example");
    host.select_device("MINI-1");
    assert_eq!(host.key_info(0).unwrap().settings["url"], "https://mini.example");
    assert_eq!(host.layout().unwrap().pages.len(), 2);
}

#[test]
fn a_device_without_a_serial_keeps_the_model_id() {
    let data = tempfile::tempdir().unwrap();
    std::fs::write(data.path().join("layout.json"), OLD_LAYOUT).unwrap();
    let (mut host, _rx) = start_m18(data.path());
    host.set_device(desc("M18", "M18", 3, 5));
    host.device_connected_on("M18", true);
    assert_eq!(host.key_info(2).unwrap().settings["url"], "https://repp.example");
    host.shutdown();
    assert_eq!(saved(data.path())["device"]["id"], "M18");
}

#[test]
fn an_unreadable_layout_is_kept_aside() {
    let data = tempfile::tempdir().unwrap();
    std::fs::write(data.path().join("layout.json"), "{ not json").unwrap();
    let (mut host, _rx) = start_m18(data.path());
    host.shutdown();
    let kept: Vec<String> = std::fs::read_dir(data.path())
        .unwrap()
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| n.starts_with("layout.unreadable-"))
        .collect();
    assert_eq!(kept.len(), 1);
    assert_eq!(
        std::fs::read_to_string(data.path().join(&kept[0])).unwrap(),
        "{ not json"
    );
}

#[test]
fn plugins_hear_each_device_by_its_own_id() {
    if !node_ok() {
        eprintln!("skipping: needs Node 22+ on PATH");
        return;
    }
    let data = tempfile::tempdir().unwrap();
    let (_host_keep, rx) = start_m18(data.path());
    let host = &_host_keep;
    host.set_device(desc("M18-ABC", "M18", 3, 5));
    host.device_connected_on("M18-ABC", true);
    host.set_device(desc("MINI-1", "Mini", 2, 3));
    host.device_connected_on("MINI-1", true);
    host.place(0, "com.agora.testplugin", "com.agora.testplugin.counter")
        .unwrap();
    host.select_device("MINI-1");
    host.place(0, "com.agora.testplugin", "com.agora.testplugin.counter")
        .unwrap();
    wait_for(&rx, 10, |e| {
        matches!(e, HostEvent::Face { device, key: 0, face } if device == "MINI-1" && face.title == "hello")
    });
    // Same key, same action, two devices: the press and the plugin's answer
    // stay on the device pressed.
    host.key_on("MINI-1", 0, true);
    host.key_on("MINI-1", 0, false);
    wait_for(&rx, 5, |e| {
        matches!(e, HostEvent::Face { device, key: 0, face } if device == "MINI-1" && face.title == "n=1")
    });
    for n in 1..=2 {
        host.key_on("M18-ABC", 0, true);
        host.key_on("M18-ABC", 0, false);
        let title = format!("n={n}");
        wait_for(&rx, 5, |e| {
            matches!(e, HostEvent::Face { device, key: 0, face } if device == "M18-ABC" && face.title == title)
        });
    }
    wait_for(&rx, 5, |e| {
        matches!(e, HostEvent::Face { device, key: 0, face } if device == "M18-ABC" && face.overlay == Overlay::None)
    });
    assert_eq!(host.key_info(0).unwrap().settings["count"], 1);
    host.select_device("M18-ABC");
    assert_eq!(host.key_info(0).unwrap().settings["count"], 2);
}

#[test]
fn the_old_layout_stays_with_its_model_whatever_was_used_since() {
    let data = tempfile::tempdir().unwrap();
    std::fs::write(data.path().join("layout.json"), OLD_LAYOUT).unwrap();
    // First run: only another model connects. The M18 layout waits.
    let (mut host, _rx) = start_m18(data.path());
    host.set_device(desc("MINI-1", "Mini", 2, 3));
    host.device_connected_on("MINI-1", true);
    host.shutdown();
    assert_eq!(saved(data.path())["first_model"], "M18");
    // Next run the app's last model is the Mini; the layout still waits for
    // an M18, and the Mini keeps its own.
    let (tx, _rx) = std::sync::mpsc::channel();
    let config = Config {
        plugin_dirs: vec![(fixtures(), Source::Agora)],
        data_dir: data.path().to_path_buf(),
        node: None,
        device: desc("Mini", "Mini", 2, 3),
    };
    let mut host = HostHandle::start(config, tx).unwrap();
    host.set_device(desc("MINI-1", "Mini", 2, 3));
    host.set_device(desc("M18-ABC", "M18", 3, 5));
    host.select_device("M18-ABC");
    assert_eq!(host.key_info(2).unwrap().settings["url"], "https://repp.example");
    host.shutdown();
    let file = saved(data.path());
    assert_eq!(file["device"]["id"], "M18-ABC");
    assert!(file.get("first_model").is_none());
    assert!(file["devices"]["MINI-1"].is_object());
}
