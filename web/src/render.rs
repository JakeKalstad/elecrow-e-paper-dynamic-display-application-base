use anyhow::{ensure, Result};
use image::{DynamicImage, GrayImage, ImageFormat, Luma};
use imageproc::drawing::{draw_line_segment_mut, draw_text_mut};
use serde::Deserialize;
use std::io::Cursor;
#[derive(Deserialize)]
pub struct Chart {
    pub title: String,
    pub values: Vec<f64>,
    #[serde(default)]
    pub unit: String,
}
pub fn font() -> Result<ab_glyph::FontArc> {
    let path = std::env::var("EPAPER_FONT")
        .unwrap_or_else(|_| "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf".into());
    Ok(ab_glyph::FontArc::try_from_vec(std::fs::read(path)?)?)
}
pub fn text(im: &mut GrayImage, f: &ab_glyph::FontArc, x: i32, y: i32, size: f32, s: &str) {
    draw_text_mut(im, Luma([0]), x, y, size, f, s);
}
pub fn text_fit(
    im: &mut GrayImage,
    f: &ab_glyph::FontArc,
    x: i32,
    y: i32,
    size: f32,
    width: u32,
    s: &str,
) {
    let mut label = s.to_owned();
    if imageproc::drawing::text_size(size, f, &label).0 > width {
        while !label.is_empty()
            && imageproc::drawing::text_size(size, f, &format!("{label}…")).0 > width
        {
            label.pop();
        }
        label.push('…');
    }
    text(im, f, x, y, size, &label);
}
pub fn png(im: GrayImage) -> Result<Vec<u8>> {
    let mut out = Cursor::new(Vec::new());
    DynamicImage::ImageLuma8(im).write_to(&mut out, ImageFormat::Png)?;
    Ok(out.into_inner())
}
pub fn image(bytes: &[u8]) -> Result<Vec<u8>> {
    ensure!(
        bytes.len() <= 4 * 1024 * 1024,
        "Image must be at most 4 MiB"
    );
    let mut reader = image::ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    let decoded = reader.decode()?.to_rgba8();
    // Composite transparent pixels over white before converting to monochrome.
    let mut rgb = image::RgbImage::new(decoded.width(), decoded.height());
    for (x, y, p) in decoded.enumerate_pixels() {
        let a = p[3] as u32;
        rgb.put_pixel(
            x,
            y,
            image::Rgb([0, 1, 2].map(|c| ((p[c] as u32 * a + 255 * (255 - a)) / 255) as u8)),
        );
    }
    let fitted = DynamicImage::ImageRgb8(rgb)
        .resize(792, 272, image::imageops::FilterType::Lanczos3)
        .to_luma8();
    let mut im = GrayImage::from_pixel(792, 272, Luma([255]));
    image::imageops::overlay(
        &mut im,
        &fitted,
        ((792 - fitted.width()) / 2) as i64,
        ((272 - fitted.height()) / 2) as i64,
    );
    image::imageops::dither(&mut im, &image::imageops::BiLevel);
    png(im)
}
pub fn plot(im: &mut GrayImage, values: &[f64], x: f32, y: f32, w: f32, h: f32) {
    if values.len() < 2 {
        return;
    }
    let lo = values.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let range = (hi - lo).max(hi.abs().max(1.) * 0.0001);
    let point = |i: usize| {
        (
            x + i as f32 / (values.len() - 1) as f32 * w,
            y + h - ((values[i] - lo) / range) as f32 * h,
        )
    };
    for i in 1..values.len() {
        let a = point(i - 1);
        let b = point(i);
        draw_line_segment_mut(im, a, b, Luma([0]));
        draw_line_segment_mut(im, (a.0, a.1 + 1.), (b.0, b.1 + 1.), Luma([0]));
    }
    draw_line_segment_mut(im, (x, y + h + 4.), (x + w, y + h + 4.), Luma([0]));
}
pub fn chart(c: &Chart) -> Result<Vec<u8>> {
    ensure!(
        c.title.chars().count() <= 60 && c.unit.chars().count() <= 12,
        "Title or unit too long"
    );
    ensure!(
        (2..=1000).contains(&c.values.len()),
        "Chart needs 2–1000 values"
    );
    ensure!(
        c.values.iter().all(|v| v.is_finite() && v.abs() < 1e15),
        "Chart values must be finite and below 1e15"
    );
    let mut im = GrayImage::from_pixel(792, 272, Luma([255]));
    let f = font()?;
    text_fit(&mut im, &f, 20, 10, 26., 750, &c.title);
    let lo = c.values.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = c.values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    text(
        &mut im,
        &f,
        20,
        52,
        18.,
        &format!("High {hi:.2} {}", c.unit),
    );
    plot(&mut im, &c.values, 25., 87., 735., 122.);
    text(
        &mut im,
        &f,
        20,
        226,
        18.,
        &format!(
            "Low {lo:.2}   Latest {:.2} {}    {} samples (sample order)",
            c.values.last().unwrap(),
            c.unit,
            c.values.len()
        ),
    );
    png(im)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_nan() {
        assert!(chart(&Chart {
            title: "x".into(),
            values: vec![1., f64::NAN],
            unit: "".into()
        })
        .is_err());
    }
}
