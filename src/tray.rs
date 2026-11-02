//! The notification-area icon: the week number rendered at the exact
//! SM_CXSMICON size the shell draws (8x supersampled, measured font fit,
//! box centring, horizontal condensing for bold), as the Python version's
//! IconRenderer did with Pillow. The glyph outlines come from GDI and are
//! filled the way FreeType fills them for Pillow.

use crate::gfx::{self, Color, Filter, Gray, Rgba};
use crate::os::{self, Font, Icon, Quality, Surface};

/// Supersampling factor.
const SS: usize = 8;
/// Bold may overshoot the width by this much and is then condensed.
const BOLD_MAX_CONDENSE: f64 = 1.15;

/// The physical tray icon size (SM_CXSMICON for the system DPI).
pub fn tray_icon_size() -> usize {
    let size = os::system_metric(os::SM_CXSMICON_);
    if (8..=256).contains(&size) {
        size as usize
    } else {
        (16.0 * f64::from(os::system_dpi()) / 96.0)
            .round()
            .clamp(16.0, 64.0) as usize
    }
}

/// The text box Pillow's textbbox gives for this text: horizontally from
/// the pen origin (or the ink, if it starts left of it) to the advance (or
/// the ink, if it ends right of it); vertically from the ink top to the
/// baseline (or the ink, if it hangs below). Plus the glyph coverage.
struct Glyphs {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
    mask: Gray,
}

fn render_glyphs(text: &str, font_px: i32, bold: bool) -> Option<Glyphs> {
    // Segoe UI (segoeui.ttf / segoeuib.ttf) as the Python version loaded;
    // GDI substitutes Arial where Segoe UI is missing, as its fallback did.
    let font = Font::new("Segoe UI", font_px, bold, Quality::Antialiased)?;
    let advance = font.measure(text);
    let m = font.metrics();
    let pad = font_px.max(8);
    let (w, h) = (
        (advance + 2 * pad) as usize,
        (m.linespace() + 2 * pad) as usize,
    );
    let baseline = pad + m.ascent;
    let mask = outline_mask(&font, text, pad, baseline, w, h)
        .or_else(|| gdi_mask(&font, text, pad, w, h))?;
    let (mut l, mut t, mut r, mut b) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
    for y in 0..h {
        for x in 0..w {
            if mask.px[y * w + x] != 0 {
                l = l.min(x as i32);
                r = r.max(x as i32 + 1);
                t = t.min(y as i32);
                b = b.max(y as i32 + 1);
            }
        }
    }
    if l == i32::MAX {
        return None;
    }
    Some(Glyphs {
        left: l.min(pad),
        top: t,
        right: r.max(pad + advance),
        bottom: b.max(baseline),
        mask,
    })
}

/// Pillow draws the text with FreeType: each glyph's outline at an integer
/// pen position, filled with exact area coverage and no horizontal grid
/// fitting. The same outlines come from GDI unhinted.
fn outline_mask(
    font: &Font,
    text: &str,
    pen: i32,
    baseline: i32,
    w: usize,
    h: usize,
) -> Option<Gray> {
    let mut mask = Gray {
        w,
        h,
        px: vec![0; w * h],
    };
    let mut x = pen;
    let mut buf = [0u8; 4];
    for ch in text.chars() {
        let glyph = gfx::fill_contours(
            &v40_outline(font, ch)?,
            f64::from(x),
            f64::from(baseline),
            w,
            h,
        );
        for (d, s) in mask.px.iter_mut().zip(glyph.px) {
            *d = (*d).max(s);
        }
        x += font.measure(ch.encode_utf8(&mut buf));
    }
    Some(mask)
}

/// FreeType's default (v40) TrueType hinting, which Pillow uses, moves
/// points vertically only. Approximated by GDI's grid-fitted outline for y
/// and the unhinted outline for x, when both have the same structure.
fn v40_outline(font: &Font, ch: char) -> Option<Vec<gfx::Contour>> {
    let plain = os::glyph_outline(font, ch, false)?;
    let Some(fitted) = os::glyph_outline(font, ch, true) else {
        return Some(plain);
    };
    let same = plain.len() == fitted.len()
        && plain.iter().zip(&fitted).all(|(a, b)| {
            a.segs.len() == b.segs.len()
                && a.segs.iter().zip(&b.segs).all(|(p, q)| {
                    matches!(
                        (p, q),
                        (gfx::PathSeg::Line(..), gfx::PathSeg::Line(..))
                            | (gfx::PathSeg::Quad(..), gfx::PathSeg::Quad(..))
                    )
                })
        });
    if !same {
        return Some(plain);
    }
    Some(
        plain
            .iter()
            .zip(&fitted)
            .map(|(a, b)| gfx::Contour {
                start: (a.start.0, b.start.1),
                segs: a
                    .segs
                    .iter()
                    .zip(&b.segs)
                    .map(|(p, q)| match (*p, *q) {
                        (gfx::PathSeg::Line(x, _), gfx::PathSeg::Line(_, y)) => {
                            gfx::PathSeg::Line(x, y)
                        }
                        (gfx::PathSeg::Quad(cx, _, x, _), gfx::PathSeg::Quad(_, cy, _, y)) => {
                            gfx::PathSeg::Quad(cx, cy, x, y)
                        }
                        (p, _) => p,
                    })
                    .collect(),
            })
            .collect(),
    )
}

/// Fallback when a font has no outlines: GDI's own anti-aliased text.
fn gdi_mask(font: &Font, text: &str, pad: i32, w: usize, h: usize) -> Option<Gray> {
    let mut s = Surface::new(w, h)?;
    s.fill_rect(crate::ui::rect(0, 0, w as i32, h as i32), 0);
    s.dc().text(font, pad, pad, text, 0x00FF_FFFF);
    s.flush();
    Some(Gray {
        w,
        h,
        px: s.pixels().iter().map(|p| ((p >> 8) & 0xFF) as u8).collect(),
    })
}

/// Font fit: from `max_height` px (the canvas height on Windows) down in
/// 10% steps until the text box fits the canvas width and that height;
/// returns the glyphs at that size.
fn fit(text: &str, bold: bool, canvas: usize, max_height: usize) -> Option<Glyphs> {
    let max_width = if bold {
        (canvas as f64 * BOLD_MAX_CONDENSE) as i32
    } else {
        canvas as i32
    };
    let mut font_px = max_height as i32;
    while font_px > 8 {
        let g = render_glyphs(text, font_px, bold)?;
        if g.right - g.left <= max_width && g.bottom - g.top <= max_height as i32 {
            return Some(g);
        }
        font_px = (f64::from(font_px) * 0.9) as i32;
    }
    render_glyphs(text, 8, bold)
}

/// The icon image for `number` in `color`, `size` px square.
pub fn render(number: u32, color: Color, bold: bool, size: usize) -> Option<Rgba> {
    let text = number.clamp(1, 53).to_string();
    let size = size.clamp(16, 256);
    let canvas = size * SS;
    // The font size to fit from: the whole icon, or on Linux panel text.
    let max_height = os::tray_text_height(size).clamp(8, size) * SS;
    let glyphs = fit(&text, bold, canvas, max_height)?;
    let (gw, gh) = (
        (glyphs.right - glyphs.left) as usize,
        (glyphs.bottom - glyphs.top) as usize,
    );
    // The glyph image: colour with coverage as alpha (Pillow's text on RGBA).
    let mut glyph = Rgba::new(gw, gh, [0, 0, 0, 0]);
    for y in 0..gh {
        for x in 0..gw {
            let (sx, sy) = (glyphs.left + x as i32, glyphs.top + y as i32);
            if sx >= 0 && sy >= 0 && (sx as usize) < glyphs.mask.w && (sy as usize) < glyphs.mask.h
            {
                let a = glyphs.mask.px[sy as usize * glyphs.mask.w + sx as usize];
                if a != 0 {
                    glyph.px[y * gw + x] = color.rgba(a);
                }
            }
        }
    }
    let mut width = gw;
    if gw > canvas {
        glyph = glyph.resize(canvas, gh, Filter::Lanczos);
        width = canvas;
    }
    let mut image = Rgba::new(canvas, canvas, [0, 0, 0, 0]);
    let alpha = glyph.alpha();
    image.paste_masked(
        &glyph,
        ((canvas - width) / 2) as i64,
        (canvas as i64 - gh as i64) / 2,
        &alpha,
    );
    Some(image.resize(size, size, Filter::Lanczos))
}

/// The tray icon for `number`: rendered at `size` px, and at the other
/// sizes the platform's panel picks from (each one exact, none scaled).
pub fn make_icon(number: u32, color: Color, bold: bool, size: usize) -> Option<Icon> {
    let frames: Vec<Rgba> = os::tray_icon_sizes(size)
        .into_iter()
        .filter_map(|s| render(number, color, bold, s))
        .collect();
    os::icon_from_frames(frames)
}
