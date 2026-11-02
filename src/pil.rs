//! Pillow 13's ImageDraw shapes, ported from its C core (libImaging/Draw.c,
//! _imaging.c) and ImageDraw.py, so every mask the Python version draws
//! has exactly the same pixels here: coordinates truncated to integers as
//! Pillow does, its scan-line polygon filler (single-precision maths and
//! corner joining included), its integer ellipse walker with the clipping
//! tree for pie slices, and its wide-line polygon.
//!
//! Each shape reports the runs Pillow's hline/point handlers would paint;
//! filling is idempotent, so painting a run twice is harmless.

use crate::gfx::Shape;

type Span<'a> = &'a mut dyn FnMut(usize, usize, usize);

/// Pillow's hline8/hline32: inclusive [x0, x1] on row y, clipped.
fn hline(w: usize, h: usize, x0: i32, y: i32, x1: i32, f: Span) {
    if y < 0 || y >= h as i32 {
        return;
    }
    let x0 = if x0 < 0 {
        0
    } else if x0 >= w as i32 {
        return;
    } else {
        x0
    };
    let x1 = if x1 < 0 {
        return;
    } else if x1 >= w as i32 {
        w as i32 - 1
    } else {
        x1
    };
    if x0 <= x1 {
        f(y as usize, x0 as usize, x1 as usize + 1);
    }
}

fn point(w: usize, h: usize, x: i32, y: i32, f: Span) {
    if x >= 0 && x < w as i32 && y >= 0 && y < h as i32 {
        f(y as usize, x as usize, x as usize + 1);
    }
}

/// C's (int) cast of a double.
fn trunc(v: f64) -> i32 {
    v as i32
}

/// ROUND_UP / ROUND_DOWN of a float expression (single precision until the
/// floor/ceil, as C evaluates `(f) + 0.5F` for a float f).
fn round_up_f32(f: f32) -> i32 {
    if f >= 0.0 {
        f64::from(f + 0.5).floor() as i32
    } else {
        -(f64::from(f).abs() + 0.5).floor() as i32
    }
}

fn round_down_f32(f: f32) -> i32 {
    if f >= 0.0 {
        f64::from(f - 0.5).ceil() as i32
    } else {
        -(f64::from(f).abs() - 0.5).ceil() as i32
    }
}

/// The same macros on a double expression.
fn round_up_f64(f: f64) -> i32 {
    if f >= 0.0 {
        (f + 0.5).floor() as i32
    } else {
        -(f.abs() + 0.5).floor() as i32
    }
}

fn round_down_f64(f: f64) -> i32 {
    if f >= 0.0 {
        (f - 0.5).ceil() as i32
    } else {
        -(f.abs() - 0.5).ceil() as i32
    }
}

// ---------------------------------------------------------------------------
// Polygons (polygon_generic, add_edge, ImagingDrawPolygon)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct Edge {
    x0: i32,
    y0: i32,
    xmin: i32,
    ymin: i32,
    xmax: i32,
    ymax: i32,
    dx: f32,
}

fn add_edge(x0: i32, y0: i32, x1: i32, y1: i32) -> Edge {
    let (xmin, xmax) = if x0 <= x1 { (x0, x1) } else { (x1, x0) };
    let (ymin, ymax) = if y0 <= y1 { (y0, y1) } else { (y1, y0) };
    let dx = if y0 == y1 {
        0.0
    } else {
        (x1 - x0) as f32 / (y1 - y0) as f32
    };
    Edge {
        x0,
        y0,
        xmin,
        ymin,
        xmax,
        ymax,
        dx,
    }
}

impl Edge {
    fn x_at(&self, y: i32) -> f32 {
        (y - self.y0) as f32 * self.dx + self.x0 as f32
    }
}

fn polygon_generic(edges: &[Edge], w: usize, h: usize, f: Span) {
    if edges.is_empty() {
        return;
    }
    let mut ymin = h as i32 - 1;
    let mut ymax = 0;
    let mut table: Vec<Edge> = Vec::with_capacity(edges.len());
    for e in edges {
        ymin = ymin.min(e.ymin);
        ymax = ymax.max(e.ymax);
        if e.ymin == e.ymax {
            hline(w, h, e.xmin, e.ymin, e.xmax, f);
            continue;
        }
        table.push(*e);
    }
    ymin = ymin.max(0);
    ymax = ymax.min(h as i32);
    let mut xx: Vec<f32> = Vec::with_capacity(table.len() * 2);
    let mut y = ymin;
    while y <= ymax {
        xx.clear();
        for (i, current) in table.iter().enumerate() {
            if y < current.ymin || y > current.ymax {
                continue;
            }
            xx.push(current.x_at(y));
            if y == current.ymax && y < ymax {
                // Needed to draw consistent polygons
                let last = xx[xx.len() - 1];
                xx.push(last);
            } else if (y == current.ymin || y == current.ymax) && current.dx != 0.0 {
                // Connect discontiguous corners
                for other in &table[..i] {
                    if (y != other.ymin && y != other.ymax) || other.dx == 0.0 {
                        continue;
                    }
                    let last = xx.len() - 1;
                    if xx[last].round() != other.x_at(y).round() {
                        continue;
                    }
                    let offset = if y == current.ymax { -1 } else { 1 };
                    let adjacent = current.x_at(y + offset);
                    if y + offset >= other.ymin && y + offset <= other.ymax {
                        let adjacent_other = other.x_at(y + offset);
                        if xx[last] > adjacent + 1.0 && xx[last] > adjacent_other + 1.0 {
                            xx[last] = (f64::from(adjacent).max(f64::from(adjacent_other)) as f32)
                                .round()
                                + 1.0;
                        } else if xx[last] < adjacent - 1.0 && xx[last] < adjacent_other - 1.0 {
                            xx[last] = (f64::from(adjacent).min(f64::from(adjacent_other)) as f32)
                                .round()
                                - 1.0;
                        }
                        break;
                    }
                }
            }
        }
        xx.sort_by(f32::total_cmp);
        let mut i = 1;
        while i < xx.len() {
            hline(w, h, round_up_f32(xx[i - 1]), y, round_down_f32(xx[i]), f);
            i += 2;
        }
        y += 1;
    }
}

/// ImageDraw.polygon(xy, fill=...): vertices truncated to integers.
pub struct Polygon(pub Vec<(f64, f64)>);

impl Shape for Polygon {
    fn spans(&self, w: usize, h: usize, f: Span) {
        let xy: Vec<(i32, i32)> = self.0.iter().map(|&(x, y)| (trunc(x), trunc(y))).collect();
        let count = xy.len();
        if count == 0 {
            return;
        }
        let mut e: Vec<Edge> = Vec::with_capacity(count);
        let mut i = 0;
        while i + 1 < count {
            let (x0, y0) = xy[i];
            let (x1, y1) = xy[i + 1];
            if y0 == y1 && i != 0 && y0 == xy[i - 1].1 && !e.is_empty() {
                // A horizontal line right after another horizontal line.
                let prev_x = xy[i - 1].0;
                let last = e.len() - 1;
                if x1 > x0 && x0 > prev_x {
                    e[last].xmax = x1;
                    i += 1;
                    continue;
                } else if x1 < x0 && x0 < prev_x {
                    e[last].xmin = x1;
                    i += 1;
                    continue;
                }
            }
            e.push(add_edge(x0, y0, x1, y1));
            i += 1;
        }
        if xy[i] != xy[0] {
            e.push(add_edge(xy[i].0, xy[i].1, xy[0].0, xy[0].1));
        }
        polygon_generic(&e, w, h, f);
    }
}

// ---------------------------------------------------------------------------
// Lines (draw_lines, line, ImagingDrawWideLine)
// ---------------------------------------------------------------------------

fn bresenham(w: usize, h: usize, mut x0: i32, mut y0: i32, x1: i32, y1: i32, f: Span) {
    let mut dy = y1 - y0;
    let ys;
    if dy < 0 {
        dy = -dy;
        ys = -1;
    } else if dy == 0 {
        if x1 > x0 {
            hline(w, h, x0, y0, x1 - 1, f);
        } else if x0 > x1 {
            hline(w, h, x1 + 1, y0, x0, f);
        }
        return;
    } else {
        ys = 1;
    }
    let mut dx = x1 - x0;
    let xs = if dx < 0 {
        dx = -dx;
        -1
    } else {
        1
    };
    if dx == 0 {
        for _ in 0..dy {
            point(w, h, x0, y0, f);
            y0 += ys;
        }
    } else if dx > dy {
        let n = dx;
        dy += dy;
        let mut e = dy - dx;
        dx += dx;
        for _ in 0..n {
            point(w, h, x0, y0, f);
            if e >= 0 {
                y0 += ys;
                e -= dx;
            }
            e += dy;
            x0 += xs;
        }
    } else {
        let n = dy;
        dx += dx;
        let mut e = dx - dy;
        dy += dy;
        for _ in 0..n {
            point(w, h, x0, y0, f);
            if e >= 0 {
                x0 += xs;
                e -= dy;
            }
            e += dx;
            y0 += ys;
        }
    }
}

#[allow(clippy::too_many_arguments)] // the C function's parameters
fn wide_line(w: usize, h: usize, x0: i32, y0: i32, x1: i32, y1: i32, width: i32, f: Span) {
    let dx = x1 - x0;
    let dy = y1 - y0;
    if dx == 0 && dy == 0 {
        point(w, h, x0, y0, f);
        return;
    }
    let big = f64::from(dx).hypot(f64::from(dy));
    let small = f64::from(width - 1) / 2.0;
    let ratio_max = f64::from(round_up_f64(small)) / big;
    let ratio_min = f64::from(round_down_f64(small)) / big;
    let dxmin = round_down_f64(ratio_min * f64::from(dy));
    let dxmax = round_down_f64(ratio_max * f64::from(dy));
    let dymin = round_down_f64(ratio_min * f64::from(dx));
    let dymax = round_down_f64(ratio_max * f64::from(dx));
    let v = [
        (x0 - dxmin, y0 + dymax),
        (x1 - dxmin, y1 + dymax),
        (x1 + dxmax, y1 - dymin),
        (x0 + dxmax, y0 - dymin),
    ];
    let e: Vec<Edge> = (0..4)
        .map(|i| add_edge(v[i].0, v[i].1, v[(i + 1) % 4].0, v[(i + 1) % 4].1))
        .collect();
    polygon_generic(&e, w, h, f);
}

/// ImageDraw.line(xy, width=...) without joints (two points, or width <= 4,
/// which is how the Python version always calls it).
pub struct Line(pub Vec<(f64, f64)>, pub i32);

impl Shape for Line {
    fn spans(&self, w: usize, h: usize, f: Span) {
        let p: Vec<(i32, i32)> = self.0.iter().map(|&(x, y)| (trunc(x), trunc(y))).collect();
        if self.1 == 1 {
            for s in p.windows(2) {
                bresenham(w, h, s[0].0, s[0].1, s[1].0, s[1].1, f);
            }
            if let Some(&(x, y)) = p.get(1..).and_then(|r| r.last()) {
                point(w, h, x, y, f);
            }
        } else if self.1 > 1 {
            for s in p.windows(2) {
                wide_line(w, h, s[0].0, s[0].1, s[1].0, s[1].1, self.1, f);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Rectangles
// ---------------------------------------------------------------------------

/// ImageDraw.rectangle(xy, fill=...): inclusive, truncated corners.
pub struct Rectangle(pub f64, pub f64, pub f64, pub f64);

fn rectangle(w: usize, h: usize, x0: i32, y0: i32, x1: i32, y1: i32, f: Span) {
    let (mut y0, mut y1) = if y0 > y1 { (y1, y0) } else { (y0, y1) };
    if y0 < 0 {
        y0 = 0;
    } else if y0 >= h as i32 {
        return;
    }
    if y1 < 0 {
        return;
    } else if y1 > h as i32 {
        y1 = h as i32;
    }
    for y in y0..=y1 {
        hline(w, h, x0, y, x1, f);
    }
}

impl Shape for Rectangle {
    fn spans(&self, w: usize, h: usize, f: Span) {
        rectangle(
            w,
            h,
            trunc(self.0),
            trunc(self.1),
            trunc(self.2),
            trunc(self.3),
            f,
        );
    }
}

// ---------------------------------------------------------------------------
// Ellipses and pie slices (quarter_*, ellipse_*, clip tree, pie_init)
// ---------------------------------------------------------------------------

struct Quarter {
    cx: i32,
    cy: i32,
    ex: i32,
    ey: i32,
    a2: i64,
    b2: i64,
    a2b2: i64,
    finished: bool,
}

impl Quarter {
    fn new(a: i32, b: i32) -> Quarter {
        if a < 0 || b < 0 {
            return Quarter {
                cx: 0,
                cy: 0,
                ex: 0,
                ey: 0,
                a2: 0,
                b2: 0,
                a2b2: 0,
                finished: true,
            };
        }
        let (a2, b2) = (i64::from(a) * i64::from(a), i64::from(b) * i64::from(b));
        Quarter {
            cx: a,
            cy: b % 2,
            ex: a % 2,
            ey: b,
            a2,
            b2,
            a2b2: a2 * b2,
            finished: false,
        }
    }

    fn delta(&self, x: i64, y: i64) -> i64 {
        (self.a2 * y * y + self.b2 * x * x - self.a2b2).abs()
    }

    fn next(&mut self) -> Option<(i32, i32)> {
        if self.finished {
            return None;
        }
        let ret = (self.cx, self.cy);
        if self.cx == self.ex && self.cy == self.ey {
            self.finished = true;
        } else {
            let mut nx = self.cx;
            let mut ny = self.cy + 2;
            let mut nd = self.delta(i64::from(nx), i64::from(ny));
            if nx > 1 {
                let d = self.delta(i64::from(self.cx - 2), i64::from(self.cy + 2));
                if nd > d {
                    nx = self.cx - 2;
                    ny = self.cy + 2;
                    nd = d;
                }
                let d = self.delta(i64::from(self.cx - 2), i64::from(self.cy));
                if nd > d {
                    nx = self.cx - 2;
                    ny = self.cy;
                }
            }
            self.cx = nx;
            self.cy = ny;
        }
        Some(ret)
    }
}

struct EllipseWalk {
    st_o: Quarter,
    st_i: Quarter,
    py: i32,
    pl: i32,
    pr: i32,
    buf: Vec<(i32, i32, i32)>,
    finished: bool,
    leftmost: i32,
}

impl EllipseWalk {
    fn new(a: i32, b: i32, w: i32) -> EllipseWalk {
        let leftmost = a % 2;
        let mut st_o = Quarter::new(a, b);
        let mut s = EllipseWalk {
            st_o: Quarter::new(-1, -1),
            st_i: Quarter::new(-1, -1),
            py: 0,
            pl: 0,
            pr: 0,
            buf: Vec::with_capacity(4),
            finished: true,
            leftmost,
        };
        let first = if w < 1 { None } else { st_o.next() };
        if let Some((pr, py)) = first {
            s.pr = pr;
            s.py = py;
            s.finished = false;
            s.st_i = Quarter::new(a - 2 * (w - 1), b - 2 * (w - 1));
            s.pl = leftmost;
        }
        s.st_o = st_o;
        s
    }

    /// The next (x0, y, x1) run on the doubled grid.
    fn next(&mut self) -> Option<(i32, i32, i32)> {
        if self.buf.is_empty() {
            if self.finished {
                return None;
            }
            let y = self.py;
            let mut l = self.pl;
            let r = self.pr;
            let mut outer = None;
            while let Some((cx, cy)) = self.st_o.next() {
                if cy > y {
                    outer = Some((cx, cy));
                    break;
                }
            }
            match outer {
                None => self.finished = true,
                Some((cx, cy)) => {
                    self.pr = cx;
                    self.py = cy;
                }
            }
            let mut inner = None;
            while let Some((cx, cy)) = self.st_i.next() {
                if cy > y {
                    inner = Some(cx);
                    break;
                }
                l = cx;
            }
            self.pl = inner.unwrap_or(self.leftmost);
            // Pushed in this order, popped from the end.
            if (l > 0 || l < r) && y > 0 {
                self.buf.push((if l == 0 { 2 } else { l }, y, r));
            }
            if y > 0 {
                self.buf.push((-r, y, -l));
            }
            if l > 0 || l < r {
                self.buf.push((if l == 0 { 2 } else { l }, -y, r));
            }
            self.buf.push((-r, -y, -l));
        }
        self.buf.pop()
    }
}

/// A half-plane or a combination of two clipping trees.
enum Clip {
    Plane(f64, f64, f64),
    And(Box<Clip>, Box<Clip>),
    Or(Box<Clip>, Box<Clip>),
}

/// Ordered (x, type) end points of disjoint runs: type 1 opens, -1 closes.
fn do_clip(root: Option<&Clip>, x0: i32, y: i32, x1: i32) -> Vec<(i32, i8)> {
    let Some(root) = root else {
        return vec![(x0, 1), (x1, -1)];
    };
    match root {
        Clip::Plane(a, b, c) => {
            let (a, b, c) = (*a, *b, *c);
            let eps = 1e-9;
            let (mut x0, mut x1) = (x0, x1);
            let yf = f64::from(y);
            if a.abs() < eps {
                if b * yf + c < -eps {
                    x0 = 1;
                    x1 = 0;
                }
            } else {
                let ix = -(b * yf + c) / a;
                if a * f64::from(x0) + b * yf + c < eps {
                    x0 = f64::from(x0).max(ix).round() as i32;
                }
                if a * f64::from(x1) + b * yf + c < eps {
                    x1 = f64::from(x1).min(ix).round() as i32;
                }
            }
            if x0 <= x1 {
                vec![(x0, 1), (x1, -1)]
            } else {
                Vec::new()
            }
        }
        Clip::And(l, r) | Clip::Or(l, r) => {
            let is_or = matches!(root, Clip::Or(..));
            let l1 = do_clip(Some(l), x0, y, x1);
            let l2 = do_clip(Some(r), x0, y, x1);
            let (mut i1, mut i2) = (0, 0);
            let (mut k1, mut k2) = (0i32, 0i32);
            let mut out: Vec<(i32, i8)> = Vec::new();
            while i1 < l1.len() || i2 < l2.len() {
                let take_first = i2 >= l2.len()
                    || (i1 < l1.len()
                        && (l1[i1].0 < l2[i2].0 || (l1[i1].0 == l2[i2].0 && l1[i1].1 > l2[i2].1)));
                let t = if take_first {
                    let t = l1[i1];
                    k1 += i32::from(t.1);
                    i1 += 1;
                    t
                } else {
                    let t = l2[i2];
                    k2 += i32::from(t.1);
                    i2 += 1;
                    t
                };
                let tail = out.last().map(|e| e.1);
                let keep = if is_or {
                    (t.1 == 1 && tail.is_none_or(|ty| ty == -1))
                        || (t.1 == -1 && k1 == 0 && k2 == 0)
                } else {
                    (t.1 == 1 && tail.is_none_or(|ty| ty == -1) && k1 > 0 && k2 > 0)
                        || (t.1 == -1 && tail == Some(1) && (k1 == 0 || k2 == 0))
                };
                if keep {
                    out.push(t);
                }
            }
            out
        }
    }
}

/// normalize_angles: 0 <= al < 360, al <= ar <= al + 360 (in floats).
fn normalize_angles(al: f32, ar: f32) -> (f32, f32) {
    if ar - al >= 360.0 {
        return (0.0, 360.0);
    }
    let (alf, arf) = (f64::from(al), f64::from(ar));
    let al_n = ((if alf < 0.0 {
        360.0 - (-alf % 360.0)
    } else {
        alf
    }) % 360.0) as f32;
    let aln = f64::from(al_n);
    let ar_n = (aln
        + (if arf < aln {
            360.0 - (aln - arf) % 360.0
        } else {
            arf - aln
        }) % 360.0) as f32;
    (al_n, ar_n)
}

fn pie_clip(a: i32, b: i32, al: f32, ar: f32) -> Clip {
    let (af, bf) = (f64::from(a), f64::from(b));
    let rad = |v: f32| f64::from(v) * std::f64::consts::PI / 180.0;
    let (xl, xr) = (af * rad(al).cos(), af * rad(ar).cos());
    let (yl, yr) = (bf * rad(al).sin(), bf * rad(ar).sin());
    let lc = Box::new(Clip::Plane(-yl, xl, 0.0));
    let rc = Box::new(Clip::Plane(yr, -xr, 0.0));
    let root = if ar - al < 180.0 {
        Clip::And(lc, rc)
    } else {
        Clip::Or(lc, rc)
    };
    if ar - al < 90.0 {
        Clip::And(
            Box::new(root),
            Box::new(Clip::Plane((xl + xr) / 2.0, (yl + yr) / 2.0, 0.0)),
        )
    } else {
        root
    }
}

impl Clip {
    /// clip_tree_transpose: mirror every half-plane across y = x.
    fn transposed(self) -> Clip {
        match self {
            Clip::Plane(a, b, c) => Clip::Plane(b, a, c),
            Clip::And(l, r) => Clip::And(Box::new(l.transposed()), Box::new(r.transposed())),
            Clip::Or(l, r) => Clip::Or(Box::new(l.transposed()), Box::new(r.transposed())),
        }
    }
}

/// arc_init's clipping tree: an arc with caps orthogonal to the curve.
/// None when the arc is the whole ellipse.
fn arc_clip(a: i32, b: i32, al: f32, ar: f32) -> Option<Clip> {
    if a < b {
        return arc_clip(b, a, 90.0 - ar, 90.0 - al).map(Clip::transposed);
    }
    let (al, ar) = normalize_angles(al, ar);
    if ar == al + 360.0 {
        return None;
    }
    let (af, bf) = (f64::from(a), f64::from(b));
    let (alf, arf) = (f64::from(al), f64::from(ar));
    let pi = std::f64::consts::PI;
    let lc = Clip::Plane(
        -af * (alf * pi / 180.0).sin(),
        bf * (alf * pi / 180.0).cos(),
        f64::from(a * a - b * b) * (alf * pi / 90.0).sin() / 2.0,
    );
    let rc = Clip::Plane(
        af * (arf * pi / 180.0).sin(),
        -bf * (arf * pi / 180.0).cos(),
        f64::from(b * b - a * a) * (arf * pi / 90.0).sin() / 2.0,
    );
    let pair = |l: Clip, r: Clip| {
        if ar - al < 180.0 {
            Clip::And(Box::new(l), Box::new(r))
        } else {
            Clip::Or(Box::new(l), Box::new(r))
        }
    };
    if alf % 180.0 == 0.0 || arf % 180.0 == 0.0 {
        Some(pair(lc, rc))
    } else if ((alf / 180.0) as i32 + (arf / 180.0) as i32) % 2 == 1 {
        let side = |v: f64| {
            Clip::Plane(
                0.0,
                if (v / 180.0) as i32 % 2 == 0 {
                    1.0
                } else {
                    -1.0
                },
                0.0,
            )
        };
        Some(Clip::Or(
            Box::new(Clip::And(Box::new(side(alf)), Box::new(lc))),
            Box::new(Clip::And(Box::new(side(arf)), Box::new(rc))),
        ))
    } else {
        let half = Clip::Plane(0.0, if ar < 180.0 || ar > 540.0 { 1.0 } else { -1.0 }, 0.0);
        Some(pair(pair(lc, rc), half))
    }
}

/// ImagingDrawArc on integer corners.
#[allow(clippy::too_many_arguments)] // the C function's parameters
fn arc(
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
    start: f32,
    end: f32,
    width: i32,
    w: usize,
    h: usize,
    f: Span,
) {
    let (start, end) = normalize_angles(start, end);
    if start + 360.0 == end {
        ellipse_runs(x0, y0, x1, y1, width, None, w, h, f);
        return;
    }
    if start == end {
        return;
    }
    let clip = arc_clip(x1 - x0, y1 - y0, start, end);
    ellipse_runs(x0, y0, x1, y1, width, clip.as_ref(), w, h, f);
}

/// The runs of an ellipse (optionally clipped) in the box (x0, y0)-(x1, y1).
#[allow(clippy::too_many_arguments)] // the C function's parameters
fn ellipse_runs(
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
    width: i32,
    clip: Option<&Clip>,
    w: usize,
    h: usize,
    f: Span,
) {
    let a = x1 - x0;
    let b = y1 - y0;
    if a < 0 || b < 0 {
        return;
    }
    let mut walk = EllipseWalk::new(a, b, width);
    let mut pending: Vec<(i32, i8)> = Vec::new();
    let mut pending_y = 0;
    loop {
        if pending.is_empty() {
            let Some((sx0, sy, sx1)) = walk.next() else {
                return;
            };
            pending = do_clip(clip, sx0, sy, sx1);
            pending.reverse();
            pending_y = sy;
            continue;
        }
        let (Some(start), Some(end)) = (pending.pop(), pending.pop()) else {
            return;
        };
        hline(
            w,
            h,
            x0 + (start.0 + a) / 2,
            y0 + (pending_y + b) / 2,
            x0 + (end.0 + a) / 2,
            f,
        );
    }
}

/// ImageDraw.ellipse(xy, fill=...).
pub struct Ellipse(pub f64, pub f64, pub f64, pub f64);

impl Shape for Ellipse {
    fn spans(&self, w: usize, h: usize, f: Span) {
        let (x0, y0, x1, y1) = (trunc(self.0), trunc(self.1), trunc(self.2), trunc(self.3));
        ellipse_runs(x0, y0, x1, y1, (x1 - x0) + (y1 - y0), None, w, h, f);
    }
}

/// ImageDraw.ellipse(xy, outline=..., width=...): the ring only.
pub struct EllipseOutline(pub f64, pub f64, pub f64, pub f64, pub i32);

impl Shape for EllipseOutline {
    fn spans(&self, w: usize, h: usize, f: Span) {
        let (x0, y0, x1, y1) = (trunc(self.0), trunc(self.1), trunc(self.2), trunc(self.3));
        ellipse_runs(x0, y0, x1, y1, self.4, None, w, h, f);
    }
}

/// ImageDraw.pieslice(xy, start, end, fill=...).
pub struct Pieslice(pub f64, pub f64, pub f64, pub f64, pub f32, pub f32);

impl Shape for Pieslice {
    fn spans(&self, w: usize, h: usize, f: Span) {
        let (x0, y0, x1, y1) = (trunc(self.0), trunc(self.1), trunc(self.2), trunc(self.3));
        let (al, ar) = normalize_angles(self.4, self.5);
        let width = x1 + y1 - x0 - y0;
        if al + 360.0 == ar {
            ellipse_runs(x0, y0, x1, y1, width, None, w, h, f);
            return;
        }
        if al == ar {
            return;
        }
        let clip = pie_clip(x1 - x0, y1 - y0, al, ar);
        ellipse_runs(x0, y0, x1, y1, width, Some(&clip), w, h, f);
    }
}

/// Python's round(): halves to even.
fn py_round(v: f64) -> f64 {
    crate::gfx::py_round(v)
}

/// ImageDraw.rounded_rectangle(xy, radius, fill=...) with all corners.
pub struct RoundedRectangle(pub f64, pub f64, pub f64, pub f64, pub f64);

impl Shape for RoundedRectangle {
    fn spans(&self, w: usize, h: usize, f: Span) {
        let RoundedRectangle(fx0, fy0, fx1, fy1, radius) = *self;
        let mut d = (fx1 - fx0).min(fy1 - fy0).min(radius * 2.0);
        let (x0, y0, x1, y1) = (py_round(fx0), py_round(fy0), py_round(fx1), py_round(fy1));
        let full_x = d >= x1 - x0 - 1.0;
        if full_x {
            d = x1 - x0;
        }
        let full_y = d >= y1 - y0 - 1.0;
        if full_y {
            d = y1 - y0;
        }
        if full_x && full_y {
            return Ellipse(fx0, fy0, fx1, fy1).spans(w, h, f);
        }
        if d == 0.0 {
            return Rectangle(fx0, fy0, fx1, fy1).spans(w, h, f);
        }
        let r = (d / 2.0).floor();
        let parts: Vec<(f64, f64, f64, f64, f32, f32)> = if full_x {
            vec![
                (x0, y0, x0 + d, y0 + d, 180.0, 360.0),
                (x0, y1 - d, x0 + d, y1, 0.0, 180.0),
            ]
        } else if full_y {
            vec![
                (x0, y0, x0 + d, y0 + d, 90.0, 270.0),
                (x1 - d, y0, x1, y0 + d, 270.0, 90.0),
            ]
        } else {
            vec![
                (x0, y0, x0 + d, y0 + d, 180.0, 270.0),
                (x1 - d, y0, x1, y0 + d, 270.0, 360.0),
                (x1 - d, y1 - d, x1, y1, 0.0, 90.0),
                (x0, y1 - d, x0 + d, y1, 90.0, 180.0),
            ]
        };
        for (a, b, c, e, s, t) in parts {
            Pieslice(a, b, c, e, s, t).spans(w, h, f);
        }
        if full_x {
            Rectangle(x0, y0 + r + 1.0, x1, y1 - r - 1.0).spans(w, h, f);
        } else if x1 - r - 1.0 >= x0 + r + 1.0 {
            Rectangle(x0 + r + 1.0, y0, x1 - r - 1.0, y1).spans(w, h, f);
        }
        if !full_x && !full_y {
            Rectangle(x0, y0 + r + 1.0, x0 + r, y1 - r - 1.0).spans(w, h, f);
            Rectangle(x1 - r, y0 + r + 1.0, x1, y1 - r - 1.0).spans(w, h, f);
        }
    }
}

/// ImageDraw.rounded_rectangle(xy, radius, outline=..., width=...) with
/// all corners: the outline only.
pub struct RoundedRectangleOutline(pub f64, pub f64, pub f64, pub f64, pub f64, pub i32);

impl Shape for RoundedRectangleOutline {
    fn spans(&self, w: usize, h: usize, f: Span) {
        let RoundedRectangleOutline(fx0, fy0, fx1, fy1, radius, width) = *self;
        if width == 0 {
            return;
        }
        let mut d = (fx1 - fx0).min(fy1 - fy0).min(radius * 2.0);
        let (x0, y0, x1, y1) = (py_round(fx0), py_round(fy0), py_round(fx1), py_round(fy1));
        let full_x = d >= x1 - x0 - 1.0;
        if full_x {
            d = x1 - x0;
        }
        let full_y = d >= y1 - y0 - 1.0;
        if full_y {
            d = y1 - y0;
        }
        if full_x && full_y {
            return EllipseOutline(fx0, fy0, fx1, fy1, width).spans(w, h, f);
        }
        if d == 0.0 {
            // ImageDraw.rectangle(outline=..., width=...)
            let (rx0, ry0, rx1, ry1) = (trunc(fx0), trunc(fy0), trunc(fx1), trunc(fy1));
            for i in 0..width {
                hline(w, h, rx0, ry0 + i, rx1, f);
                hline(w, h, rx0, ry1 - i, rx1, f);
                bresenham(w, h, rx1 - i, ry0 + width, rx1 - i, ry1 - width + 1, f);
                bresenham(w, h, rx0 + i, ry0 + width, rx0 + i, ry1 - width + 1, f);
            }
            return;
        }
        let r = (d / 2.0).floor();
        let parts: Vec<(f64, f64, f64, f64, f32, f32)> = if full_x {
            vec![
                (x0, y0, x0 + d, y0 + d, 180.0, 360.0),
                (x0, y1 - d, x0 + d, y1, 0.0, 180.0),
            ]
        } else if full_y {
            vec![
                (x0, y0, x0 + d, y0 + d, 90.0, 270.0),
                (x1 - d, y0, x1, y0 + d, 270.0, 90.0),
            ]
        } else {
            vec![
                (x0, y0, x0 + d, y0 + d, 180.0, 270.0),
                (x1 - d, y0, x1, y0 + d, 270.0, 360.0),
                (x1 - d, y1 - d, x1, y1, 0.0, 90.0),
                (x0, y1 - d, x0 + d, y1, 90.0, 180.0),
            ]
        };
        for (a, b, c, e, s, t) in parts {
            arc(trunc(a), trunc(b), trunc(c), trunc(e), s, t, width, w, h, f);
        }
        let wf = f64::from(width);
        if !full_x {
            Rectangle(x0 + r + 1.0, y0, x1 - r - 1.0, y0 + wf - 1.0).spans(w, h, f);
            Rectangle(x0 + r + 1.0, y1 - wf + 1.0, x1 - r - 1.0, y1).spans(w, h, f);
        }
        if !full_y {
            Rectangle(x0, y0 + r + 1.0, x0 + wf - 1.0, y1 - r - 1.0).spans(w, h, f);
            Rectangle(x1 - wf + 1.0, y0 + r + 1.0, x1, y1 - r - 1.0).spans(w, h, f);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gfx::Gray;

    fn mask(shape: &dyn Shape, w: usize, h: usize) -> String {
        let mut g = Gray::new(w, h, 0);
        g.fill(shape, 255);
        g.px.chunks(w)
            .map(|r| {
                r.iter()
                    .map(|&v| if v != 0 { '#' } else { '.' })
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn small_ellipse_matches_pillow() {
        // Image.new("L", (6, 6)); ImageDraw.Draw(im).ellipse((0, 0, 5, 5), fill=255)
        assert_eq!(
            mask(&Ellipse(0.0, 0.0, 5.0, 5.0), 6, 6),
            "..##..\n.####.\n######\n######\n.####.\n..##.."
        );
    }
}
