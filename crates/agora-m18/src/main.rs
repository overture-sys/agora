//! agora-m18: hardware checks for the Stream Dock M18, built on Agora's
//! device layer. Close VSD Craft before running it.
//!
//!   agora-m18 list [--all]           show the docks (or all HID devices)
//!   agora-m18 test [options]         number every key, light the ring, print presses
//!   agora-m18 orient                 find which way key images must be turned
//!   agora-m18 buttons                confirm the round button ids one at a time
//!
//! Results of `orient` and `buttons` are also written to
//! agora-m18-results.txt next to the program.

use std::io::{BufRead, Write};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use agora_device::keyimage::{self, Orientation};
use agora_device::worker::{Command, DockWorker, Event};
use agora_device::{discover, Action, Control, Rgb};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("help");
    let result = match cmd {
        "list" => list(&args),
        "test" => test(&args),
        "orient" => orient(),
        "buttons" => buttons(),
        "preview" => preview(&args),
        _ => {
            println!("{}", HELP);
            Ok(())
        }
    };
    if let Err(e) = result {
        eprintln!("Error: {e}");
        pause_if_double_clicked();
        std::process::exit(1);
    }
    pause_if_double_clicked();
}

const HELP: &str = "agora-m18: Stream Dock M18 checks (close VSD Craft first)

  agora-m18 list [--all]      show the docks found (--all: every vendor HID device)
  agora-m18 test              number every key and print presses for 30 s
      --seconds N             listen for N seconds
      --brightness N          0-100 (default 80)
      --ring                  turn the light ring blue
      --image FILE            show a PNG/JPEG on every key instead of numbers
      --orientation NAME      upright, rotate90, rotate180, rotate270, fliph, flipv
  agora-m18 orient            find which way key images must be turned
  agora-m18 buttons           confirm the three round button ids";

type R<T> = Result<T, String>;

fn flag(args: &[String], name: &str) -> bool {
    args.iter().any(|a| a == name)
}

fn value<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .map(String::as_str)
}

fn list(args: &[String]) -> R<()> {
    let api = discover::api().map_err(|e| e.to_string())?;
    let found = discover::find(&api);
    if found.is_empty() {
        println!("No M18 found (5548:1000, usage page 0xFFA0).");
    }
    for f in &found {
        println!("{}  {:?}  serial {:?}", f.model.name, f.product, f.serial);
    }
    if flag(args, "--all") {
        println!("\nVendor HID devices:");
        for line in discover::describe_all(&api, true) {
            println!("  {line}");
        }
    }
    Ok(())
}

fn connect() -> R<(DockWorker, Receiver<Event>)> {
    let api = discover::api().map_err(|e| e.to_string())?;
    let found = discover::find(&api);
    let first = found.first().ok_or(
        "No M18 found. Is it plugged in? Run `agora-m18 list --all` and send the output back.",
    )?;
    println!("Opening {} {:?}", first.model.name, first.product);
    let dock = discover::open(&api, first).map_err(|e| e.to_string())?;
    DockWorker::start(dock).map_err(|e| e.to_string())
}

fn show_faces(
    worker: &DockWorker,
    orientation: Orientation,
    face: impl Fn(usize) -> agora_device::keyimage::DynamicImage,
) -> R<()> {
    let size = worker.layout.key_size;
    for key in 0..worker.layout.key_count() {
        let jpeg =
            keyimage::to_key_jpeg(&face(key), size, orientation).map_err(|e| e.to_string())?;
        worker.send(Command::SetKeyJpeg(key, jpeg));
    }
    Ok(())
}

fn test(args: &[String]) -> R<()> {
    let seconds: u64 = value(args, "--seconds")
        .and_then(|s| s.parse().ok())
        .unwrap_or(30);
    let brightness: u8 = value(args, "--brightness")
        .and_then(|s| s.parse().ok())
        .unwrap_or(80);
    let orientation = match value(args, "--orientation") {
        Some(name) => Orientation::parse(name).ok_or(format!("unknown orientation {name}"))?,
        None => Orientation::Upright,
    };
    let picture = match value(args, "--image") {
        Some(path) => Some(image_open(path)?),
        None => None,
    };

    let (worker, events) = connect()?;
    worker.send(Command::Brightness(brightness));
    show_faces(&worker, orientation, |key| match &picture {
        Some(img) => img.clone(),
        None => keyimage::text_face(
            64,
            [20, 30, 60],
            [255, 255, 255],
            &["KEY", &(key + 1).to_string()],
        ),
    })?;
    println!("Numbered every key 1-15 (reading order).");
    if flag(args, "--ring") {
        worker.send(Command::Ring(vec![Rgb(0, 80, 255); 24]));
        println!("Turned the light ring blue.");
    }

    println!("Press keys and buttons. Listening for {seconds} s.");
    let end = Instant::now() + Duration::from_secs(seconds);
    let mut presses = 0;
    while Instant::now() < end {
        match events.recv_timeout(Duration::from_millis(200)) {
            Ok(Event::Input(ev)) => {
                println!(
                    "  0x{:02x}  {}  {}",
                    ev.raw_id,
                    match ev.action {
                        Action::Press => "DOWN".to_string(),
                        Action::Release => "up  ".to_string(),
                        Action::Turn(t) => format!("turn {t:+}"),
                        Action::Swipe(s) => format!("swipe {s:+}"),
                    },
                    name(ev.control)
                );
                if ev.action == Action::Press {
                    presses += 1;
                }
            }
            Ok(Event::Error(e)) => return Err(format!("device error: {e}")),
            Ok(Event::Stopped) => return Err("the dock stopped (unplugged?)".into()),
            Err(_) => {}
        }
    }
    drop(worker);
    println!("\nDone. {presses} press(es) seen. The dock is back on its stock screen.");
    Ok(())
}

fn orient() -> R<()> {
    let (worker, _events) = connect()?;
    println!("Each key will show a white arrow, a yellow block and its number.");
    println!("Correct means: arrow points UP, yellow block in the TOP-LEFT corner,");
    println!(
        "and the numbers run 1-5 across the top row, 6-10 in the middle, 11-15 at the bottom.\n"
    );
    let stdin = std::io::stdin();
    for o in Orientation::ALL {
        show_faces(&worker, o, |key| keyimage::orientation_face(64, key))?;
        print!("Trying '{}'. Is everything correct? [y/n/q] ", o.name());
        std::io::stdout().flush().ok();
        let mut line = String::new();
        stdin
            .lock()
            .read_line(&mut line)
            .map_err(|e| e.to_string())?;
        match line.trim().to_lowercase().as_str() {
            "y" | "yes" => {
                let msg = format!("orient: '{}' is correct", o.name());
                println!("\n{msg}");
                record(&msg);
                return Ok(());
            }
            "q" => break,
            _ => {
                print!("  What's wrong? (e.g. 'arrow points left', 'numbers out of order') ");
                std::io::stdout().flush().ok();
                let mut note = String::new();
                stdin.lock().read_line(&mut note).ok();
                record(&format!("orient: '{}' wrong: {}", o.name(), note.trim()));
            }
        }
    }
    record("orient: no orientation marked correct");
    println!("\nNo orientation was marked correct. Your notes are in agora-m18-results.txt.");
    Ok(())
}

fn buttons() -> R<()> {
    let (worker, events) = connect()?;
    worker.send(Command::Brightness(80));
    show_faces(&worker, Orientation::Upright, |_| {
        keyimage::text_face(64, [20, 30, 60], [255, 255, 255], &["BTN", "TEST"])
    })?;
    let mut ids = Vec::new();
    for which in ["LEFT", "MIDDLE", "RIGHT"] {
        println!("Press and release the {which} round button (30 s)...");
        let id = wait_for_button(&events, Duration::from_secs(30))?;
        println!("  {which} = 0x{id:02x}");
        ids.push(id);
    }
    let msg = format!(
        "buttons: left=0x{:02x} middle=0x{:02x} right=0x{:02x}",
        ids[0], ids[1], ids[2]
    );
    println!("\n{msg}");
    let matches_default = ids == [0x25, 0x30, 0x31];
    println!(
        "{}",
        if matches_default {
            "This matches Agora's default (0x25, 0x30, 0x31)."
        } else {
            "This differs from Agora's default (0x25, 0x30, 0x31); Claude will update the driver."
        }
    );
    record(&msg);
    Ok(())
}

/// Saves the test faces as PNGs (no dock needed), to check them on screen.
fn preview(args: &[String]) -> R<()> {
    let dir = std::path::PathBuf::from(args.get(1).map(String::as_str).unwrap_or("."));
    let faces = [
        ("orient-key1.png", keyimage::orientation_face(64, 0)),
        ("orient-key15.png", keyimage::orientation_face(64, 14)),
        (
            "test-key7.png",
            keyimage::text_face(64, [20, 30, 60], [255, 255, 255], &["KEY", "7"]),
        ),
        (
            "buttons.png",
            keyimage::text_face(64, [20, 30, 60], [255, 255, 255], &["BTN", "TEST"]),
        ),
    ];
    for (file, img) in faces {
        img.resize(256, 256, keyimage::FilterType::Nearest)
            .save(dir.join(file))
            .map_err(|e| e.to_string())?;
    }
    println!("Saved previews to {}", dir.display());
    Ok(())
}

fn wait_for_button(events: &Receiver<Event>, timeout: Duration) -> R<u8> {
    let end = Instant::now() + timeout;
    while Instant::now() < end {
        match events.recv_timeout(Duration::from_millis(200)) {
            Ok(Event::Input(ev)) if ev.action == Action::Press => match ev.control {
                Control::Key(k) => {
                    println!("  (that was screen key {}, press a round button)", k + 1)
                }
                _ => return Ok(ev.raw_id),
            },
            Ok(Event::Error(e)) => return Err(format!("device error: {e}")),
            Ok(Event::Stopped) => return Err("the dock stopped (unplugged?)".into()),
            _ => {}
        }
    }
    Err("no button press seen in time".into())
}

fn name(c: Control) -> String {
    match c {
        Control::Key(k) => format!("key {}", k + 1),
        Control::Button(b) => ["left button", "middle button", "right button"]
            .get(b)
            .map_or(format!("button {}", b + 1), |n| n.to_string()),
        Control::Dial(d) => format!("knob {}", d + 1),
        Control::Touch(t) => format!("touch strip segment {}", t + 1),
        Control::Strip => "touch strip".into(),
        Control::Unknown(id) => format!("unknown control 0x{id:02x}"),
    }
}

fn image_open(path: &str) -> R<agora_device::keyimage::DynamicImage> {
    agora_device::keyimage::open_image(std::path::Path::new(path)).map_err(|e| e.to_string())
}

/// Appends a line to agora-m18-results.txt next to the program.
fn record(line: &str) {
    let path = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("agora-m18-results.txt")))
        .unwrap_or_else(|| "agora-m18-results.txt".into());
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(f, "{line}");
    }
}

/// When started by double-click on Windows the console closes at once;
/// wait for Enter so the output can be read.
fn pause_if_double_clicked() {
    if cfg!(windows) && std::env::args().len() == 1 {
        print!("\nPress Enter to close.");
        std::io::stdout().flush().ok();
        let _ = std::io::stdin().lock().read_line(&mut String::new());
    }
}
