//! Text on macOS: Core Text fonts at pixel sizes, measured and drawn as
//! grayscale coverage into the app's back buffers, and glyph outlines for
//! the menu bar icon. The UI's Windows family names map to the system font
//! (San Francisco) and the system monospaced font.
#![allow(unsafe_code)]

use super::objc::{self, CGPathElement, CGPoint};
use crate::gfx::{Contour, PathSeg};
use crate::os::unix::surface::blend_pixel;
use crate::os::{FontMetrics, Quality, Rect};
use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;

/// kCTFontUIFontSystem, kCTFontUIFontUserFixedPitch.
const UI_FONT_SYSTEM: u32 = 2;
const UI_FONT_FIXED: u32 = 1;
const BOLD_TRAIT: u32 = 1 << 1;

pub struct Font {
    font: *const c_void,
    metrics: FontMetrics,
    widths: RefCell<HashMap<String, i32>>,
}

impl Drop for Font {
    fn drop(&mut self) {
        objc::cf_release(self.font);
    }
}

fn is_mono_name(family: &str) -> bool {
    let f = family.to_lowercase();
    ["mono", "courier", "consolas", "cascadia", "menlo", "code"]
        .iter()
        .any(|k| f.contains(k))
}

impl Font {
    pub fn new(family: &str, px: i32, bold: bool, _quality: Quality) -> Option<Font> {
        let size = f64::from(px.clamp(1, 1024));
        // SAFETY: Core Text constructors return +1 references (or NULL),
        // owned by the Font and released in Drop.
        let font = unsafe {
            let base = if is_mono_name(family) {
                objc::CTFontCreateUIFontForLanguage(UI_FONT_FIXED, size, std::ptr::null())
            } else {
                objc::CTFontCreateUIFontForLanguage(UI_FONT_SYSTEM, size, std::ptr::null())
            };
            if base.is_null() {
                return None;
            }
            if bold {
                let b = objc::CTFontCreateCopyWithSymbolicTraits(
                    base,
                    0.0,
                    std::ptr::null(),
                    BOLD_TRAIT,
                    BOLD_TRAIT,
                );
                if b.is_null() {
                    base
                } else {
                    objc::CFRelease(base);
                    b
                }
            } else {
                base
            }
        };
        // SAFETY: a valid font.
        let (a, d) = unsafe { (objc::CTFontGetAscent(font), objc::CTFontGetDescent(font)) };
        Some(Font {
            font,
            metrics: FontMetrics {
                ascent: a.round() as i32,
                descent: d.round() as i32,
            },
            widths: RefCell::new(HashMap::new()),
        })
    }

    pub fn metrics(&self) -> FontMetrics {
        self.metrics
    }

    /// A Core Text line for `s` (+1), drawing in the context's fill colour.
    fn line(&self, s: &str) -> *const c_void {
        let text = objc::cfstring(s);
        // SAFETY: a two-entry attribute dictionary of CF objects, with the
        // standard CF callbacks; every created object is released here or
        // owned by the returned line.
        unsafe {
            let yes: objc::Id = objc::send1(objc::class(c"NSNumber"), c"numberWithBool:", true);
            let keys = [
                objc::kCTFontAttributeName,
                objc::kCTForegroundColorFromContextAttributeName,
            ];
            let values = [self.font, yes as *const c_void];
            let attrs = objc::CFDictionaryCreate(
                std::ptr::null(),
                keys.as_ptr(),
                values.as_ptr(),
                2,
                &raw const objc::kCFTypeDictionaryKeyCallBacks,
                &raw const objc::kCFTypeDictionaryValueCallBacks,
            );
            let attributed = objc::CFAttributedStringCreate(std::ptr::null(), text, attrs);
            let line = objc::CTLineCreateWithAttributedString(attributed);
            objc::cf_release(attributed);
            objc::cf_release(attrs);
            objc::cf_release(text);
            line
        }
    }

    /// Text width in pixels.
    pub fn measure(&self, s: &str) -> i32 {
        if s.is_empty() {
            return 0;
        }
        if let Some(w) = self.widths.borrow().get(s) {
            return *w;
        }
        let line = self.line(s);
        if line.is_null() {
            return 0;
        }
        // SAFETY: a valid line; the out-parameters may be NULL.
        let w = unsafe {
            objc::CTLineGetTypographicBounds(
                line,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        objc::cf_release(line);
        let w = w.ceil() as i32;
        let mut cache = self.widths.borrow_mut();
        if cache.len() > 256 {
            cache.clear();
        }
        cache.insert(s.to_string(), w);
        w
    }

    /// The font pointer as a handle (the edit field's WM_SETFONT).
    pub fn handle(&self) -> isize {
        self.font as isize
    }

    /// Draw `s` with the top of its line box at (x, y) into `px` (w wide).
    #[allow(clippy::too_many_arguments)]
    pub fn draw_text(
        &self,
        px: &mut [u32],
        w: usize,
        clip: Rect,
        x: i32,
        y: i32,
        s: &str,
        rgb: u32,
    ) {
        if s.is_empty() {
            return;
        }
        let tw = self.measure(s) + 4;
        let th = self.metrics.ascent + self.metrics.descent + 4;
        let (bw, bh) = (tw.max(1) as usize, th.max(1) as usize);
        let mut mask = vec![0u8; bw * bh];
        let line = self.line(s);
        if line.is_null() {
            return;
        }
        // SAFETY: an 8-bit alpha-only bitmap context over `mask`, which
        // outlives the context; the line is drawn white (coverage) at its
        // baseline, then context and line are released.
        unsafe {
            let ctx = objc::CGBitmapContextCreate(
                mask.as_mut_ptr().cast(),
                bw,
                bh,
                8,
                bw,
                std::ptr::null(),
                7, /* kCGImageAlphaOnly */
            );
            if !ctx.is_null() {
                objc::CGContextSetGrayFillColor(ctx, 1.0, 1.0);
                objc::CGContextSetAllowsFontSubpixelPositioning(ctx, true);
                objc::CGContextSetShouldSubpixelPositionFonts(ctx, true);
                // Core Graphics' origin is bottom-left: the baseline sits
                // `descent + 2` px above the bitmap's bottom.
                objc::CGContextSetTextPosition(ctx, 2.0, f64::from(self.metrics.descent + 2));
                objc::CTLineDraw(line, ctx);
                objc::CGContextRelease(ctx);
            }
            objc::cf_release(line);
        }
        // The bitmap's first row is its top.
        let (ox, oy) = (x - 2, y - 2);
        for yy in 0..bh {
            let ty = oy + yy as i32;
            if ty < clip.top || ty >= clip.bottom {
                continue;
            }
            for xx in 0..bw {
                let tx = ox + xx as i32;
                if tx < clip.left || tx >= clip.right {
                    continue;
                }
                let a = u32::from(mask[yy * bw + xx]);
                if a != 0 {
                    blend_pixel(&mut px[ty as usize * w + tx as usize], rgb, a);
                }
            }
        }
    }

    /// The outline of one character in pixels (x right, y up from the pen
    /// origin on the baseline), for the menu bar icon.
    pub fn outline_px(&self, ch: char) -> Option<Vec<Contour>> {
        let mut units = [0u16; 2];
        let n = ch.encode_utf16(&mut units).len();
        let mut glyphs = [0u16; 2];
        // SAFETY: arrays of `n` UTF-16 units and glyphs.
        let ok = unsafe {
            objc::CTFontGetGlyphsForCharacters(
                self.font,
                units.as_ptr(),
                glyphs.as_mut_ptr(),
                n as isize,
            )
        };
        if !ok {
            return None;
        }
        // SAFETY: a valid font and glyph; the path (+1) is released below.
        let path =
            unsafe { objc::CTFontCreatePathForGlyph(self.font, glyphs[0], std::ptr::null()) };
        if path.is_null() {
            return Some(Vec::new());
        }
        let mut collector = PathCollector::default();
        // SAFETY: the callback receives `collector` back as `info` for the
        // duration of the call only.
        unsafe {
            objc::CGPathApply(path, (&raw mut collector).cast(), path_element);
            objc::CGPathRelease(path);
        }
        collector.finish();
        Some(collector.contours)
    }
}

#[derive(Default)]
struct PathCollector {
    contours: Vec<Contour>,
    current: Option<Contour>,
    pen: (f64, f64),
}

impl PathCollector {
    fn finish(&mut self) {
        if let Some(c) = self.current.take()
            && !c.segs.is_empty()
        {
            self.contours.push(c);
        }
    }
    fn seg(&mut self, s: PathSeg) {
        if self.current.is_none() {
            self.current = Some(Contour {
                start: self.pen,
                segs: Vec::new(),
            });
        }
        if let Some(c) = &mut self.current {
            c.segs.push(s);
        }
        self.pen = match s {
            PathSeg::Line(x, y) | PathSeg::Quad(_, _, x, y) => (x, y),
        };
    }
}

/// CGPathApply's callback: move, line, quad, cubic (split into quads), close.
extern "C" fn path_element(info: *mut c_void, e: *const CGPathElement) {
    // SAFETY: `info` is the PathCollector passed to CGPathApply and `e`
    // points to an element with the number of points its kind defines.
    let (c, e) = unsafe { (&mut *info.cast::<PathCollector>(), &*e) };
    let p = |i: usize| -> (f64, f64) {
        // SAFETY: as above.
        let pt: CGPoint = unsafe { *e.points.add(i) };
        (pt.x, pt.y)
    };
    match e.kind {
        0 => {
            c.finish();
            c.pen = p(0);
        }
        1 => {
            let (x, y) = p(0);
            c.seg(PathSeg::Line(x, y));
        }
        2 => {
            let ((cx, cy), (x, y)) = (p(0), p(1));
            c.seg(PathSeg::Quad(cx, cy, x, y));
        }
        3 => {
            // A cubic as four quadratic pieces (error far below a pixel at
            // icon sizes).
            let (p0, p1, p2, p3) = (c.pen, p(0), p(1), p(2));
            let at = |t: f64| {
                let u = 1.0 - t;
                (
                    u * u * u * p0.0
                        + 3.0 * u * u * t * p1.0
                        + 3.0 * u * t * t * p2.0
                        + t * t * t * p3.0,
                    u * u * u * p0.1
                        + 3.0 * u * u * t * p1.1
                        + 3.0 * u * t * t * p2.1
                        + t * t * t * p3.1,
                )
            };
            let d = |t: f64| {
                let u = 1.0 - t;
                (
                    3.0 * (u * u * (p1.0 - p0.0)
                        + 2.0 * u * t * (p2.0 - p1.0)
                        + t * t * (p3.0 - p2.0)),
                    3.0 * (u * u * (p1.1 - p0.1)
                        + 2.0 * u * t * (p2.1 - p1.1)
                        + t * t * (p3.1 - p2.1)),
                )
            };
            for k in 0..4 {
                let (t0, t1) = (k as f64 / 4.0, (k + 1) as f64 / 4.0);
                let (a, b) = (at(t0), at(t1));
                let (da, db) = (d(t0), d(t1));
                let h = (t1 - t0) / 2.0;
                // Control point: the tangents' meeting point, averaged.
                let ctrl = (
                    (a.0 + da.0 * h + b.0 - db.0 * h) / 2.0,
                    (a.1 + da.1 * h + b.1 - db.1 * h) / 2.0,
                );
                c.seg(PathSeg::Quad(ctrl.0, ctrl.1, b.0, b.1));
            }
        }
        4 => {
            if let Some(cur) = &c.current {
                let start = cur.start;
                if c.pen != start {
                    c.seg(PathSeg::Line(start.0, start.1));
                }
            }
            c.finish();
        }
        _ => {}
    }
}

/// The families the UI asks about: the system faces are always there.
pub fn families() -> Vec<String> {
    vec!["Segoe UI".into(), "Courier New".into()]
}
