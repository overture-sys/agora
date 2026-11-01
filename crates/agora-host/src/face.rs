//! Draws a key face the way the Stream Dock app does: the action's image
//! (from `setImage` or the manifest), with its title on top.
//!
//! Everything is drawn as one SVG on a 72x72 canvas, the size plugin font
//! sizes are meant for, then rendered at the device's key size.

use std::path::Path;
use std::sync::{Arc, OnceLock};

use base64::Engine;
use image::{DynamicImage, RgbaImage};
use resvg::{tiny_skia, usvg};

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

/// What to draw on one key.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Face {
    /// A `data:` URI from `setImage`, or a file from the manifest, as a data URI.
    pub image: Option<String>,
    pub title: String,
    pub show_title: bool,
    /// "top", "middle" or "bottom".
    pub align: String,
    pub font_size: f32,
    pub color: String,
    pub overlay: Overlay,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Overlay {
    #[default]
    None,
    Ok,
    Alert,
}

fn fontdb() -> Arc<usvg::fontdb::Database> {
    static DB: OnceLock<Arc<usvg::fontdb::Database>> = OnceLock::new();
    DB.get_or_init(|| {
        let mut db = usvg::fontdb::Database::new();
        db.load_system_fonts();
        db.set_sans_serif_family(if cfg!(windows) {
            "Segoe UI"
        } else {
            "DejaVu Sans"
        });
        Arc::new(db)
    })
    .clone()
}

/// Turns any image a plugin may send into a base64 data URI usvg can embed:
/// `data:image/svg+xml;charset=utf8,<percent-encoded>` becomes base64; PNG,
/// JPEG, GIF and base64 SVG pass through.
pub fn normalize_data_uri(uri: &str) -> Option<String> {
    let rest = uri.strip_prefix("data:")?;
    let (meta, data) = rest.split_once(',')?;
    let mime = meta.split(';').next().unwrap_or("").trim();
    if meta
        .split(';')
        .any(|p| p.trim().eq_ignore_ascii_case("base64"))
    {
        // Some plugins wrap base64 across lines.
        let clean: String = data.chars().filter(|c| !c.is_whitespace()).collect();
        return Some(format!("data:{mime};base64,{clean}"));
    }
    let bytes: Vec<u8> = percent_encoding::percent_decode_str(data).collect();
    Some(format!("data:{mime};base64,{}", B64.encode(bytes)))
}

/// Loads a manifest image path. Elgato-style paths may leave out the
/// extension, so try `@2x.png`, `.png` and `.svg` too.
pub fn file_data_uri(plugin_dir: &Path, rel: &str) -> Option<String> {
    if rel.is_empty() {
        return None;
    }
    let base = plugin_dir.join(rel);
    let tries = [
        base.clone(),
        base.with_file_name(format!("{}@2x.png", base.file_name()?.to_str()?)),
        base.with_extension("png"),
        base.with_extension("svg"),
    ];
    let path = tries.iter().find(|p| p.is_file())?;
    let bytes = std::fs::read(path).ok()?;
    let mime = match path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("svg") => "image/svg+xml",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        _ => "image/png",
    };
    Some(format!("data:{mime};base64,{}", B64.encode(bytes)))
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn safe_color(c: &str) -> String {
    let ok = c.starts_with('#') && c.len() <= 9 && c[1..].chars().all(|ch| ch.is_ascii_hexdigit());
    if ok {
        c.to_string()
    } else {
        "#ffffff".into()
    }
}

/// The face as one SVG document on a 72x72 canvas.
pub fn face_svg(face: &Face) -> String {
    let mut svg = String::from(
        r##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="72" height="72" viewBox="0 0 72 72"><rect width="72" height="72" fill="#000"/>"##,
    );
    if let Some(img) = face.image.as_deref().and_then(normalize_data_uri) {
        svg.push_str(&format!(
            r#"<image width="72" height="72" preserveAspectRatio="xMidYMid meet" xlink:href="{}"/>"#,
            escape(&img)
        ));
    }
    if face.show_title && !face.title.is_empty() {
        let size = if face.font_size > 0.0 {
            face.font_size.min(40.0)
        } else {
            9.0
        };
        let lines: Vec<&str> = face.title.split('\n').collect();
        let line_h = size * 1.15;
        let block = line_h * (lines.len() as f32 - 1.0);
        let first_y = match face.align.as_str() {
            "top" => size + 2.0,
            "middle" | "center" => 36.0 - block / 2.0 + size * 0.35,
            _ => 70.0 - size * 0.25 - block,
        };
        for (i, line) in lines.iter().enumerate() {
            svg.push_str(&format!(
                r##"<text x="36" y="{:.1}" font-family="sans-serif" font-size="{size}" font-weight="600" text-anchor="middle" fill="{}" stroke="#000" stroke-width="{:.1}" paint-order="stroke">{}</text>"##,
                first_y + line_h * i as f32,
                safe_color(&face.color),
                size / 5.0,
                escape(line)
            ));
        }
    }
    match face.overlay {
        Overlay::None => {}
        Overlay::Ok => svg.push_str(
            r##"<rect width="72" height="72" fill="#000" opacity="0.45"/><circle cx="36" cy="36" r="20" fill="#2f8f5b"/><path d="M26 36 l7 7 l13 -14" fill="none" stroke="#fff" stroke-width="5" stroke-linecap="round" stroke-linejoin="round"/>"##,
        ),
        Overlay::Alert => svg.push_str(
            r##"<rect width="72" height="72" fill="#000" opacity="0.45"/><path d="M36 14 L59 56 H13 Z" fill="#e8b840" stroke="#000" stroke-width="1.5" stroke-linejoin="round"/><rect x="33.5" y="28" width="5" height="15" rx="2" fill="#000"/><circle cx="36" cy="49" r="3" fill="#000"/>"##,
        ),
    }
    svg.push_str("</svg>");
    svg
}

/// Renders a face at `size` x `size` pixels. A plugin image that can't be
/// read leaves the key black rather than failing.
pub fn render(face: &Face, size: u32) -> DynamicImage {
    render_scaled(face, size, 1.0)
}

/// Renders any SVG (with system fonts for text) to a `width` x `height`
/// picture, scaled to fit. Used for pictures that span the whole dock.
pub fn render_svg(svg: &str, width: u32, height: u32) -> RgbaImage {
    let opts = usvg::Options {
        fontdb: fontdb(),
        ..Default::default()
    };
    let mut pixmap = tiny_skia::Pixmap::new(width.max(1), height.max(1)).expect("non-zero size");
    if let Ok(tree) = usvg::Tree::from_str(svg, &opts) {
        let size = tree.size();
        let scale = (width as f32 / size.width()).min(height as f32 / size.height());
        let dx = (width as f32 - size.width() * scale) / 2.0;
        let dy = (height as f32 - size.height() * scale) / 2.0;
        resvg::render(
            &tree,
            tiny_skia::Transform::from_scale(scale, scale).post_translate(dx, dy),
            &mut pixmap.as_mut(),
        );
    }
    RgbaImage::from_raw(width.max(1), height.max(1), pixmap.take()).expect("pixmap size")
}

/// Renders a face shrunk or grown by `zoom` around the key's centre, on
/// black. Used for the little bounce a built-in key gives when pressed.
pub fn render_scaled(face: &Face, size: u32, zoom: f32) -> DynamicImage {
    let svg = face_svg(face);
    let opts = usvg::Options {
        fontdb: fontdb(),
        ..Default::default()
    };
    let mut pixmap = tiny_skia::Pixmap::new(size, size).expect("non-zero key size");
    if let Ok(tree) = usvg::Tree::from_str(&svg, &opts) {
        let scale = size as f32 / 72.0 * zoom;
        let shift = size as f32 * (1.0 - zoom) / 2.0;
        resvg::render(
            &tree,
            tiny_skia::Transform::from_scale(scale, scale).post_translate(shift, shift),
            &mut pixmap.as_mut(),
        );
    }
    let img = RgbaImage::from_raw(size, size, pixmap.take()).expect("pixmap size");
    DynamicImage::ImageRgba8(img)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RED_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="144" height="144"><rect width="144" height="144" fill="#ff0000"/></svg>"##;

    fn utf8_uri(svg: &str) -> String {
        let enc = percent_encoding::utf8_percent_encode(svg, percent_encoding::NON_ALPHANUMERIC);
        format!("data:image/svg+xml;charset=utf8,{enc}")
    }

    #[test]
    fn percent_encoded_svg_renders() {
        let face = Face {
            image: Some(utf8_uri(RED_SVG)),
            ..Default::default()
        };
        let img = render(&face, 64).to_rgba8();
        let p = img.get_pixel(32, 32);
        assert!(p[0] > 200 && p[1] < 40, "centre pixel {p:?}");
    }

    #[test]
    fn base64_png_renders() {
        let mut png = Vec::new();
        let src = RgbaImage::from_pixel(8, 8, image::Rgba([0, 0, 255, 255]));
        DynamicImage::ImageRgba8(src)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let face = Face {
            image: Some(format!("data:image/png;base64,{}", B64.encode(&png))),
            ..Default::default()
        };
        let p = *render(&face, 64).to_rgba8().get_pixel(32, 32);
        assert!(p[2] > 200 && p[0] < 40, "centre pixel {p:?}");
    }

    #[test]
    fn title_is_escaped_and_bad_colours_ignored() {
        let face = Face {
            title: "a<b>&\"c\nline2".into(),
            show_title: true,
            color: "red;evil".into(),
            ..Default::default()
        };
        let svg = face_svg(&face);
        assert!(svg.contains("a&lt;b&gt;&amp;&quot;c"));
        assert!(svg.contains(r##"fill="#ffffff""##));
        assert_eq!(svg.matches("<text").count(), 2);
        usvg::Tree::from_str(&svg, &usvg::Options::default()).unwrap();
    }

    #[test]
    fn shrunk_face_leaves_black_edges() {
        let face = Face {
            image: Some(format!("data:image/svg+xml;base64,{}", B64.encode(RED_SVG))),
            ..Default::default()
        };
        let img = render_scaled(&face, 72, 0.8).to_rgba8();
        assert_eq!(img.get_pixel(36, 36).0[0], 255);
        assert_eq!(img.get_pixel(1, 1).0[..3], [0, 0, 0]);
    }

    #[test]
    fn overlays_draw() {
        let ok = render(
            &Face {
                overlay: Overlay::Ok,
                ..Default::default()
            },
            64,
        )
        .to_rgba8();
        let p = ok.get_pixel(20, 32);
        assert!(p[1] > p[0], "expected green, got {p:?}");
    }
}
