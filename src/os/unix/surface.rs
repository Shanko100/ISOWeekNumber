//! The back buffer every window paints into (0x00RRGGBB pixels, top-down),
//! and the device context the app's drawing code writes text through. The
//! finished buffer goes to the screen in one piece with `present`.

use super::{HWND, Rect};
use crate::gfx::Rgba;
use crate::os::Font;
use std::cell::RefCell;

pub struct Surface {
    px: RefCell<Vec<u32>>,
    pub w: usize,
    pub h: usize,
}

/// Where drawing goes: a back buffer, or (inside `paint`) a window.
pub enum Dc<'a> {
    Pixels(&'a RefCell<Vec<u32>>, usize, usize),
    Window(HWND),
}

impl Surface {
    pub fn new(w: usize, h: usize) -> Option<Surface> {
        let (w, h) = (w.max(1), h.max(1));
        if w.checked_mul(h)? > 64 << 20 {
            return None;
        }
        Some(Surface {
            px: RefCell::new(vec![0; w * h]),
            w,
            h,
        })
    }

    pub fn dc(&self) -> Dc<'_> {
        Dc::Pixels(&self.px, self.w, self.h)
    }

    /// The pixels, 0x00RRGGBB, row-major, top-down.
    pub fn pixels(&mut self) -> &mut [u32] {
        self.px.get_mut()
    }

    /// Kept for the GDI backend's batching; drawing here is immediate.
    pub fn flush(&self) {}

    pub fn fill_rect(&mut self, r: Rect, rgb: u32) {
        let (w, h) = (self.w as i32, self.h as i32);
        let (x0, x1) = (r.left.clamp(0, w), r.right.clamp(0, w));
        let (y0, y1) = (r.top.clamp(0, h), r.bottom.clamp(0, h));
        let wu = self.w;
        let px = self.px.get_mut();
        for y in y0..y1 {
            px[y as usize * wu + x0 as usize..y as usize * wu + x1 as usize].fill(rgb);
        }
    }

    /// Copy an opaque image (alpha ignored) with its top-left at (x, y).
    pub fn blit(&mut self, img: &Rgba, x: i32, y: i32) {
        let (sw, sh, wu) = (self.w as i32, self.h as i32, self.w);
        let px = self.px.get_mut();
        for iy in 0..img.h as i32 {
            let ty = y + iy;
            if ty < 0 || ty >= sh {
                continue;
            }
            for ix in 0..img.w as i32 {
                let tx = x + ix;
                if tx < 0 || tx >= sw {
                    continue;
                }
                let p = img.px[iy as usize * img.w + ix as usize];
                px[ty as usize * wu + tx as usize] =
                    (u32::from(p[0]) << 16) | (u32::from(p[1]) << 8) | u32::from(p[2]);
            }
        }
    }

    /// Draw an image with alpha over what is already there (Tk's
    /// truncating photo-image blend).
    pub fn blend(&mut self, img: &Rgba, x: i32, y: i32) {
        let (sw, sh, wu) = (self.w as i32, self.h as i32, self.w);
        let px = self.px.get_mut();
        for iy in 0..img.h as i32 {
            let ty = y + iy;
            if ty < 0 || ty >= sh {
                continue;
            }
            for ix in 0..img.w as i32 {
                let tx = x + ix;
                if tx < 0 || tx >= sw {
                    continue;
                }
                let p = img.px[iy as usize * img.w + ix as usize];
                let a = u32::from(p[3]);
                if a == 0 {
                    continue;
                }
                let d = &mut px[ty as usize * wu + tx as usize];
                let un = 255 - a;
                let mix = |bg: u32, c: u8| (bg * un + u32::from(c) * a) / 255;
                *d = (mix((*d >> 16) & 0xFF, p[0]) << 16)
                    | (mix((*d >> 8) & 0xFF, p[1]) << 8)
                    | mix(*d & 0xFF, p[2]);
            }
        }
    }

    /// Show the buffer in the window `paint` handed over.
    pub fn present(&self, target: Dc<'_>) {
        if let Dc::Window(h) = target {
            crate::os::backend::present_pixels(h, &self.px.borrow(), self.w, self.h);
        }
    }
}

impl Dc<'_> {
    /// Text at (x, y) = top-left of the line box; `color` is a COLORREF
    /// (0x00BBGGRR) as the GDI backend takes it.
    pub fn text(self, font: &Font, x: i32, y: i32, s: &str, color: u32) {
        self.draw(font, x, y, s, color, None);
    }

    /// Text as text() draws it, clipped to `clip`.
    pub fn text_clipped(self, font: &Font, x: i32, y: i32, s: &str, color: u32, clip: Rect) {
        self.draw(font, x, y, s, color, Some(clip));
    }

    fn draw(self, font: &Font, x: i32, y: i32, s: &str, color: u32, clip: Option<Rect>) {
        if let Dc::Pixels(px, w, h) = self {
            let rgb = ((color & 0xFF) << 16) | (color & 0xFF00) | ((color >> 16) & 0xFF);
            let full = Rect {
                left: 0,
                top: 0,
                right: w as i32,
                bottom: h as i32,
            };
            let clip = clip.map_or(full, |c| intersect(c, full));
            font.draw_text(&mut px.borrow_mut(), w, clip, x, y, s, rgb);
        }
    }

    /// A Tk canvas oval with fill and 1 px outline on the box (x1, y1)-(x2,
    /// y2): the filled ellipse and its outline ring, unantialiased as GDI's
    /// Pie and Arc draw them.
    pub fn tk_oval(self, x1: i32, y1: i32, x2: i32, y2: i32, fill: u32, outline: u32) {
        let Dc::Pixels(px, w, h) = self else { return };
        let (bw, bh) = ((x2 - x1 + 1).max(1) as usize, (y2 - y1 + 1).max(1) as usize);
        let mut inside = crate::gfx::Gray::new(bw, bh, 0);
        inside.fill(
            &crate::pil::Ellipse(0.0, 0.0, bw as f64 - 1.0, bh as f64 - 1.0),
            255,
        );
        let mut ring = crate::gfx::Gray::new(bw, bh, 0);
        ring.fill(
            &crate::pil::EllipseOutline(0.0, 0.0, bw as f64 - 1.0, bh as f64 - 1.0, 1),
            255,
        );
        let swap = |c: u32| ((c & 0xFF) << 16) | (c & 0xFF00) | ((c >> 16) & 0xFF);
        let (fill, outline) = (swap(fill), swap(outline));
        let mut px = px.borrow_mut();
        for yy in 0..bh {
            for xx in 0..bw {
                let (tx, ty) = (x1 + xx as i32, y1 + yy as i32);
                if tx < 0 || ty < 0 || tx >= w as i32 || ty >= h as i32 {
                    continue;
                }
                let i = yy * bw + xx;
                let c = if ring.px[i] != 0 {
                    outline
                } else if inside.px[i] != 0 {
                    fill
                } else {
                    continue;
                };
                px[ty as usize * w + tx as usize] = c;
            }
        }
    }
}

pub fn intersect(a: Rect, b: Rect) -> Rect {
    Rect {
        left: a.left.max(b.left),
        top: a.top.max(b.top),
        right: a.right.min(b.right),
        bottom: a.bottom.min(b.bottom),
    }
}

/// Blend `rgb` with coverage `a` (0-255) into one pixel.
#[inline]
pub fn blend_pixel(d: &mut u32, rgb: u32, a: u32) {
    if a == 0 {
        return;
    }
    if a >= 255 {
        *d = rgb;
        return;
    }
    let un = 255 - a;
    let mix = |bg: u32, c: u32| (bg * un + c * a + 127) / 255;
    *d = (mix((*d >> 16) & 0xFF, (rgb >> 16) & 0xFF) << 16)
        | (mix((*d >> 8) & 0xFF, (rgb >> 8) & 0xFF) << 8)
        | mix(*d & 0xFF, rgb & 0xFF);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fills_blits_and_blends() {
        let mut s = Surface::new(4, 3).unwrap();
        s.fill_rect(
            Rect {
                left: 1,
                top: 1,
                right: 9,
                bottom: 2,
            },
            0x123456,
        );
        assert_eq!(&s.pixels()[4..8], &[0, 0x123456, 0x123456, 0x123456]);
        let img = Rgba::new(2, 1, [255, 0, 0, 128]);
        s.blend(&img, -1, 0);
        assert_eq!(s.pixels()[0], 0x800000);
        s.blit(&img, 3, 2);
        assert_eq!(s.pixels()[11], 0xFF0000);
        let mut d = 0x000000;
        blend_pixel(&mut d, 0xFFFFFF, 255);
        assert_eq!(d, 0xFFFFFF);
        blend_pixel(&mut d, 0x000000, 128);
        assert_eq!(d, 0x7F7F7F);
        assert!(Surface::new(100_000, 100_000).is_none());
    }

    #[test]
    fn oval_has_outline_and_fill() {
        let s = Surface::new(9, 9).unwrap();
        s.dc().tk_oval(0, 0, 8, 8, 0x0000FF, 0x00FF00);
        let px = s.px.borrow();
        assert_eq!(px[4 * 9 + 4], 0xFF0000);
        assert_eq!(px[4 * 9], 0x00FF00);
        assert_eq!(px[0], 0);
    }
}
