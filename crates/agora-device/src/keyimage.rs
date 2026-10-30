//! Turns pictures into key-ready JPEGs, and draws simple test faces.

use image::codecs::jpeg::JpegEncoder;
use image::{Rgb, RgbImage};

pub use image::imageops::FilterType;
pub use image::DynamicImage;

use crate::{glyphs, Error, Result};

/// How a model's screen is turned relative to the picture we send.
/// The M18 shows images upright (checked with `agora-m18 orient`, 2026-10-30).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Orientation {
    #[default]
    Upright,
    Rotate90,
    Rotate180,
    Rotate270,
    FlipH,
    FlipV,
}

impl Orientation {
    pub const ALL: [Orientation; 6] = [
        Orientation::Upright,
        Orientation::Rotate90,
        Orientation::Rotate180,
        Orientation::Rotate270,
        Orientation::FlipH,
        Orientation::FlipV,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Orientation::Upright => "upright",
            Orientation::Rotate90 => "rotate90",
            Orientation::Rotate180 => "rotate180",
            Orientation::Rotate270 => "rotate270",
            Orientation::FlipH => "fliph",
            Orientation::FlipV => "flipv",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|o| o.name() == s)
    }

    fn apply(self, img: DynamicImage) -> DynamicImage {
        match self {
            Orientation::Upright => img,
            Orientation::Rotate90 => img.rotate90(),
            Orientation::Rotate180 => img.rotate180(),
            Orientation::Rotate270 => img.rotate270(),
            Orientation::FlipH => img.fliph(),
            Orientation::FlipV => img.flipv(),
        }
    }
}

/// Scales `img` to fill a `size` x `size` square (cropping the long side),
/// turns it for the device, and encodes it as JPEG.
pub fn to_key_jpeg(img: &DynamicImage, size: u32, orientation: Orientation) -> Result<Vec<u8>> {
    let square = img.resize_to_fill(size, size, FilterType::Lanczos3);
    let turned = orientation.apply(square).to_rgb8();
    let mut out = Vec::new();
    JpegEncoder::new_with_quality(&mut out, 90)
        .encode_image(&turned)
        .map_err(|e| Error::Image(e.to_string()))?;
    Ok(out)
}

/// Loads a PNG or JPEG file.
pub fn open_image(path: &std::path::Path) -> Result<DynamicImage> {
    image::open(path).map_err(|e| Error::Image(e.to_string()))
}

/// A plain face: background colour with centred text (one or two lines).
pub fn text_face(size: u32, bg: [u8; 3], fg: [u8; 3], lines: &[&str]) -> DynamicImage {
    let mut img = RgbImage::from_pixel(size, size, Rgb(bg));
    let scale = if lines.iter().all(|l| glyphs::text_width(l, 2) <= size - 4) {
        2
    } else {
        1
    };
    let line_h = 7 * scale as i32 + 3 * scale as i32;
    let total_h = line_h * lines.len() as i32 - 3 * scale as i32;
    let mut y = (size as i32 - total_h) / 2;
    for line in lines {
        let x = (size as i32 - glyphs::text_width(line, scale) as i32) / 2;
        glyphs::draw(line, x, y, scale, |px, py| put(&mut img, px, py, fg));
        y += line_h;
    }
    DynamicImage::ImageRgb8(img)
}

/// An orientation test face: an up arrow, the key's number, and a bright
/// corner block at the top left. On a correctly turned key the arrow points
/// up and the block sits top left.
pub fn orientation_face(size: u32, key: usize) -> DynamicImage {
    let mut img = RgbImage::from_pixel(size, size, Rgb([16, 24, 48]));
    let s = size as i32;
    for y in 0..s / 5 {
        for x in 0..s / 5 {
            put(&mut img, x, y, [255, 200, 0]);
        }
    }
    glyphs::draw("^", s / 2 - 7, 4, 3, |x, y| {
        put(&mut img, x, y, [255, 255, 255])
    });
    let label = (key + 1).to_string();
    let w = glyphs::text_width(&label, 3) as i32;
    glyphs::draw(&label, (s - w) / 2, s - 7 * 3 - 6, 3, |x, y| {
        put(&mut img, x, y, [120, 220, 255])
    });
    DynamicImage::ImageRgb8(img)
}

fn put(img: &mut RgbImage, x: i32, y: i32, c: [u8; 3]) {
    if x >= 0 && y >= 0 && (x as u32) < img.width() && (y as u32) < img.height() {
        img.put_pixel(x as u32, y as u32, Rgb(c));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn makes_a_small_valid_jpeg() {
        let face = text_face(64, [0, 0, 0], [255, 255, 255], &["AGORA", "OK"]);
        let jpeg = to_key_jpeg(&face, 64, Orientation::Upright).unwrap();
        assert_eq!(&jpeg[..2], &[0xFF, 0xD8]);
        assert!(jpeg.len() < 8192, "jpeg was {} bytes", jpeg.len());
        let back = image::load_from_memory(&jpeg).unwrap();
        assert_eq!((back.width(), back.height()), (64, 64));
    }

    #[test]
    fn non_square_pictures_are_cropped_to_square() {
        let wide = DynamicImage::ImageRgb8(RgbImage::new(200, 100));
        let jpeg = to_key_jpeg(&wide, 64, Orientation::Rotate90).unwrap();
        let back = image::load_from_memory(&jpeg).unwrap();
        assert_eq!((back.width(), back.height()), (64, 64));
    }

    #[test]
    fn orientation_names_round_trip() {
        for o in Orientation::ALL {
            assert_eq!(Orientation::parse(o.name()), Some(o));
        }
    }
}
