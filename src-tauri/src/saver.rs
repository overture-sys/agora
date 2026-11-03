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
    let grid = Grid { cols: model.columns as u32, rows: model.rows as u32, size };
    let mut painter: Box<dyn Painter> = match &show {
        Show::Intro => Box::new(Temple { intro: true, grid }),
        Show::Saver { mode, gif } => match mode.as_str() {
            "temple" => Box::new(Temple { intro: false, grid }),
            "stars" => Box::new(Stars::new(grid)),
            "clock" => Box::new(BigClock { grid }),
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

/// The dock's keys: how many across and down, and each one's size in the
/// canvas. Shows that draw key by key use it; the canvas is the whole dock.
#[derive(Clone, Copy)]
struct Grid {
    cols: u32,
    rows: u32,
    size: u32,
}

impl Grid {
    /// The 5 x 3 dock (M18 and alike), which has its own key-by-key layouts.
    fn five_by_three(&self) -> bool {
        self.cols == 5 && self.rows == 3
    }
    /// Top-left corner of key `i` (counted across, then down).
    fn origin(&self, i: u32) -> (u32, u32) {
        ((i % self.cols) * self.size, (i / self.cols) * self.size)
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
    grid: Grid,
}

/// The temple drawn key by key on a 5 x 3 dock: the pediment across the
/// top row's middle three keys, and the six keys of the icon on keys 7-9
/// and 12-14, the gold one on key 8.
fn temple_keys_svg(g: Grid, t: f32, gold: usize, glow: f32) -> String {
    let ease = |a: f32, b: f32| ((t - a) / (b - a)).clamp(0.0, 1.0);
    let out = |x: f32| 1.0 - (1.0 - x).powi(3);
    let (w, h, s) = (g.cols * g.size, g.rows * g.size, g.size as f32);
    let roof = out(ease(0.0, 0.7));
    let beam = ease(0.5, 0.9);
    let mut body = String::new();
    // Pediment: from key 2's left edge to key 4's right edge, on the top row.
    let (l, r, top, base) = (s * 1.08, s * 3.92, s * 0.22, s * 0.78);
    let drop = -s * (1.0 - roof);
    body.push_str(&format!(
        r##"<polygon points="{:.1},{:.1} {:.1},{:.1} {:.1},{:.1}" fill="#A94E26" opacity="{roof:.2}"/>"##,
        (l + r) / 2.0, top + drop, r, base + drop, l, base + drop
    ));
    body.push_str(&format!(
        r##"<rect x="{:.1}" y="{:.1}" width="{:.1}" height="{:.1}" rx="3" fill="#A94E26" opacity="{beam:.2}"/>"##,
        l, s * 0.84, r - l, s * 0.1
    ));
    for (n, key) in [6u32, 7, 8, 11, 12, 13].into_iter().enumerate() {
        let appear = out(ease(0.8 + n as f32 * 0.12, 1.1 + n as f32 * 0.12));
        if appear <= 0.0 {
            continue;
        }
        let (x0, y0) = g.origin(key);
        let side = s * 0.78 * appear;
        let (x, y) = (x0 as f32 + (s - side) / 2.0, y0 as f32 + (s - side) / 2.0);
        let fill = if n == gold { "#D9A441" } else { "#3A302A" };
        body.push_str(&format!(
            r##"<rect x="{x:.1}" y="{y:.1}" width="{side:.1}" height="{side:.1}" rx="{:.1}" fill="{fill}"/>"##,
            side * 0.18
        ));
        if n == gold && glow > 0.0 {
            body.push_str(&format!(
                r##"<rect x="{x:.1}" y="{y:.1}" width="{side:.1}" height="{side:.1}" rx="{:.1}" fill="#FFF3C4" opacity="{glow:.2}"/>"##,
                side * 0.18
            ));
        }
    }
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}"><rect width="{w}" height="{h}" fill="#14110E"/>{body}</svg>"##
    )
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
            if self.grid.five_by_three() {
                temple_keys_svg(self.grid, t, 1, glow)
            } else {
                temple_svg(t, 1, glow)
            }
        } else {
            // Built, with the gold moving from key to key.
            let gold = ((t / 1.5) as usize * 5 + 1) % 6;
            let glow = ((t % 1.5) * std::f32::consts::PI / 1.5).sin() * 0.35;
            if self.grid.five_by_three() {
                temple_keys_svg(self.grid, 10.0, gold, glow)
            } else {
                temple_svg(10.0, gold, glow)
            }
        };
        let wait = if self.intro { 50 } else { 120 };
        Some((face::render_svg(&svg, w, h), Duration::from_millis(wait)))
    }
}

// ---- Starfield -----------------------------------------------------------

struct Stars {
    rng: Rng,
    stars: Vec<[f32; 3]>,
    /// Still stars, at least three on every key, that twinkle: x, y, speed, phase.
    twinkles: Vec<[f32; 4]>,
}

impl Stars {
    fn new(g: Grid) -> Self {
        let mut rng = Rng::new();
        let stars = (0..260)
            .map(|_| [rng.unit() * 2.0 - 1.0, rng.unit() * 2.0 - 1.0, rng.unit()])
            .collect();
        let mut twinkles = Vec::new();
        let s = g.size as f32;
        for key in 0..g.cols * g.rows {
            let (x0, y0) = g.origin(key);
            for _ in 0..3 + (rng.next() % 2) as usize {
                twinkles.push([
                    x0 as f32 + s * (0.12 + rng.unit() * 0.76),
                    y0 as f32 + s * (0.12 + rng.unit() * 0.76),
                    1.5 + rng.unit() * 3.0,
                    rng.unit() * std::f32::consts::TAU,
                ]);
            }
        }
        Stars { rng, stars, twinkles }
    }
}

/// A square star of `size` pixels, if it's on the canvas.
fn dot(img: &mut RgbaImage, x: f32, y: f32, size: i64, c: [u8; 3]) {
    let (w, h) = (img.width() as i64, img.height() as i64);
    for dx in 0..size {
        for dy in 0..size {
            let (px, py) = (x as i64 + dx, y as i64 + dy);
            if px >= 0 && py >= 0 && px < w && py < h {
                img.put_pixel(px as u32, py as u32, Rgba([c[0], c[1], c[2], 255]));
            }
        }
    }
}

impl Painter for Stars {
    fn paint(&mut self, w: u32, h: u32, t: Duration, _frame: u64) -> Option<(RgbaImage, Duration)> {
        let mut img = RgbaImage::from_pixel(w, h, Rgba([0, 0, 0, 255]));
        let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
        let secs = t.as_secs_f32();
        for tw in &self.twinkles {
            let k = 0.5 + 0.5 * (secs * tw[2] + tw[3]).sin();
            let b = (60.0 + 195.0 * k) as u8;
            dot(&mut img, tw[0], tw[1], if k > 0.8 { 3 } else { 2 }, [b, b, b.saturating_add(25)]);
        }
        for s in &mut self.stars {
            s[2] -= 0.015;
            if s[2] <= 0.02 {
                *s = [self.rng.unit() * 2.0 - 1.0, self.rng.unit() * 2.0 - 1.0, 1.0];
            }
            let x = cx + s[0] / s[2] * cx * 0.5;
            let y = cy + s[1] / s[2] * cy * 0.5;
            let bright = ((1.0 - s[2]) * 1.6).min(1.0).mul_add(225.0, 30.0) as u8;
            let size = if s[2] < 0.3 { 3 } else { 2 };
            dot(&mut img, x, y, size, [bright, bright, bright.saturating_add(20)]);
        }
        Some((img, Duration::from_millis(60)))
    }
}

// ---- Big clock ------------------------------------------------------------

struct BigClock {
    grid: Grid,
}

/// One glyph or word, centred on key `key`.
fn key_text(g: Grid, key: u32, text: &str, size: f32, weight: u32, fill: &str) -> String {
    let (x0, y0) = g.origin(key);
    let s = g.size as f32;
    format!(
        r##"<text x="{:.1}" y="{:.1}" font-family="sans-serif" font-size="{:.1}" font-weight="{weight}" text-anchor="middle" fill="{fill}">{text}</text>"##,
        x0 as f32 + s / 2.0,
        y0 as f32 + s / 2.0 + size * 0.36,
        size
    )
}

/// The clock key by key on a 5 x 3 dock: the weekday on the top row's
/// middle keys, H H : M M across the middle row, the date on the bottom row.
fn clock_keys_svg(g: Grid, now: chrono::DateTime<chrono::Local>) -> String {
    let (w, h, s) = (g.cols * g.size, g.rows * g.size, g.size as f32);
    let mut body = String::new();
    let day = now.format("%a").to_string().to_uppercase();
    for (i, c) in day.chars().take(3).enumerate() {
        body.push_str(&key_text(g, 1 + i as u32, &c.to_string(), s * 0.5, 600, "#c9b79c"));
    }
    let hm = now.format("%H%M").to_string();
    let digits: Vec<char> = hm.chars().collect();
    for (key, d) in [(5u32, digits[0]), (6, digits[1]), (8, digits[2]), (9, digits[3])] {
        body.push_str(&key_text(g, key, &d.to_string(), s * 0.82, 700, "#f3ecdf"));
    }
    // The colon blinks with the seconds.
    let colon = if now.timestamp() % 2 == 0 { "#D9A441" } else { "#4a3f35" };
    body.push_str(&key_text(g, 7, ":", s * 0.82, 700, colon));
    body.push_str(&key_text(g, 11, &now.format("%-d").to_string(), s * 0.5, 600, "#c9b79c"));
    body.push_str(&key_text(g, 12, &now.format("%b").to_string().to_uppercase(), s * 0.36, 600, "#c9b79c"));
    body.push_str(&key_text(g, 13, &now.format("%Y").to_string(), s * 0.3, 600, "#8a7b68"));
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}"><rect width="{w}" height="{h}" fill="#000"/>{body}</svg>"##
    )
}

impl Painter for BigClock {
    fn paint(&mut self, w: u32, h: u32, _t: Duration, _frame: u64) -> Option<(RgbaImage, Duration)> {
        let now = chrono::Local::now();
        if self.grid.five_by_three() {
            return Some((face::render_svg(&clock_keys_svg(self.grid, now), w, h), Duration::from_millis(500)));
        }
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

/// Checks a picture or video the user picked: a file on this computer, not a
/// network path (reading `\\server\share\...` would send the user's
/// Windows sign-in to that server), with one of `exts`, no bigger than
/// `max_bytes`. Returns the path to read.
pub(crate) fn local_media(path: &str, exts: &[&str], max_bytes: u64) -> Result<std::path::PathBuf, String> {
    let path = path.trim();
    if path.starts_with("\\\\") || path.starts_with("//") || path.contains("://") {
        return Err("Pick a file on this computer, not a network path".into());
    }
    let p = std::path::Path::new(path);
    if !p.is_absolute() {
        return Err("Pick a file with a full path".into());
    }
    let ext = p
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    if !exts.contains(&ext.as_str()) {
        return Err(format!("Pick a {} file", exts.join(" or ").to_ascii_uppercase()));
    }
    let real = std::fs::canonicalize(p).map_err(|e| e.to_string())?;
    // A link can still point at a network share.
    let shown = real.to_string_lossy();
    if shown.starts_with("\\\\?\\UNC\\") || shown.starts_with("//") {
        return Err("Pick a file on this computer, not a network path".into());
    }
    let meta = std::fs::metadata(&real).map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Err("That isn't a file".into());
    }
    if meta.len() > max_bytes {
        return Err(format!("That file is over {} MB", max_bytes / (1024 * 1024)));
    }
    Ok(real)
}

impl Gif {
    /// Reads a GIF and fits each frame to the dock (filling it, cropped).
    /// Big files are refused so a huge GIF can't eat memory.
    fn open(path: &str, w: u32, h: u32) -> Option<Self> {
        use image::AnimationDecoder;
        let path = local_media(path, &["gif"], 40 * 1024 * 1024).ok()?;
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
    fn media_must_be_a_local_file_of_the_right_kind() {
        let dir = std::env::temp_dir().join(format!("agora-media-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let gif = dir.join("a.gif");
        std::fs::write(&gif, b"GIF89a").unwrap();
        let g = gif.to_str().unwrap();
        assert!(local_media(g, &["gif"], 1024).is_ok());
        assert!(local_media(g, &["mp4"], 1024).is_err());
        assert!(local_media(g, &["gif"], 2).is_err());
        assert!(local_media("a.gif", &["gif"], 1024).is_err());
        assert!(local_media("\\\\evil\\share\\a.gif", &["gif"], 1024).is_err());
        assert!(local_media("//evil/share/a.gif", &["gif"], 1024).is_err());
        assert!(local_media(dir.to_str().unwrap(), &["gif"], 1024).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn every_show_paints_a_dock_sized_frame() {
        let (w, h) = (320, 192);
        let g = Grid { cols: 5, rows: 3, size: 64 };
        let other = Grid { cols: 4, rows: 2, size: 80 };
        let mut painters: Vec<Box<dyn Painter>> = vec![
            Box::new(Matrix::new(w, h)),
            Box::new(Temple { intro: true, grid: g }),
            Box::new(Temple { intro: false, grid: g }),
            Box::new(Temple { intro: false, grid: other }),
            Box::new(Stars::new(g)),
            Box::new(BigClock { grid: g }),
            Box::new(BigClock { grid: other }),
        ];
        for p in &mut painters {
            for f in 0..3 {
                let (img, _) = p.paint(w, h, Duration::from_millis(f * 400), f).unwrap();
                assert_eq!((img.width(), img.height()), (w, h));
            }
        }
        // The intro ends by itself.
        assert!(Temple { intro: true, grid: g }.paint(w, h, Duration::from_secs(3), 0).is_none());
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
        let g = Grid { cols: 5, rows: 3, size: 64 };
        std::fs::create_dir_all("/tmp/agora-shows").unwrap();
        let mut m = Matrix::new(w, h);
        for f in 0..30 {
            let (img, _) = m.paint(w, h, Duration::ZERO, f).unwrap();
            if f == 29 {
                img.save("/tmp/agora-shows/matrix.png").unwrap();
            }
        }
        for (name, t) in [("intro-0.4", 0.4), ("intro-1.2", 1.2), ("intro-2.0", 2.0)] {
            let (img, _) = Temple { intro: true, grid: g }.paint(w, h, Duration::from_secs_f32(t), 0).unwrap();
            img.save(format!("/tmp/agora-shows/{name}.png")).unwrap();
        }
        let mut s = Stars::new(g);
        for f in 0..40 {
            let (img, _) = s.paint(w, h, Duration::ZERO, f).unwrap();
            if f == 39 {
                img.save("/tmp/agora-shows/stars.png").unwrap();
            }
        }
        BigClock { grid: g }.paint(w, h, Duration::ZERO, 0).unwrap().0.save("/tmp/agora-shows/clock.png").unwrap();
        Temple { intro: false, grid: g }.paint(w, h, Duration::from_secs(1), 0).unwrap().0.save("/tmp/agora-shows/temple.png").unwrap();
    }
}
