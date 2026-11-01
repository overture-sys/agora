//! Pictures that play across the whole dock: the Agora intro when a dock
//! connects, and the screen saver after the dock has sat idle.
//!
//! The dock is treated as one picture (columns x rows keys); each frame is
//! cut into key-sized tiles and sent like any key face. While a show plays,
//! key faces from the host wait (see [`active`]), and the first press only
//! stops the show.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use agora_device::keyimage;
use agora_device::models::Model;
use agora_device::worker::Command;
use agora_host::face;
use image::{imageops, Rgba, RgbaImage};
use tauri::{AppHandle, Manager};

use crate::{Host, Shared};

static ACTIVE: AtomicBool = AtomicBool::new(false);
static GEN: AtomicU64 = AtomicU64::new(0);

fn last_input() -> &'static Mutex<Instant> {
    static LAST: std::sync::OnceLock<Mutex<Instant>> = std::sync::OnceLock::new();
    LAST.get_or_init(|| Mutex::new(Instant::now()))
}

/// A show owns the keys right now.
pub fn active() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

/// The dock was used: the idle clock starts again.
pub fn touch() {
    *last_input().lock().unwrap() = Instant::now();
}

pub fn idle() -> Duration {
    last_input().lock().unwrap().elapsed()
}

/// The screen savers Agora has, as (id, name) for the editor.
pub const SAVERS: &[(&str, &str)] = &[
    ("matrix", "Matrix rain"),
    ("temple", "Agora temple"),
    ("stars", "Starfield"),
    ("clock", "Big clock"),
    ("gif", "Your own GIF"),
];

#[derive(Clone)]
pub enum Show {
    /// The temple builds itself, then the keys come back.
    Intro,
    /// Plays until the next press. `gif` is the file for the "gif" saver.
    Saver { mode: String, gif: String },
}

/// Starts a show, replacing any that is playing.
pub fn start(app: &AppHandle, show: Show) {
    let shared = app.state::<Shared>().inner().clone();
    let (model, connected) = {
        let dock = shared.lock().unwrap();
        (dock.current_model(), dock.worker.is_some())
    };
    if !connected || model.keys.is_empty() {
        return;
    }
    let gen = GEN.fetch_add(1, Ordering::SeqCst) + 1;
    ACTIVE.store(true, Ordering::SeqCst);
    let app = app.clone();
    let _ = std::thread::Builder::new()
        .name("agora-show".into())
        .spawn(move || {
            play(&shared, model, gen, show);
            // Ended on its own (the intro): give the keys back.
            if GEN.load(Ordering::SeqCst) == gen {
                finish(&app);
            }
        });
}

/// Stops a show if one is playing. True if it was.
pub fn stop(app: &AppHandle) -> bool {
    if !ACTIVE.load(Ordering::SeqCst) {
        return false;
    }
    GEN.fetch_add(1, Ordering::SeqCst);
    finish(app);
    true
}

fn finish(app: &AppHandle) {
    ACTIVE.store(false, Ordering::SeqCst);
    if let Some(h) = app.state::<Host>().lock().unwrap().as_ref() {
        h.redraw_all();
    }
}

fn play(shared: &Shared, model: &'static Model, gen: u64, show: Show) {
    let size = model.key_size();
    let (w, h) = (size * model.columns as u32, size * model.rows as u32);
    let mut painter: Box<dyn Painter> = match &show {
        Show::Intro => Box::new(Temple { intro: true }),
        Show::Saver { mode, gif } => match mode.as_str() {
            "temple" => Box::new(Temple { intro: false }),
            "stars" => Box::new(Stars::new()),
            "clock" => Box::new(BigClock),
            "gif" => match Gif::open(gif, w, h) {
                Some(g) => Box::new(g),
                None => Box::new(Matrix::new(w, h)),
            },
            _ => Box::new(Matrix::new(w, h)),
        },
    };
    let begun = Instant::now();
    let mut frame = 0u64;
    while GEN.load(Ordering::SeqCst) == gen {
        let t = begun.elapsed();
        let started = Instant::now();
        let Some((canvas, wait)) = painter.paint(w, h, t, frame) else {
            return;
        };
        for (i, screen) in model.keys.iter().enumerate() {
            let (col, row) = ((i % model.columns) as u32, (i / model.columns) as u32);
            let mut tile = imageops::crop_imm(&canvas, col * size, row * size, size, size).to_image();
            if screen.width != size || screen.height != size {
                tile = imageops::resize(&tile, screen.width, screen.height, imageops::FilterType::Triangle);
            }
            let img = image::DynamicImage::ImageRgba8(tile);
            if let Ok(jpeg) = keyimage::to_jpeg(&img, screen.width, screen.height, model.orientation) {
                if GEN.load(Ordering::SeqCst) != gen {
                    return;
                }
                match shared.lock().unwrap().worker.as_ref() {
                    Some(wk) => {
                        wk.send(Command::SetKeyJpeg(i, jpeg));
                    }
                    None => return,
                }
            }
        }
        frame += 1;
        std::thread::sleep(wait.saturating_sub(started.elapsed()));
    }
}

/// Draws one frame of a show, and how long to wait before the next.
/// `None` ends the show.
trait Painter: Send {
    fn paint(&mut self, w: u32, h: u32, t: Duration, frame: u64) -> Option<(RgbaImage, Duration)>;
}

/// A small, fast random number source.
struct Rng(u64);

impl Rng {
    fn new() -> Self {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(1, |d| d.as_nanos() as u64)
            | 1;
        Rng(seed)
    }
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    /// 0.0 to 1.0.
    fn unit(&mut self) -> f32 {
        (self.next() % 10_000) as f32 / 10_000.0
    }
}

// ---- Matrix rain ----------------------------------------------------------

/// Glyphs for the rain, 5 x 7, drawn for Agora (half-width-kana-like
/// strokes, digits and a few symbols).
const GLYPHS: &[[&str; 7]] = &[
    ["#####", "....#", "...#.", "..##.", "..#..", ".#...", "#...."],
    ["..#..", "#####", "..#..", ".###.", "#.#.#", "..#..", "..#.."],
    ["#...#", "#...#", ".#.#.", "..#..", ".#.#.", "#...#", "#...#"],
    ["#####", "#....", "####.", "....#", "....#", "#...#", ".###."],
    [".###.", "#...#", "#..##", "#.#.#", "##..#", "#...#", ".###."],
    ["..#..", ".##..", "..#..", "..#..", "..#..", "..#..", ".###."],
    ["####.", "...#.", "...#.", "#####", "...#.", "...#.", "...#."],
    ["#.#.#", "#.#.#", "#.#.#", "....#", "...#.", "..#..", "##..."],
    [".#...", "#####", ".#..#", ".#..#", ".#.#.", ".#...", ".####"],
    ["#####", "..#..", "..#..", "#####", "..#..", ".#...", "#...."],
    ["#....", "#....", "####.", "#...#", "#...#", "#...#", "####."],
    ["..#..", "..#..", "#####", "..#..", ".#.#.", "#...#", "....."],
    [".###.", "#...#", "....#", "..##.", "....#", "#...#", ".###."],
    ["#...#", "#...#", "#####", "#...#", ".#.#.", "..#..", "..#.."],
];

const CELL_W: u32 = 8;
const CELL_H: u32 = 10;

struct Drop {
    head: f32,
    speed: f32,
    length: f32,
}

struct Matrix {
    rng: Rng,
    drops: Vec<Drop>,
    /// The glyph in each cell, changed now and then.
    glyphs: Vec<u8>,
    cols: usize,
    rows: usize,
}

impl Matrix {
    fn new(w: u32, h: u32) -> Self {
        let cols = (w / CELL_W).max(1) as usize;
        let rows = (h / CELL_H).max(1) as usize + 1;
        let mut rng = Rng::new();
        let drops = (0..cols)
            .map(|_| Drop {
                head: -(rng.unit() * rows as f32 * 2.0),
                speed: 0.25 + rng.unit() * 0.6,
                length: 5.0 + rng.unit() * 12.0,
            })
            .collect();
        let glyphs = (0..cols * rows)
            .map(|_| (rng.next() % GLYPHS.len() as u64) as u8)
            .collect();
        Matrix { rng, drops, glyphs, cols, rows }
    }
}

fn draw_glyph(img: &mut RgbaImage, g: u8, x0: u32, y0: u32, color: [u8; 3]) {
    let glyph = &GLYPHS[g as usize % GLYPHS.len()];
    for (gy, line) in glyph.iter().enumerate() {
        for (gx, c) in line.bytes().enumerate() {
            if c == b'#' {
                let (x, y) = (x0 + 1 + gx as u32, y0 + 1 + gy as u32);
                if x < img.width() && y < img.height() {
                    img.put_pixel(x, y, Rgba([color[0], color[1], color[2], 255]));
                }
            }
        }
    }
}

impl Painter for Matrix {
    fn paint(&mut self, w: u32, h: u32, _t: Duration, _frame: u64) -> Option<(RgbaImage, Duration)> {
        let mut img = RgbaImage::from_pixel(w, h, Rgba([0, 0, 0, 255]));
        for _ in 0..(self.cols / 3).max(1) {
            let i = (self.rng.next() % self.glyphs.len() as u64) as usize;
            self.glyphs[i] = (self.rng.next() % GLYPHS.len() as u64) as u8;
        }
        for (c, d) in self.drops.iter_mut().enumerate() {
            d.head += d.speed;
            if d.head - d.length > self.rows as f32 {
                d.head = -(self.rng.unit() * self.rows as f32);
                d.speed = 0.25 + self.rng.unit() * 0.6;
                d.length = 5.0 + self.rng.unit() * 12.0;
            }
            let head = d.head.floor() as i64;
            for r in 0..self.rows as i64 {
                let behind = head - r;
                if behind < 0 || behind as f32 > d.length {
                    continue;
                }
                let color = if behind == 0 {
                    [200, 255, 210]
                } else {
                    let fade = 1.0 - behind as f32 / d.length;
                    [0, (60.0 + 195.0 * fade) as u8, (20.0 + 50.0 * fade) as u8]
                };
                let g = self.glyphs[r as usize * self.cols + c];
                draw_glyph(&mut img, g, c as u32 * CELL_W, r as u32 * CELL_H, color);
            }
        }
        Some((img, Duration::from_millis(90)))
    }
}

// ---- The Agora temple -----------------------------------------------------

struct Temple {
    intro: bool,
}

/// The icon's shapes on a dark ground, at time `t` of the build. `gold` is
/// which of the six keys shines.
fn temple_svg(t: f32, gold: usize, glow: f32) -> String {
    let ease = |a: f32, b: f32| ((t - a) / (b - a)).clamp(0.0, 1.0);
    let out = |x: f32| 1.0 - (1.0 - x).powi(3);
    let roof = out(ease(0.0, 0.7));
    let beam = ease(0.5, 0.9);
    let mut keys = String::new();
    let spots = [(96, 232), (208, 232), (320, 232), (96, 344), (208, 344), (320, 344)];
    for (i, (x, y)) in spots.iter().enumerate() {
        let appear = out(ease(0.8 + i as f32 * 0.12, 1.1 + i as f32 * 0.12));
        if appear <= 0.0 {
            continue;
        }
        let s = 96.0 * appear;
        let (cx, cy) = (*x as f32 + 48.0, *y as f32 + 48.0);
        let fill = if i == gold { "#D9A441" } else { "#3A302A" };
        keys.push_str(&format!(
            r##"<rect x="{:.1}" y="{:.1}" width="{s:.1}" height="{s:.1}" rx="18" fill="{fill}"/>"##,
            cx - s / 2.0,
            cy - s / 2.0
        ));
        if i == gold && glow > 0.0 {
            keys.push_str(&format!(
                r##"<rect x="{:.1}" y="{:.1}" width="{s:.1}" height="{s:.1}" rx="18" fill="#FFF3C4" opacity="{glow:.2}"/>"##,
                cx - s / 2.0,
                cy - s / 2.0
            ));
        }
    }
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="853" height="512" viewBox="-170 0 853 512"><rect x="-170" width="853" height="512" fill="#14110E"/><g transform="translate(0 {:.1})" opacity="{roof:.2}"><polygon points="256,72 440,176 72,176" fill="#A94E26"/></g><rect x="80" y="188" width="352" height="22" rx="4" fill="#A94E26" opacity="{beam:.2}"/>{keys}</svg>"##,
        -120.0 * (1.0 - roof),
    )
}

impl Painter for Temple {
    fn paint(&mut self, w: u32, h: u32, t: Duration, _frame: u64) -> Option<(RgbaImage, Duration)> {
        let t = t.as_secs_f32();
        let svg = if self.intro {
            if t > 2.6 {
                return None;
            }
            let glow = if t > 1.7 { ((t - 1.7) * std::f32::consts::PI / 0.8).sin().max(0.0) * 0.6 } else { 0.0 };
            temple_svg(t, 1, glow)
        } else {
            // Built, with the gold moving from key to key.
            let gold = ((t / 1.5) as usize * 5 + 1) % 6;
            let glow = ((t % 1.5) * std::f32::consts::PI / 1.5).sin() * 0.35;
            temple_svg(10.0, gold, glow)
        };
        let wait = if self.intro { 50 } else { 120 };
        Some((face::render_svg(&svg, w, h), Duration::from_millis(wait)))
    }
}

// ---- Starfield -----------------------------------------------------------

struct Stars {
    rng: Rng,
    stars: Vec<[f32; 3]>,
}

impl Stars {
    fn new() -> Self {
        let mut rng = Rng::new();
        let stars = (0..260)
            .map(|_| [rng.unit() * 2.0 - 1.0, rng.unit() * 2.0 - 1.0, rng.unit()])
            .collect();
        Stars { rng, stars }
    }
}

impl Painter for Stars {
    fn paint(&mut self, w: u32, h: u32, _t: Duration, _frame: u64) -> Option<(RgbaImage, Duration)> {
        let mut img = RgbaImage::from_pixel(w, h, Rgba([0, 0, 0, 255]));
        let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
        for s in &mut self.stars {
            s[2] -= 0.015;
            if s[2] <= 0.02 {
                *s = [self.rng.unit() * 2.0 - 1.0, self.rng.unit() * 2.0 - 1.0, 1.0];
            }
            let x = cx + s[0] / s[2] * cx * 0.5;
            let y = cy + s[1] / s[2] * cy * 0.5;
            let bright = ((1.0 - s[2]) * 1.6).min(1.0).mul_add(225.0, 30.0) as u8;
            let size = if s[2] < 0.3 { 2 } else { 1 };
            for dx in 0..size {
                for dy in 0..size {
                    let (px, py) = (x as i64 + dx, y as i64 + dy);
                    if px >= 0 && py >= 0 && (px as u32) < w && (py as u32) < h {
                        img.put_pixel(px as u32, py as u32, Rgba([bright, bright, bright.saturating_add(20), 255]));
                    }
                }
            }
        }
        Some((img, Duration::from_millis(60)))
    }
}

// ---- Big clock ------------------------------------------------------------

struct BigClock;

impl Painter for BigClock {
    fn paint(&mut self, w: u32, h: u32, _t: Duration, _frame: u64) -> Option<(RgbaImage, Duration)> {
        let now = chrono::Local::now();
        let svg = format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}"><rect width="{w}" height="{h}" fill="#000"/><text x="{}" y="{}" font-family="sans-serif" font-size="{}" font-weight="700" text-anchor="middle" fill="#f3ecdf">{}</text><text x="{}" y="{}" font-family="sans-serif" font-size="{}" font-weight="600" text-anchor="middle" fill="#c9b79c">{}</text></svg>"##,
            w / 2,
            h * 58 / 100,
            h * 42 / 100,
            now.format("%H:%M"),
            w / 2,
            h * 86 / 100,
            h * 12 / 100,
            now.format("%A %-d %B")
        );
        Some((face::render_svg(&svg, w, h), Duration::from_millis(1000)))
    }
}

// ---- The user's own GIF ----------------------------------------------------

struct Gif {
    frames: Vec<(RgbaImage, Duration)>,
    at: usize,
}

impl Gif {
    /// Reads a GIF and fits each frame to the dock (filling it, cropped).
    /// Big files are refused so a huge GIF can't eat memory.
    fn open(path: &str, w: u32, h: u32) -> Option<Self> {
        use image::AnimationDecoder;
        let meta = std::fs::metadata(path).ok()?;
        if meta.len() > 40 * 1024 * 1024 {
            return None;
        }
        let file = std::io::BufReader::new(std::fs::File::open(path).ok()?);
        let decoder = image::codecs::gif::GifDecoder::new(file).ok()?;
        // A small file can still claim a huge picture; refuse those.
        let (gw, gh) = image::ImageDecoder::dimensions(&decoder);
        if gw == 0 || gh == 0 || gw > 2048 || gh > 2048 {
            return None;
        }
        let mut frames = Vec::new();
        for f in decoder.into_frames().take(600) {
            let f = f.ok()?;
            let (num, den) = f.delay().numer_denom_ms();
            let ms = num.checked_div(den).map_or(100, |m| m.clamp(40, 2000));
            let img = f.into_buffer();
            let scale = (w as f32 / img.width() as f32).max(h as f32 / img.height() as f32);
            let (sw, sh) = (
                ((img.width() as f32 * scale).ceil() as u32).max(w),
                ((img.height() as f32 * scale).ceil() as u32).max(h),
            );
            let big = imageops::resize(&img, sw, sh, imageops::FilterType::Triangle);
            let fitted = imageops::crop_imm(&big, (sw - w) / 2, (sh - h) / 2, w, h).to_image();
            frames.push((fitted, Duration::from_millis(ms as u64)));
        }
        if frames.is_empty() {
            None
        } else {
            Some(Gif { frames, at: 0 })
        }
    }
}

impl Painter for Gif {
    fn paint(&mut self, _w: u32, _h: u32, _t: Duration, _frame: u64) -> Option<(RgbaImage, Duration)> {
        let (img, wait) = self.frames[self.at % self.frames.len()].clone();
        self.at += 1;
        Some((img, wait.max(Duration::from_millis(60))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_show_paints_a_dock_sized_frame() {
        let (w, h) = (320, 192);
        let mut painters: Vec<Box<dyn Painter>> = vec![
            Box::new(Matrix::new(w, h)),
            Box::new(Temple { intro: true }),
            Box::new(Temple { intro: false }),
            Box::new(Stars::new()),
            Box::new(BigClock),
        ];
        for p in &mut painters {
            for f in 0..3 {
                let (img, _) = p.paint(w, h, Duration::from_millis(f * 400), f).unwrap();
                assert_eq!((img.width(), img.height()), (w, h));
            }
        }
        // The intro ends by itself.
        assert!(Temple { intro: true }.paint(w, h, Duration::from_secs(3), 0).is_none());
    }

    #[test]
    fn matrix_rain_lights_something_up() {
        let mut m = Matrix::new(320, 192);
        let mut lit = false;
        for f in 0..40 {
            let (img, _) = m.paint(320, 192, Duration::ZERO, f).unwrap();
            lit |= img.pixels().any(|p| p.0[1] > 100);
        }
        assert!(lit);
    }
}

#[cfg(test)]
mod look {
    use super::*;

    /// Writes sample frames to /tmp/agora-shows for a look by eye.
    #[test]
    #[ignore]
    fn dump_frames() {
        let (w, h) = (320, 192);
        std::fs::create_dir_all("/tmp/agora-shows").unwrap();
        let mut m = Matrix::new(w, h);
        for f in 0..30 {
            let (img, _) = m.paint(w, h, Duration::ZERO, f).unwrap();
            if f == 29 {
                img.save("/tmp/agora-shows/matrix.png").unwrap();
            }
        }
        for (name, t) in [("intro-0.4", 0.4), ("intro-1.2", 1.2), ("intro-2.0", 2.0)] {
            let (img, _) = Temple { intro: true }.paint(w, h, Duration::from_secs_f32(t), 0).unwrap();
            img.save(format!("/tmp/agora-shows/{name}.png")).unwrap();
        }
        let mut s = Stars::new();
        for f in 0..40 {
            let (img, _) = s.paint(w, h, Duration::ZERO, f).unwrap();
            if f == 39 {
                img.save("/tmp/agora-shows/stars.png").unwrap();
            }
        }
        BigClock.paint(w, h, Duration::ZERO, 0).unwrap().0.save("/tmp/agora-shows/clock.png").unwrap();
    }
}
