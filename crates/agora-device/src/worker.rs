//! Runs a dock on its own thread: applies commands, reads input, and sends
//! the keepalive so the dock never falls back to its stock screen.

use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::JoinHandle;
use std::time::Instant;

use crate::dock::Dock;
use crate::spec::DeviceSpec;
use crate::{InputEvent, Layout, Result, Rgb};

#[derive(Debug, Clone)]
pub enum Command {
    /// A picture for a key, already encoded for the device
    /// ([`DeviceSpec::encode`]).
    SetKeyImage(usize, Vec<u8>),
    /// A picture for one segment of the touch strip.
    SetStripImage(usize, Vec<u8>),
    Brightness(u8),
    Ring(Vec<Rgb>),
    /// Ring brightness, 0 to 100.
    RingBrightness(u8),
    /// All ring LEDs off.
    RingOff,
    /// Hand the ring back to the dock's own lighting.
    RingReset,
    ClearAll,
    /// Hand the dock back to its stock screen and stop the thread.
    Shutdown,
}

#[derive(Debug, Clone)]
pub enum Event {
    Input(InputEvent),
    /// A command or read failed. The worker stops after a USB error.
    Error(String),
    Stopped,
}

pub struct DockWorker {
    tx: Sender<Command>,
    handle: Option<JoinHandle<()>>,
    pub layout: Layout,
    pub spec: DeviceSpec,
}

impl DockWorker {
    /// Opens the dock and starts its thread. Events arrive on the returned
    /// receiver until [`Event::Stopped`].
    pub fn start<D: Dock + 'static>(mut dock: D) -> Result<(Self, Receiver<Event>)> {
        dock.open()?;
        let spec = dock.spec().clone();
        let layout = Layout::of(&spec);
        let (tx, rx) = mpsc::channel();
        let (etx, erx) = mpsc::channel();
        let handle = std::thread::Builder::new()
            .name("agora-dock".into())
            .spawn(move || run(dock, rx, etx))
            .expect("spawn dock thread");
        Ok((
            Self {
                tx,
                handle: Some(handle),
                layout,
                spec,
            },
            erx,
        ))
    }

    /// Queues a command. Returns false if the worker has stopped.
    pub fn send(&self, cmd: Command) -> bool {
        self.tx.send(cmd).is_ok()
    }
}

impl Drop for DockWorker {
    fn drop(&mut self) {
        let _ = self.tx.send(Command::Shutdown);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn run<D: Dock>(mut dock: D, rx: Receiver<Command>, events: Sender<Event>) {
    // Devices that don't need a keepalive never get one.
    let interval = dock.keepalive_interval();
    let mut next_keepalive = interval.map(|i| Instant::now() + i);
    let mut ring_used = false;

    let stop = |dock: &mut D, ring_used: bool| {
        if ring_used {
            let _ = dock.ring_reset();
        }
        let _ = dock.close();
    };

    loop {
        // Take everything queued, so a burst (a page change) goes out back
        // to back and a key redrawn twice in it is only sent once.
        let mut cmds = Vec::new();
        loop {
            match rx.try_recv() {
                Ok(c) => cmds.push(c),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    cmds.push(Command::Shutdown);
                    break;
                }
            }
        }
        let busy = !cmds.is_empty();
        for cmd in coalesce(cmds) {
            let result = match cmd {
                Command::SetKeyImage(k, img) => dock.set_key_image(k, &img),
                Command::SetStripImage(s, img) => dock.set_strip_image(s, &img),
                Command::Brightness(p) => dock.set_brightness(p),
                Command::Ring(c) => {
                    ring_used = true;
                    dock.set_ring(&c)
                }
                Command::RingBrightness(p) => {
                    ring_used = true;
                    dock.set_ring_brightness(p)
                }
                Command::RingOff => {
                    ring_used = true;
                    dock.ring_off()
                }
                Command::RingReset => dock.ring_reset(),
                Command::ClearAll => dock.clear_all(),
                Command::Shutdown => {
                    stop(&mut dock, ring_used);
                    let _ = events.send(Event::Stopped);
                    return;
                }
            };
            if let Err(e) = result {
                let _ = events.send(Event::Error(e.to_string()));
                let _ = events.send(Event::Stopped);
                return;
            }
        }

        if let (Some(due), Some(every)) = (next_keepalive, interval) {
            if Instant::now() >= due {
                if let Err(e) = dock.keepalive() {
                    let _ = events.send(Event::Error(e.to_string()));
                    let _ = events.send(Event::Stopped);
                    return;
                }
                next_keepalive = Some(Instant::now() + every);
            }
        }

        // Don't sit waiting for input while pictures are still coming in.
        match dock.read_event(if busy { 0 } else { 20 }) {
            Ok(Some(ev)) => {
                if events.send(Event::Input(ev)).is_err() {
                    stop(&mut dock, ring_used);
                    return;
                }
            }
            Ok(None) => {}
            Err(e) => {
                let _ = events.send(Event::Error(e.to_string()));
                let _ = events.send(Event::Stopped);
                return;
            }
        }
    }
}

/// Drops pictures that a later one in the same batch replaces.
fn coalesce(cmds: Vec<Command>) -> Vec<Command> {
    let target = |c: &Command| match c {
        Command::SetKeyImage(k, _) => Some((0, *k)),
        Command::SetStripImage(s, _) => Some((1, *s)),
        _ => None,
    };
    let mut out: Vec<Command> = Vec::with_capacity(cmds.len());
    for c in cmds {
        if let Some(t) = target(&c) {
            out.retain(|o| target(o) != Some(t));
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::FakeTransport;
    use crate::{models, Control, StreamDock};
    use std::time::Duration;

    #[test]
    fn a_key_drawn_twice_in_a_burst_is_sent_once() {
        let out = coalesce(vec![
            Command::SetKeyImage(0, vec![1]),
            Command::Brightness(50),
            Command::SetKeyImage(1, vec![2]),
            Command::SetKeyImage(0, vec![3]),
            Command::SetStripImage(0, vec![4]),
        ]);
        let keys: Vec<(usize, u8)> = out
            .iter()
            .filter_map(|c| match c {
                Command::SetKeyImage(k, j) => Some((*k, j[0])),
                _ => None,
            })
            .collect();
        assert_eq!(keys, [(1, 2), (0, 3)]);
        assert_eq!(out.len(), 4);
    }

    /// A device that needs no keepalive and counts what it is asked to do.
    struct Quiet {
        spec: DeviceSpec,
        log: std::sync::Arc<std::sync::Mutex<Vec<&'static str>>>,
    }

    impl Dock for Quiet {
        fn spec(&self) -> &DeviceSpec {
            &self.spec
        }
        fn open(&mut self) -> Result<()> {
            self.log.lock().unwrap().push("open");
            Ok(())
        }
        fn set_brightness(&mut self, _: u8) -> Result<()> {
            self.log.lock().unwrap().push("brightness");
            Ok(())
        }
        fn keepalive(&mut self) -> Result<()> {
            self.log.lock().unwrap().push("keepalive");
            Ok(())
        }
        fn clear_all(&mut self) -> Result<()> {
            Ok(())
        }
        fn set_key_image(&mut self, _: usize, _: &[u8]) -> Result<()> {
            self.log.lock().unwrap().push("image");
            Ok(())
        }
        fn close(&mut self) -> Result<()> {
            self.log.lock().unwrap().push("close");
            Ok(())
        }
        fn read_event(&mut self, _: i32) -> Result<Option<InputEvent>> {
            std::thread::sleep(Duration::from_millis(5));
            Ok(None)
        }
    }

    #[test]
    fn any_driver_runs_in_the_worker_and_keepalive_is_optional() {
        let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let dock: Box<dyn Dock> = Box::new(Quiet {
            spec: models::N3.spec(),
            log: log.clone(),
        });
        let (worker, _events) = DockWorker::start(dock).unwrap();
        assert_eq!(worker.spec.id, "N3");
        assert_eq!(worker.layout.dials, 3);
        worker.send(Command::SetKeyImage(0, vec![1]));
        worker.send(Command::Brightness(10));
        std::thread::sleep(Duration::from_millis(100));
        drop(worker);
        let log = log.lock().unwrap();
        assert_eq!(*log, ["open", "image", "brightness", "close"]);
    }

    #[test]
    fn worker_reports_presses_and_closes_on_drop() {
        let mut fake = FakeTransport::default();
        let mut r = b"ACK\0\0OK\0\0".to_vec();
        r.extend_from_slice(&[0x03, 1]);
        fake.to_read.push_back(r);
        let (worker, events) = DockWorker::start(StreamDock::new(&models::M18, fake)).unwrap();
        match events.recv_timeout(Duration::from_secs(2)).unwrap() {
            Event::Input(ev) => assert_eq!(ev.control, Control::Key(2)),
            other => panic!("unexpected {other:?}"),
        }
        drop(worker);
        let rest: Vec<_> = events.try_iter().collect();
        assert!(matches!(rest.last(), Some(Event::Stopped)));
    }
}
