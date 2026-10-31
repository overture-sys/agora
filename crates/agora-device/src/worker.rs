//! Runs a dock on its own thread: applies commands, reads input, and sends
//! the keepalive so the dock never falls back to its stock screen.

use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::transport::Transport;
use crate::{InputEvent, Layout, Model, Result, Rgb, StreamDock};

/// What a dock driver offers. [`StreamDock`] serves every model; tests can
/// supply their own.
pub trait Dock: Send {
    fn model(&self) -> &'static Model;
    fn open(&mut self) -> Result<()>;
    fn set_brightness(&mut self, pct: u8) -> Result<()>;
    fn keepalive(&mut self) -> Result<()>;
    fn keepalive_interval(&self) -> Duration;
    fn clear_all(&mut self) -> Result<()>;
    fn set_key_jpeg(&mut self, key: usize, jpeg: &[u8]) -> Result<()>;
    fn set_strip_jpeg(&mut self, segment: usize, jpeg: &[u8]) -> Result<()>;
    fn set_ring(&mut self, colors: &[Rgb]) -> Result<()>;
    fn set_ring_brightness(&mut self, pct: u8) -> Result<()>;
    fn ring_off(&mut self) -> Result<()>;
    fn ring_reset(&mut self) -> Result<()>;
    fn close(&mut self) -> Result<()>;
    fn read_event(&mut self, timeout_ms: i32) -> Result<Option<InputEvent>>;
}

impl<T: Transport> Dock for StreamDock<T> {
    fn model(&self) -> &'static Model {
        StreamDock::model(self)
    }
    fn keepalive_interval(&self) -> Duration {
        Duration::from_millis(crate::streamdock::KEEPALIVE_MS)
    }
    fn open(&mut self) -> Result<()> {
        StreamDock::open(self)
    }
    fn set_brightness(&mut self, pct: u8) -> Result<()> {
        StreamDock::set_brightness(self, pct)
    }
    fn keepalive(&mut self) -> Result<()> {
        StreamDock::keepalive(self)
    }
    fn clear_all(&mut self) -> Result<()> {
        StreamDock::clear_all(self)
    }
    fn set_key_jpeg(&mut self, key: usize, jpeg: &[u8]) -> Result<()> {
        StreamDock::set_key_jpeg(self, key, jpeg)
    }
    fn set_strip_jpeg(&mut self, segment: usize, jpeg: &[u8]) -> Result<()> {
        StreamDock::set_strip_jpeg(self, segment, jpeg)
    }
    fn set_ring(&mut self, colors: &[Rgb]) -> Result<()> {
        StreamDock::set_ring(self, colors)
    }
    fn set_ring_brightness(&mut self, pct: u8) -> Result<()> {
        StreamDock::set_ring_brightness(self, pct)
    }
    fn ring_off(&mut self) -> Result<()> {
        StreamDock::ring_off(self)
    }
    fn ring_reset(&mut self) -> Result<()> {
        StreamDock::ring_reset(self)
    }
    fn close(&mut self) -> Result<()> {
        StreamDock::close(self)
    }
    fn read_event(&mut self, timeout_ms: i32) -> Result<Option<InputEvent>> {
        StreamDock::read_event(self, timeout_ms)
    }
}

#[derive(Debug, Clone)]
pub enum Command {
    SetKeyJpeg(usize, Vec<u8>),
    /// A picture for one segment of the touch strip.
    SetStripJpeg(usize, Vec<u8>),
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
    pub model: &'static Model,
}

impl DockWorker {
    /// Opens the dock and starts its thread. Events arrive on the returned
    /// receiver until [`Event::Stopped`].
    pub fn start<D: Dock + 'static>(mut dock: D) -> Result<(Self, Receiver<Event>)> {
        dock.open()?;
        let model = dock.model();
        let layout = Layout::of(model);
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
                model,
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
    let interval = dock.keepalive_interval();
    let mut next_keepalive = Instant::now() + interval;
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
                Command::SetKeyJpeg(k, jpeg) => dock.set_key_jpeg(k, &jpeg),
                Command::SetStripJpeg(s, jpeg) => dock.set_strip_jpeg(s, &jpeg),
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

        if Instant::now() >= next_keepalive {
            if let Err(e) = dock.keepalive() {
                let _ = events.send(Event::Error(e.to_string()));
                let _ = events.send(Event::Stopped);
                return;
            }
            next_keepalive = Instant::now() + interval;
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
        Command::SetKeyJpeg(k, _) => Some((0, *k)),
        Command::SetStripJpeg(s, _) => Some((1, *s)),
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

    #[test]
    fn a_key_drawn_twice_in_a_burst_is_sent_once() {
        let out = coalesce(vec![
            Command::SetKeyJpeg(0, vec![1]),
            Command::Brightness(50),
            Command::SetKeyJpeg(1, vec![2]),
            Command::SetKeyJpeg(0, vec![3]),
            Command::SetStripJpeg(0, vec![4]),
        ]);
        let keys: Vec<(usize, u8)> = out
            .iter()
            .filter_map(|c| match c {
                Command::SetKeyJpeg(k, j) => Some((*k, j[0])),
                _ => None,
            })
            .collect();
        assert_eq!(keys, [(1, 2), (0, 3)]);
        assert_eq!(out.len(), 4);
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
