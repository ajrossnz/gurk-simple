//! Coloured text previews of image attachments
//!
//! Images are decoded in the background into small thumbnails, which are then rendered with
//! half-block characters (`▀`): each terminal cell shows two pixels, the upper one as foreground
//! and the lower one as background color.

use std::path::Path;
use std::sync::Arc;

use image::RgbImage;
use image::imageops::FilterType;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

/// Max size of the decoded thumbnail kept in memory
const THUMBNAIL_SIZE: u32 = 160;

pub type Thumbnail = Arc<RgbImage>;

#[derive(Debug, Clone)]
pub enum PreviewState {
    Loading,
    Ready(Thumbnail),
    Failed,
}

pub fn is_image(content_type: &str) -> bool {
    matches!(
        content_type,
        "image/png" | "image/jpeg" | "image/jpg" | "image/gif" | "image/webp"
    )
}

/// Decodes the image at `path` into a small thumbnail (blocking)
pub fn load_thumbnail(path: &Path) -> Option<RgbImage> {
    let image = image::ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?
        .decode()
        .inspect_err(|error| tracing::warn!(path = %path.display(), %error, "failed to decode"))
        .ok()?;
    Some(image.thumbnail(THUMBNAIL_SIZE, THUMBNAIL_SIZE).to_rgb8())
}

/// Renders the thumbnail into at most `max_cols` x `max_rows` cells, each line prefixed by
/// `indent`
pub fn render(image: &RgbImage, max_cols: usize, max_rows: usize, indent: &str) -> Vec<Line<'static>> {
    let (w, h) = image.dimensions();
    if w == 0 || h == 0 || max_cols == 0 || max_rows == 0 {
        return Vec::new();
    }
    // each cell is roughly twice as high as wide, and holds two vertical pixels => square pixels
    let mut cols = max_cols.min(w as usize);
    let mut rows = (cols * h as usize).div_ceil(w as usize * 2);
    if rows > max_rows {
        rows = max_rows;
        cols = ((rows * 2 * w as usize) / h as usize).clamp(1, max_cols);
    }
    let rows = rows.max(1);
    let resized = image::imageops::resize(image, cols as u32, rows as u32 * 2, FilterType::Triangle);

    (0..rows as u32)
        .map(|row| {
            let mut spans = Vec::with_capacity(cols + 1);
            spans.push(Span::raw(indent.to_string()));
            for col in 0..cols as u32 {
                let top = resized.get_pixel(col, row * 2);
                let bottom = resized.get_pixel(col, row * 2 + 1);
                spans.push(Span::styled(
                    "▀",
                    Style::default()
                        .fg(Color::Rgb(top[0], top[1], top[2]))
                        .bg(Color::Rgb(bottom[0], bottom[1], bottom[2])),
                ));
            }
            Line::from(spans)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_keeps_aspect_ratio() {
        let image = RgbImage::from_pixel(100, 50, image::Rgb([255, 0, 0]));
        let lines = render(&image, 20, 10, "  ");
        // 20 cols wide => 10 px high => 5 rows
        assert_eq!(lines.len(), 5);
        assert_eq!(lines[0].spans.len(), 21);
        assert_eq!(lines[0].spans[1].style.fg, Some(Color::Rgb(255, 0, 0)));

        // limited by rows
        let tall = RgbImage::from_pixel(10, 100, image::Rgb([0, 0, 0]));
        let lines = render(&tall, 20, 4, "");
        assert_eq!(lines.len(), 4);
        assert!(lines[0].spans.len() <= 2);
    }

    #[test]
    fn load_thumbnail_from_png() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("img.png");
        RgbImage::from_pixel(400, 200, image::Rgb([1, 2, 3]))
            .save(&path)
            .unwrap();
        let thumb = load_thumbnail(&path).unwrap();
        assert_eq!(thumb.dimensions(), (160, 80));
        assert!(load_thumbnail(&tmp.path().join("missing.png")).is_none());
    }
}
