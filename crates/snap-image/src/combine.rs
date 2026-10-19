//! Joins several images into one sheet.

use image::{Rgba, RgbaImage};

use crate::util::blit_over;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    Horizontal,
    Vertical,
    /// Row-major grid with `cols` columns (at least 1).
    Grid {
        cols: u32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Align {
    Start,
    #[default]
    Center,
    End,
}

impl Align {
    fn offset(self, free: u32) -> i64 {
        match self {
            Align::Start => 0,
            Align::Center => (free / 2) as i64,
            Align::End => free as i64,
        }
    }
}

/// Lays `imgs` out on a `bg` canvas with `gap` px between cells.
///
/// Along the layout direction each cell hugs its image; across it (and in both directions for
/// a grid) cells are as large as their biggest member and `align` positions the smaller images.
/// An empty list yields a 1x1 `bg` pixel.
pub fn combine(imgs: &[RgbaImage], layout: Layout, gap: u32, align: Align, bg: [u8; 4]) -> RgbaImage {
    if imgs.is_empty() {
        return RgbaImage::from_pixel(1, 1, Rgba(bg));
    }
    let n = imgs.len();
    let cols = match layout {
        Layout::Horizontal => n,
        Layout::Vertical => 1,
        Layout::Grid { cols } => (cols.max(1) as usize).min(n),
    };
    let rows = n.div_ceil(cols);

    let mut col_w = vec![0u32; cols];
    let mut row_h = vec![0u32; rows];
    for (i, img) in imgs.iter().enumerate() {
        col_w[i % cols] = col_w[i % cols].max(img.width());
        row_h[i / cols] = row_h[i / cols].max(img.height());
    }
    let span = |sizes: &[u32]| sizes.iter().sum::<u32>() + gap * (sizes.len() as u32 - 1);
    let mut canvas = RgbaImage::from_pixel(span(&col_w).max(1), span(&row_h).max(1), Rgba(bg));

    let mut y = 0i64;
    for (r, &rh) in row_h.iter().enumerate() {
        let mut x = 0i64;
        for (c, &cw) in col_w.iter().enumerate() {
            if let Some(img) = imgs.get(r * cols + c) {
                let dx = align.offset(cw - img.width());
                let dy = align.offset(rh - img.height());
                blit_over(&mut canvas, img, x + dx, y + dy);
            }
            x += (cw + gap) as i64;
        }
        y += (rh + gap) as i64;
    }
    canvas
}
