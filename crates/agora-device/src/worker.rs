//! Runs a dock on its own thread: applies commands, reads input, and sends
//! the keepalive so the dock never falls back to its stock screen.

use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::transport::Transport;
use crate::{InputEvent, Layout, Result, Rgb, M18};

/// What every model driver offers. One driver per model.
pub trait Dock: Send {
    fn layout(&self) -> Layout;
    fn open(&mut self) -> Result<()>;
    fn set_brightness(&mut self, pct: u8) -> Result<()>;
    fn keepalive(&mut self) -> Result<()>;
    fn keepalive_interval(&self) -> Duration;
    fn clear_all(&mut self) -> Result<()>;
    fn set_key_jpeg(&mut self, key: usize, jpeg: &[u8]) -> Result<()>;
    fn set_ring(&mut self, colors: &[Rgb]) -> Result<()>;
    fn set_ring_brightness(&mut self, pct: u8) -> Result<()>;
    fn ring_off(&mut self) -> Result<()>;
    fn ring_reset(&mut self) -> Result<()>;
    fn close(&mut self) -> Result<()>;
    fn read_event(&mut self, timeout_ms: i32) -> Result<Option<InputEvent>>;
}

impl<T: Transport> Dock for M18<T> {
    fn layout(&self) -> Layout {
        M18::layout(self)
    }
    fn open(&mut self) -> Result<()> {
        M18::open(self)
    }
    fn set_brightness(&mut self, pct: u8) -> Result<()> {
        M18::set_brightness(self, pct)
    }
    fn keepalive(&mut self) -> Result<()> {
        M18::keepalive(self)
    }
    fn keepalive_interval(&self) -> Duration {
        Duration::from_millis(crate::m18::KEEPALIVE_MS)
    }
    fn clear_all(&mut self) -> Result<()> {
        M18::clear_all(self)
    }
    fn set_key_jpeg(&mut self, key: usize, jpeg: &[u8]) -> Result<()> {
        M18::set_key_jpeg(self, key, jpeg)
    }
    fn set_ring(&mut self, colors: &[Rgb]) -> Result<()> {
        M18::set_ring(self, colors)
    }
    fn set_ring_brightness(&mut self, pct: u8) -> Result<()> {
        M18::set_ring_brightness(self, pct)
    }
    fn ring_off(&mut self) -> Result<()> {
        M18::ring_off(self)
    }
    fn ring_reset(&mut self) -> Result<()> {
        M18::ring_reset(self)
    }
    fn close(&mut self) -> Result<()> {
        M18::close(self)
    }
    fn read_event(&mut self, timeout_ms: i32) -> Result<Option<InputEvent>> {
        M18::read_event(self, timeout_ms)
    }
}

#[derive(Debug, Clone)]
pub enum Command {
    SetKeyJpeg(usize, Vec<u8>),
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
}

impl DockWorker {
    /// Opens the dock and starts its thread. Events arrive on the returned
    /// receiver until [`Event::Stopped`].
    pub fn start<D: Dock + 'static>(mut dock: D) -> Result<(Self, Receiver<Event>)> {
        dock.open()?;
        let layout = dock.layout();
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
        loop {
            let cmd = match rx.try_recv() {
                Ok(c) => c,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => Command::Shutdown,
            };
            let result = match cmd {
                Command::SetKeyJpeg(k, jpeg) => dock.set_key_jpeg(k, &jpeg),
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

        match dock.read_event(20) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::FakeTransport;
    use crate::{Control, M18};

    #[test]
    fn worker_reports_presses_and_closes_on_drop() {
        let mut fake = FakeTransport::default();
        let mut r = b"ACK\0\0OK\0\0".to_vec();
        r.extend_from_slice(&[0x03, 1]);
        fake.to_read.push_back(r);
        let (worker, events) = DockWorker::start(M18::new(fake)).unwrap();
        match events.recv_timeout(Duration::from_secs(2)).unwrap() {
            Event::Input(ev) => assert_eq!(ev.control, Control::Key(2)),
            other => panic!("unexpected {other:?}"),
        }
        drop(worker);
        let rest: Vec<_> = events.try_iter().collect();
        assert!(matches!(rest.last(), Some(Event::Stopped)));
    }
}
