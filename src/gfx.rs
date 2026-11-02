//! Software raster used for every shape the app draws: what the Python version
//! drew with Pillow, done the same way -- shapes filled without antialiasing at
//! a supersampled size, then resampled down with the same Lanczos/bicubic
//! filter and fixed-point arithmetic Pillow uses (Resample.c), and composited
//! with Pillow's and Tk's integer formulas. Pure Rust, no unsafe.

/// An 8-bit single-channel image (Pillow mode "L").
#[derive(Clone)]
pub struct Gray {
    pub w: usize,
    pub h: usize,
    pub px: Vec<u8>,
}

/// Straight (non-premultiplied) RGBA (Pillow mode "RGBA"; "RGB" is alpha 255).
#[derive(Clone)]
pub struct Rgba {
    pub w: usize,
    pub h: usize,
    pub px: Vec<[u8; 4]>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub struct Color(pub u8, pub u8, pub u8);

impl Color {
    pub fn hex(s: &str) -> Color {
        let (r, g, b) = crate::settings::hex_to_rgb(s);
        Color(r, g, b)
    }
    pub fn rgba(self, a: u8) -> [u8; 4] {
        [self.0, self.1, self.2, a]
    }
    /// COLORREF (0x00BBGGRR).
    pub fn colorref(self) -> u32 {
        u32::from(self.0) | (u32::from(self.1) << 8) | (u32::from(self.2) << 16)
    }
}

/// Lighten (factor > 1) or darken a colour, as weeknumber_widgets.shade.
pub fn shade(c: Color, factor: f64) -> Color {
    let f = |v: u8| py_round(f64::from(v) * factor).clamp(0.0, 255.0) as u8;
    Color(f(c.0), f(c.1), f(c.2))
}

/// Mix `a` toward `b` by `amount`, as weeknumber_widgets.blend.
pub fn blend(a: Color, b: Color, amount: f64) -> Color {
    let m = |x: u8, y: u8| {
        py_round(f64::from(x) + (f64::from(y) - f64::from(x)) * amount).clamp(0.0, 255.0) as u8
    };
    Color(m(a.0, b.0), m(a.1, b.1), m(a.2, b.2))
}

/// Python's round(): halves to even.
pub fn py_round(v: f64) -> f64 {
    let r = v.round();
    if (v - v.trunc()).abs() == 0.5 && r % 2.0 != 0.0 {
        r - v.signum()
    } else {
        r
    }
}

// ---------------------------------------------------------------------------
// Span rasterization (the shapes themselves are Pillow's, in pil.rs)
// ---------------------------------------------------------------------------

/// Calls `f(y, x0, x1)` for each run of pixels [x0, x1) a shape covers.
pub trait Shape {
    fn spans(&self, w: usize, h: usize, f: &mut dyn FnMut(usize, usize, usize));
}

// ---------------------------------------------------------------------------
// Images
// ---------------------------------------------------------------------------

impl Gray {
    pub fn new(w: usize, h: usize, v: u8) -> Gray {
        Gray {
            w,
            h,
            px: vec![v; w * h],
        }
    }
    pub fn fill(&mut self, shape: &dyn Shape, v: u8) {
        let w = self.w;
        let px = &mut self.px;
        shape.spans(self.w, self.h, &mut |y, a, b| {
            px[y * w + a..y * w + b].fill(v)
        });
    }
    pub fn point(&self, f: impl Fn(u8) -> u8) -> Gray {
        Gray {
            w: self.w,
            h: self.h,
            px: self.px.iter().map(|&v| f(v)).collect(),
        }
    }
    pub fn flip(&self) -> Gray {
        let mut out = Gray::new(self.w, self.h, 0);
        for y in 0..self.h {
            out.px[y * self.w..(y + 1) * self.w]
                .copy_from_slice(&self.px[(self.h - 1 - y) * self.w..(self.h - y) * self.w]);
        }
        out
    }
    pub fn mirror(&self) -> Gray {
        let mut out = self.clone();
        for row in out.px.chunks_exact_mut(self.w) {
            row.reverse();
        }
        out
    }
    pub fn lighter(&self, other: &Gray) -> Gray {
        Gray {
            w: self.w,
            h: self.h,
            px: self
                .px
                .iter()
                .zip(&other.px)
                .map(|(a, b)| *a.max(b))
                .collect(),
        }
    }
    pub fn resize(&self, w: usize, h: usize, filter: Filter) -> Gray {
        let px = resample(&self.px, 1, self.w, self.h, w, h, filter);
        Gray { w, h, px }
    }
}

impl Rgba {
    pub fn new(w: usize, h: usize, c: [u8; 4]) -> Rgba {
        Rgba {
            w,
            h,
            px: vec![c; w * h],
        }
    }
    pub fn fill(&mut self, shape: &dyn Shape, c: [u8; 4]) {
        let w = self.w;
        let px = &mut self.px;
        shape.spans(self.w, self.h, &mut |y, a, b| {
            px[y * w + a..y * w + b].fill(c)
        });
    }
    pub fn get(&self, x: usize, y: usize) -> [u8; 4] {
        self.px[y * self.w + x]
    }
    /// A solid colour behind an alpha mask (weeknumber_calendar's layer()).
    pub fn layer(mask: &Gray, c: Color) -> Rgba {
        Rgba {
            w: mask.w,
            h: mask.h,
            px: mask.px.iter().map(|&a| c.rgba(a)).collect(),
        }
    }
    pub fn putalpha(&mut self, mask: &Gray) {
        for (p, &a) in self.px.iter_mut().zip(&mask.px) {
            p[3] = a;
        }
    }
    /// Pillow's RGBA resize: through premultiplied RGBa and back.
    pub fn resize(&self, w: usize, h: usize, filter: Filter) -> Rgba {
        let mut flat = Vec::with_capacity(self.px.len() * 4);
        for p in &self.px {
            let a = u32::from(p[3]);
            for &c in &p[..3] {
                flat.push(muldiv255(u32::from(c), a));
            }
            flat.push(p[3]);
        }
        let out = resample(&flat, 4, self.w, self.h, w, h, filter);
        let px = out
            .as_chunks::<4>()
            .0
            .iter()
            .map(|q| {
                let a = q[3];
                if a == 0 || a == 255 {
                    [q[0], q[1], q[2], a]
                } else {
                    let un = |c: u8| ((255 * u32::from(c)) / u32::from(a)).min(255) as u8;
                    [un(q[0]), un(q[1]), un(q[2]), a]
                }
            })
            .collect();
        Rgba { w, h, px }
    }
    /// Resize an opaque image (Pillow mode "RGB").
    pub fn resize_rgb(&self, w: usize, h: usize, filter: Filter) -> Rgba {
        let flat: Vec<u8> = self.px.iter().flat_map(|p| [p[0], p[1], p[2]]).collect();
        let out = resample(&flat, 3, self.w, self.h, w, h, filter);
        Rgba {
            w,
            h,
            px: out
                .as_chunks::<3>()
                .0
                .iter()
                .map(|q| [q[0], q[1], q[2], 255])
                .collect(),
        }
    }
    /// Pillow's Image.reduce(2) for RGB: each 2x2 block averaged, rounded.
    pub fn reduce2(&self) -> Rgba {
        let (w, h) = (self.w / 2, self.h / 2);
        let mut px = Vec::with_capacity(w * h);
        for y in 0..h {
            for x in 0..w {
                let p = [
                    self.get(2 * x, 2 * y),
                    self.get(2 * x + 1, 2 * y),
                    self.get(2 * x, 2 * y + 1),
                    self.get(2 * x + 1, 2 * y + 1),
                ];
                let avg =
                    |c: usize| ((p.iter().map(|q| u32::from(q[c])).sum::<u32>() + 2) >> 2) as u8;
                px.push([avg(0), avg(1), avg(2), 255]);
            }
        }
        Rgba { w, h, px }
    }
    /// Pillow's Image.alpha_composite(self, src) in place.
    pub fn alpha_composite(&mut self, src: &Rgba) {
        const PB: u32 = 7;
        let shift255 = |a: u32| ((a >> 8) + a) >> 8;
        for (d, s) in self.px.iter_mut().zip(&src.px) {
            if s[3] == 0 {
                continue;
            }
            let (sa, da) = (u32::from(s[3]), u32::from(d[3]));
            let blend = da * (255 - sa);
            let outa255 = sa * 255 + blend;
            let coef1 = sa * 255 * 255 * (1 << PB) / outa255;
            let coef2 = 255 * (1 << PB) - coef1;
            for c in 0..3 {
                let tmp = u32::from(s[c]) * coef1 + u32::from(d[c]) * coef2;
                d[c] = (shift255(tmp + (0x80 << PB)) >> PB) as u8;
            }
            d[3] = shift255(outa255 + 0x80) as u8;
        }
    }
    /// Pillow's paste(src, (x, y), mask): every channel, alpha included,
    /// blended by the mask.
    pub fn paste_masked(&mut self, src: &Rgba, ox: i64, oy: i64, mask: &Gray) {
        for y in 0..src.h {
            let ty = oy + y as i64;
            if ty < 0 || ty >= self.h as i64 {
                continue;
            }
            for x in 0..src.w {
                let tx = ox + x as i64;
                if tx < 0 || tx >= self.w as i64 {
                    continue;
                }
                let m = u32::from(mask.px[y * mask.w + x]);
                let s = src.px[y * src.w + x];
                let d = &mut self.px[ty as usize * self.w + tx as usize];
                for c in 0..4 {
                    d[c] = div255(u32::from(d[c]) * (255 - m) + u32::from(s[c]) * m);
                }
            }
        }
    }
    /// The alpha channel as a mask.
    pub fn alpha(&self) -> Gray {
        Gray {
            w: self.w,
            h: self.h,
            px: self.px.iter().map(|p| p[3]).collect(),
        }
    }
    /// Image.composite(a, b, mask): `a` where the mask is set, else `b`.
    pub fn composite(a: &Rgba, b: &Rgba, mask: &Gray) -> Rgba {
        let mut out = b.clone();
        out.paste_masked(a, 0, 0, mask);
        out
    }
    /// Tk's display of a photo image with alpha over a solid background
    /// (BlendComplexAlpha): truncating integer blend.
    pub fn over_bg(&self, bg: Color) -> Rgba {
        let bgc = [bg.0, bg.1, bg.2];
        let px = self
            .px
            .iter()
            .map(|p| {
                let a = u32::from(p[3]);
                let un = 255 - a;
                let mix = |i: usize| ((u32::from(bgc[i]) * un + u32::from(p[i]) * a) / 255) as u8;
                [mix(0), mix(1), mix(2), 255]
            })
            .collect();
        Rgba {
            w: self.w,
            h: self.h,
            px,
        }
    }
}

fn muldiv255(a: u32, b: u32) -> u8 {
    let tmp = a * b + 128;
    (((tmp >> 8) + tmp) >> 8) as u8
}

fn div255(a: u32) -> u8 {
    let tmp = a + 128;
    (((tmp >> 8) + tmp) >> 8) as u8
}

// ---------------------------------------------------------------------------
// Resampling (Pillow Resample.c, 8 bits per channel)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Filter {
    Lanczos,
    Bicubic,
}

impl Filter {
    fn support(self) -> f64 {
        match self {
            Filter::Lanczos => 3.0,
            Filter::Bicubic => 2.0,
        }
    }
    fn weight(self, x: f64) -> f64 {
        match self {
            Filter::Lanczos => {
                if (-3.0..3.0).contains(&x) {
                    sinc(x) * sinc(x / 3.0)
                } else {
                    0.0
                }
            }
            Filter::Bicubic => {
                const A: f64 = -0.5;
                let x = x.abs();
                if x < 1.0 {
                    ((A + 2.0) * x - (A + 3.0)) * x * x + 1.0
                } else if x < 2.0 {
                    (((x - 5.0) * x + 8.0) * x - 4.0) * A
                } else {
                    0.0
                }
            }
        }
    }
}

fn sinc(x: f64) -> f64 {
    if x == 0.0 {
        1.0
    } else {
        let x = x * std::f64::consts::PI;
        x.sin() / x
    }
}

const PRECISION_BITS: u32 = 32 - 8 - 2;

/// Per output pixel: first input index and fixed-point weights.
fn coeffs(in_size: usize, out_size: usize, filter: Filter) -> Vec<(usize, Vec<i32>)> {
    let scale = in_size as f64 / out_size as f64;
    let filterscale = scale.max(1.0);
    let support = filter.support() * filterscale;
    let ss = 1.0 / filterscale;
    (0..out_size)
        .map(|xx| {
            let center = (xx as f64 + 0.5) * scale;
            let xmin = ((center - support + 0.5) as i64).max(0) as usize;
            let xmax = ((center + support + 0.5) as i64).min(in_size as i64) as usize;
            let raw: Vec<f64> = (xmin..xmax)
                .map(|x| filter.weight((x as f64 - center + 0.5) * ss))
                .collect();
            let total: f64 = raw.iter().sum();
            let k = raw
                .iter()
                .map(|&w| {
                    let w = if total != 0.0 { w / total } else { w };
                    let v = w * f64::from(1u32 << PRECISION_BITS);
                    if v < 0.0 {
                        (-0.5 + v) as i32
                    } else {
                        (0.5 + v) as i32
                    }
                })
                .collect();
            (xmin, k)
        })
        .collect()
}

fn clip8(v: i64) -> u8 {
    (v >> PRECISION_BITS).clamp(0, 255) as u8
}

/// Separable resample of `ch`-channel interleaved pixels, in Pillow's pass
/// order: vertical first when the height shrinks by more than twice as
/// much as the width, else horizontal first (each pass rounds to 8 bits,
/// so the order shows in the result).
pub fn resample(
    src: &[u8],
    ch: usize,
    w: usize,
    h: usize,
    nw: usize,
    nh: usize,
    filter: Filter,
) -> Vec<u8> {
    let (dy, dx) = (h as i64 - nh as i64, w as i64 - nw as i64);
    let horizontal_first = !(dy > 0 && dy > dx * 2);
    if horizontal_first {
        let tmp = if nw != w {
            resample_horizontal(src, ch, w, h, nw, filter)
        } else {
            src.to_vec()
        };
        if nh != h {
            resample_vertical(&tmp, ch, nw, h, nh, filter)
        } else {
            tmp
        }
    } else {
        let tmp = if nh != h {
            resample_vertical(src, ch, w, h, nh, filter)
        } else {
            src.to_vec()
        };
        if nw != w {
            resample_horizontal(&tmp, ch, w, nh, nw, filter)
        } else {
            tmp
        }
    }
}

fn resample_horizontal(
    src: &[u8],
    ch: usize,
    w: usize,
    h: usize,
    nw: usize,
    filter: Filter,
) -> Vec<u8> {
    let k = coeffs(w, nw, filter);
    let mut out = vec![0u8; nw * h * ch];
    for y in 0..h {
        let row = &src[y * w * ch..(y + 1) * w * ch];
        for (xx, (xmin, kk)) in k.iter().enumerate() {
            for c in 0..ch {
                let mut ss: i64 = 1 << (PRECISION_BITS - 1);
                for (i, &kv) in kk.iter().enumerate() {
                    ss += i64::from(row[(xmin + i) * ch + c]) * i64::from(kv);
                }
                out[(y * nw + xx) * ch + c] = clip8(ss);
            }
        }
    }
    out
}

fn resample_vertical(
    src: &[u8],
    ch: usize,
    w: usize,
    h: usize,
    nh: usize,
    filter: Filter,
) -> Vec<u8> {
    let k = coeffs(h, nh, filter);
    let mut out = vec![0u8; w * nh * ch];
    for (yy, (ymin, kk)) in k.iter().enumerate() {
        for x in 0..w * ch {
            let mut ss: i64 = 1 << (PRECISION_BITS - 1);
            for (i, &kv) in kk.iter().enumerate() {
                ss += i64::from(src[(ymin + i) * w * ch + x]) * i64::from(kv);
            }
            out[yy * w * ch + x] = clip8(ss);
        }
    }
    out
}

/// Image.linear_gradient("L"): 256x256, each pixel its row number.
pub fn linear_gradient() -> Gray {
    Gray {
        w: 256,
        h: 256,
        px: (0..256 * 256).map(|i| (i / 256) as u8).collect(),
    }
}

/// Image.rotate(90) of a square image: counter-clockwise.
pub fn rotate90(g: &Gray) -> Gray {
    let n = g.w;
    let mut out = Gray::new(g.h, n, 0);
    for y in 0..g.h {
        for x in 0..n {
            // (x, y) -> (y, n - 1 - x)
            out.px[(n - 1 - x) * g.h + y] = g.px[y * n + x];
        }
    }
    out
}

/// colorsys.hsv_to_rgb.
pub fn hsv_to_rgb(h: f64, s: f64, v: f64) -> (f64, f64, f64) {
    if s == 0.0 {
        return (v, v, v);
    }
    let i = (h * 6.0).floor();
    let f = h * 6.0 - i;
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - s * f), v * (1.0 - s * (1.0 - f)));
    match (i as i64).rem_euclid(6) {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    }
}

/// colorsys.rgb_to_hsv.
pub fn rgb_to_hsv(r: f64, g: f64, b: f64) -> (f64, f64, f64) {
    let maxc = r.max(g).max(b);
    let minc = r.min(g).min(b);
    let rangec = maxc - minc;
    let v = maxc;
    if minc == maxc {
        return (0.0, 0.0, v);
    }
    let s = rangec / maxc;
    let rc = (maxc - r) / rangec;
    let gc = (maxc - g) / rangec;
    let bc = (maxc - b) / rangec;
    let h = if r == maxc {
        bc - gc
    } else if g == maxc {
        2.0 + rc - bc
    } else {
        4.0 + gc - rc
    };
    ((h / 6.0).rem_euclid(1.0), s, v)
}

// ---------------------------------------------------------------------------
// Glyph outlines: FreeType's anti-aliased fill
// ---------------------------------------------------------------------------

/// One segment of a glyph contour, in pixels.
#[derive(Clone, Copy, Debug)]
pub enum PathSeg {
    Line(f64, f64),
    /// Quadratic Bezier: control point, end point.
    Quad(f64, f64, f64, f64),
}

/// A closed glyph contour: its start point and segments (x right, y up
/// from the baseline at the pen origin, as TrueType and GDI report them).
#[derive(Clone, Debug)]
pub struct Contour {
    pub start: (f64, f64),
    pub segs: Vec<PathSeg>,
}

/// Fills contours the way FreeType's smooth rasterizer does (the one
/// Pillow's text drawing uses): exact area coverage per pixel, non-zero
/// winding, coordinates on a 1/256 px grid, coverage floor(area * 256)
/// capped at 255. Each contour is placed with its origin at (ox, oy) in a
/// w x h image whose y axis points down.
pub fn fill_contours(contours: &[Contour], ox: f64, oy: f64, w: usize, h: usize) -> Gray {
    let stride = w + 2;
    let mut acc = vec![0f64; stride * h];
    let snap = |v: f64| (v * 256.0).round() / 256.0;
    let pt = |x: f64, y: f64| (snap(ox + x), snap(oy - y));
    for c in contours {
        let start = pt(c.start.0, c.start.1);
        let mut cur = start;
        for seg in &c.segs {
            match *seg {
                PathSeg::Line(x, y) => {
                    let p = pt(x, y);
                    accumulate_line(&mut acc, stride, h, cur, p);
                    cur = p;
                }
                PathSeg::Quad(cx, cy, x, y) => {
                    let (c1, p) = (pt(cx, cy), pt(x, y));
                    // Subdivide until the curve is within 1/16 px of its chord.
                    let dev =
                        ((cur.0 + p.0 - 2.0 * c1.0).abs()).max((cur.1 + p.1 - 2.0 * c1.1).abs());
                    let n = ((dev * 4.0).sqrt().ceil() as usize).clamp(1, 64);
                    let mut prev = cur;
                    for i in 1..=n {
                        let t = i as f64 / n as f64;
                        let u = 1.0 - t;
                        let q = (
                            u * u * cur.0 + 2.0 * u * t * c1.0 + t * t * p.0,
                            u * u * cur.1 + 2.0 * u * t * c1.1 + t * t * p.1,
                        );
                        accumulate_line(&mut acc, stride, h, prev, q);
                        prev = q;
                    }
                    cur = p;
                }
            }
        }
        accumulate_line(&mut acc, stride, h, cur, start);
    }
    let mut px = vec![0u8; w * h];
    for y in 0..h {
        let mut sum = 0.0;
        for x in 0..w {
            sum += acc[y * stride + x];
            px[y * w + x] = ((sum.abs() * 256.0).floor()).min(255.0) as u8;
        }
    }
    Gray { w, h, px }
}

// Derived from `draw_line` in font-rs src/raster.rs,
// Copyright 2015 Google Inc. All rights reserved.
// Licensed under the Apache License, Version 2.0
// (legal/licenses/font-rs-Apache-2.0.txt).
// Modified for WeekNumber: f64 coordinates, a padded row stride and
// clamping of x to the image width.
/// Signed-area accumulation of one edge (the font-rs scheme): each row's
/// running sum of `acc` is the winding-weighted coverage of each pixel.
fn accumulate_line(acc: &mut [f64], stride: usize, h: usize, p0: (f64, f64), p1: (f64, f64)) {
    if p0.1 == p1.1 {
        return;
    }
    let (dir, a, b) = if p0.1 < p1.1 {
        (1.0, p0, p1)
    } else {
        (-1.0, p1, p0)
    };
    let dxdy = (b.0 - a.0) / (b.1 - a.1);
    let mut x = a.0;
    let y_start = a.1.max(0.0);
    if a.1 < 0.0 {
        x -= a.1 * dxdy;
    }
    let max_x = (stride - 2) as f64;
    let mut y = y_start.floor() as usize;
    while y < h && (y as f64) < b.1 {
        let row = y * stride;
        let dy = ((y + 1) as f64).min(b.1) - (y as f64).max(a.1);
        let xnext = x + dxdy * dy;
        let d = dy * dir;
        let (x0, x1) = if x < xnext { (x, xnext) } else { (xnext, x) };
        let (x0, x1) = (x0.clamp(0.0, max_x), x1.clamp(0.0, max_x));
        let x0floor = x0.floor();
        let x0i = x0floor as usize;
        let x1ceil = x1.ceil();
        let x1i = x1ceil as usize;
        if x1i <= x0i + 1 {
            let xmf = 0.5 * (x0 + x1) - x0floor;
            acc[row + x0i] += d - d * xmf;
            acc[row + x0i + 1] += d * xmf;
        } else {
            let s = 1.0 / (x1 - x0);
            let x0f = x0 - x0floor;
            let a0 = 0.5 * s * (1.0 - x0f) * (1.0 - x0f);
            let x1f = x1 - x1ceil + 1.0;
            let am = 0.5 * s * x1f * x1f;
            acc[row + x0i] += d * a0;
            if x1i == x0i + 2 {
                acc[row + x0i + 1] += d * (1.0 - a0 - am);
            } else {
                let a1 = s * (1.5 - x0f);
                acc[row + x0i + 1] += d * (a1 - a0);
                for xi in x0i + 2..x1i - 1 {
                    acc[row + xi] += d * s;
                }
                let a2 = a1 + (x1i - x0i - 3) as f64 * s;
                acc[row + x1i - 1] += d * (1.0 - a2 - am);
            }
            acc[row + x1i] += d * am;
        }
        x = xnext;
        y += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contour_coverage() {
        // A 2x2 px square offset by half a pixel: corners a quarter, edges
        // half, the centre pixel full.
        let sq = Contour {
            start: (0.5, -0.5),
            segs: vec![
                PathSeg::Line(2.5, -0.5),
                PathSeg::Line(2.5, -2.5),
                PathSeg::Line(0.5, -2.5),
            ],
        };
        let g = fill_contours(&[sq], 0.0, 0.0, 4, 4);
        assert_eq!(&g.px[0..4], &[64, 128, 64, 0]);
        assert_eq!(&g.px[4..8], &[128, 255, 128, 0]);
        assert_eq!(&g.px[8..12], &[64, 128, 64, 0]);
        assert_eq!(&g.px[12..16], &[0, 0, 0, 0]);
        // Winding direction does not matter.
        let rev = Contour {
            start: (0.5, -0.5),
            segs: vec![
                PathSeg::Line(0.5, -2.5),
                PathSeg::Line(2.5, -2.5),
                PathSeg::Line(2.5, -0.5),
            ],
        };
        assert_eq!(fill_contours(&[rev], 0.0, 0.0, 4, 4).px, g.px);
        // A quadratic arc stays inside its hull and fills partially.
        let arc = Contour {
            start: (0.0, 0.0),
            segs: vec![PathSeg::Quad(2.0, -4.0, 4.0, 0.0)],
        };
        let a = fill_contours(&[arc], 0.0, 0.0, 4, 4);
        assert!(a.px[4 + 1] > 200 && a.px[4] < a.px[4 + 1] && a.px[12] == 0);
    }

    #[test]
    fn identity_and_flat_resample() {
        let g = Gray::new(40, 40, 200);
        let r = g.resize(10, 10, Filter::Lanczos);
        assert!(r.px.iter().all(|&v| v == 200));
        let mut m = Gray::new(100, 100, 0);
        m.fill(
            &crate::pil::RoundedRectangle(0.0, 0.0, 99.0, 99.0, 20.0),
            255,
        );
        assert_eq!(m.px[0], 0);
        assert_eq!(m.px[50 * 100 + 50], 255);
        let small = m.resize(25, 25, Filter::Lanczos);
        assert!(small.px[0] < 60 && small.px[12 * 25 + 12] == 255);
    }

    #[test]
    fn composite_math() {
        let mut d = Rgba::new(1, 1, [0, 0, 0, 0]);
        d.alpha_composite(&Rgba::new(1, 1, [255, 0, 0, 128]));
        assert_eq!(d.px[0], [255, 0, 0, 128]);
        let o = Rgba::new(1, 1, [255, 255, 255, 128]).over_bg(Color(0, 0, 0));
        assert_eq!(o.px[0][0], 128);
        assert_eq!(py_round(2.5), 2.0);
        assert_eq!(py_round(3.5), 4.0);
        let (h, s, v) = rgb_to_hsv(1.0, 0.5, 0.0);
        let (r, g, b) = hsv_to_rgb(h, s, v);
        assert!((r - 1.0).abs() < 1e-9 && (g - 0.5).abs() < 1e-9 && b.abs() < 1e-9);
    }

    #[test]
    fn rotate() {
        let g = linear_gradient();
        let r = rotate90(&g);
        assert_eq!(r.px[0], 0);
        assert_eq!(r.px[255], 255);
        assert_eq!(r.px[255 * 256], 0);
    }
}
