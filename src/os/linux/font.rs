//! Text on Linux: the system's TrueType fonts, read and drawn here. A font
//! file is parsed for its character map, metrics and glyph outlines
//! (simple and composite glyphs); outlines are filled with the same exact
//! area-coverage rasteriser the tray icon uses (gfx::fill_contours).
//!
//! Families are found by file name in the standard font folders; the UI's
//! Windows family names map to the desktop's usual sans-serif and
//! monospace faces. Font files are untrusted input: every table offset and
//! length is checked, and malformed files are skipped.

use crate::gfx::{self, Contour, Gray, PathSeg};
use crate::os::{FontMetrics, Quality, Rect};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// Largest font file read (Noto CJK is ~20 MB; the faces used are < 1 MB).
const MAX_FONT_FILE: u64 = 32 << 20;
/// Composite glyphs nest at most this deep.
const MAX_COMPOSITE_DEPTH: u32 = 8;

/// (family, regular file names, bold file names), best first. File names
/// are matched case-insensitively anywhere under the font folders.
const SANS: &[(&str, &[&str], &[&str])] = &[
    (
        "Noto Sans",
        &["notosans-regular.ttf"],
        &["notosans-bold.ttf"],
    ),
    ("DejaVu Sans", &["dejavusans.ttf"], &["dejavusans-bold.ttf"]),
    (
        "Liberation Sans",
        &["liberationsans-regular.ttf"],
        &["liberationsans-bold.ttf"],
    ),
    (
        "Ubuntu",
        &["ubuntu-r.ttf", "ubuntu-regular.ttf"],
        &["ubuntu-b.ttf", "ubuntu-bold.ttf"],
    ),
    (
        "Open Sans",
        &["opensans-regular.ttf"],
        &["opensans-bold.ttf"],
    ),
    ("FreeSans", &["freesans.ttf"], &["freesansbold.ttf"]),
    ("Carlito", &["carlito-regular.ttf"], &["carlito-bold.ttf"]),
    ("Arial", &["arial.ttf"], &["arialbd.ttf"]),
];

const MONO: &[(&str, &[&str], &[&str])] = &[
    (
        "Noto Sans Mono",
        &["notosansmono-regular.ttf"],
        &["notosansmono-bold.ttf"],
    ),
    (
        "DejaVu Sans Mono",
        &["dejavusansmono.ttf"],
        &["dejavusansmono-bold.ttf"],
    ),
    (
        "Liberation Mono",
        &["liberationmono-regular.ttf"],
        &["liberationmono-bold.ttf"],
    ),
    ("Ubuntu Mono", &["ubuntumono-r.ttf"], &["ubuntumono-b.ttf"]),
    ("FreeMono", &["freemono.ttf"], &["freemonobold.ttf"]),
];

fn font_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        dirs.push(home.join(".local/share/fonts"));
        dirs.push(home.join(".fonts"));
    }
    let data = std::env::var("XDG_DATA_DIRS")
        .unwrap_or_else(|_| "/usr/local/share:/usr/share".to_string());
    for d in data.split(':').filter(|d| d.starts_with('/')) {
        dirs.push(Path::new(d).join("fonts"));
    }
    for d in [
        "/usr/share/fonts",
        "/usr/local/share/fonts",
        "/run/current-system/sw/share/X11/fonts",
        // Inside Flatpak: the host's fonts, as the sandbox exposes them.
        "/run/host/fonts",
        "/run/host/local-fonts",
        "/run/host/user-fonts",
    ] {
        dirs.push(PathBuf::from(d));
    }
    dirs.dedup();
    dirs
}

thread_local! {
    static FILES: RefCell<Option<Rc<HashMap<String, PathBuf>>>> = const { RefCell::new(None) };
}

/// Every .ttf/.ttc file name (lower case) under the font folders, kept
/// while windows are open.
fn font_files() -> Rc<HashMap<String, PathBuf>> {
    FILES.with(|f| {
        f.borrow_mut()
            .get_or_insert_with(|| {
                let mut map = HashMap::new();
                let mut budget = 50_000usize;
                for d in font_dirs() {
                    walk(&d, 0, &mut budget, &mut map);
                }
                Rc::new(map)
            })
            .clone()
    })
}

fn walk(dir: &Path, depth: u32, budget: &mut usize, out: &mut HashMap<String, PathBuf>) {
    if depth > 8 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        if *budget == 0 {
            return;
        }
        *budget -= 1;
        let Ok(kind) = e.file_type() else { continue };
        let path = e.path();
        if kind.is_dir() {
            walk(&path, depth + 1, budget, out);
        } else if kind.is_file() || kind.is_symlink() {
            let name = e.file_name().to_string_lossy().to_lowercase();
            if name.ends_with(".ttf") || name.ends_with(".ttc") {
                out.entry(name).or_insert(path);
            }
        }
    }
}

fn is_mono_name(family: &str) -> bool {
    let f = family.to_lowercase();
    ["mono", "courier", "consolas", "cascadia", "menlo", "code"]
        .iter()
        .any(|k| f.contains(k))
}

/// The families this system can draw, in the UI's preference order.
pub fn families() -> Vec<String> {
    let files = font_files();
    SANS.iter()
        .chain(MONO)
        .filter(|(_, regular, _)| regular.iter().any(|n| files.contains_key(*n)))
        .map(|(f, _, _)| f.to_string())
        .collect()
}

thread_local! {
    static FACES: RefCell<HashMap<PathBuf, Option<Rc<Face>>>> = RefCell::new(HashMap::new());
}

/// Every window has closed: give the font files and the folder listing
/// back (faces still in use stay alive through their fonts).
pub fn release_caches() {
    FACES.with(|f| f.borrow_mut().clear());
    FILES.with(|f| *f.borrow_mut() = None);
    HANDLES.with(|h| h.borrow_mut().clear());
}

/// The parsed face for `family` (or the best fallback) and whether bold
/// has to be synthesised.
fn resolve(family: &str, bold: bool) -> Option<(Rc<Face>, bool)> {
    let files = font_files();
    let list = if is_mono_name(family) { MONO } else { SANS };
    let named = list
        .iter()
        .chain(SANS)
        .chain(MONO)
        .filter(|(f, _, _)| f.eq_ignore_ascii_case(family));
    let ordered: Vec<&(&str, &[&str], &[&str])> = named.chain(list.iter()).chain(SANS).collect();
    let load = |p: &PathBuf| {
        FACES.with(|faces| {
            faces
                .borrow_mut()
                .entry(p.clone())
                .or_insert_with(|| Face::load(p).map(Rc::new))
                .clone()
        })
    };
    for (_, regular, bolds) in &ordered {
        if bold {
            for n in bolds.iter() {
                if let Some(face) = files.get(*n).and_then(load) {
                    return Some((face, false));
                }
            }
        }
        for n in regular.iter() {
            if let Some(face) = files.get(*n).and_then(load) {
                return Some((face, bold));
            }
        }
    }
    // Anything at all, so the app never shows blank labels.
    let mut names: Vec<&String> = files.keys().collect();
    names.sort();
    names
        .into_iter()
        .filter(|n| n.contains("sans") || n.contains("dejavu"))
        .find_map(|n| load(&files[n]))
        .map(|f| (f, bold))
}

// ---------------------------------------------------------------------------
// TrueType parsing
// ---------------------------------------------------------------------------

pub struct Face {
    data: Vec<u8>,
    glyf: (usize, usize),
    loca: (usize, usize),
    hmtx: (usize, usize),
    cmap: (usize, usize, u16),
    long_loca: bool,
    glyphs: u16,
    hmetrics: u16,
    upem: f64,
    ascent: f64,
    descent: f64,
}

fn u16_at(b: &[u8], i: usize) -> Option<u16> {
    Some(u16::from_be_bytes(b.get(i..i + 2)?.try_into().ok()?))
}
fn i16_at(b: &[u8], i: usize) -> Option<i16> {
    Some(i16::from_be_bytes(b.get(i..i + 2)?.try_into().ok()?))
}
fn u32_at(b: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(i..i + 4)?.try_into().ok()?))
}

impl Face {
    pub fn load(path: &Path) -> Option<Face> {
        use std::io::Read;
        let f = std::fs::File::open(path).ok()?;
        if f.metadata().ok()?.len() > MAX_FONT_FILE {
            return None;
        }
        let mut data = Vec::new();
        f.take(MAX_FONT_FILE).read_to_end(&mut data).ok()?;
        Face::parse(data)
    }

    pub fn parse(data: Vec<u8>) -> Option<Face> {
        // A collection: its first face.
        let base = if data.get(..4)? == b"ttcf" {
            u32_at(&data, 12)? as usize
        } else {
            0
        };
        let tag = u32_at(&data, base)?;
        if tag != 0x0001_0000 && tag != u32::from_be_bytes(*b"true") {
            return None; // CFF outlines are not supported
        }
        let n = usize::from(u16_at(&data, base + 4)?);
        let mut tables: HashMap<[u8; 4], (usize, usize)> = HashMap::new();
        for i in 0..n.min(64) {
            let r = base + 12 + 16 * i;
            let t: [u8; 4] = data.get(r..r + 4)?.try_into().ok()?;
            let off = u32_at(&data, r + 8)? as usize;
            let len = u32_at(&data, r + 12)? as usize;
            if off.checked_add(len)? > data.len() {
                return None;
            }
            tables.insert(t, (off, len));
        }
        let t = |name: &[u8; 4]| tables.get(name).copied();
        let head = t(b"head")?;
        let hhea = t(b"hhea")?;
        let maxp = t(b"maxp")?;
        let upem = f64::from(u16_at(&data, head.0 + 18)?);
        if !(16.0..=16384.0).contains(&upem) {
            return None;
        }
        let long_loca = i16_at(&data, head.0 + 50)? == 1;
        let glyphs = u16_at(&data, maxp.0 + 4)?;
        let hmetrics = u16_at(&data, hhea.0 + 34)?;
        let (mut ascent, mut descent) = (
            f64::from(i16_at(&data, hhea.0 + 4)?),
            -f64::from(i16_at(&data, hhea.0 + 6)?),
        );
        // GDI's line box: the OS/2 table's Windows ascent and descent.
        if let Some(os2) = t(b"OS/2")
            && os2.1 >= 78
        {
            ascent = f64::from(u16_at(&data, os2.0 + 74)?);
            descent = f64::from(u16_at(&data, os2.0 + 76)?);
        }
        let cmap = t(b"cmap")?;
        let sub = Face::pick_cmap(&data, cmap.0)?;
        let face = Face {
            glyf: t(b"glyf")?,
            loca: t(b"loca")?,
            hmtx: t(b"hmtx")?,
            cmap: sub,
            long_loca,
            glyphs,
            hmetrics: hmetrics.max(1),
            upem,
            ascent,
            descent,
            data,
        };
        // Must at least map 'A' to a glyph.
        (face.glyph_index('A') != 0).then_some(face)
    }

    /// The best Unicode subtable: (offset, length, format).
    fn pick_cmap(data: &[u8], at: usize) -> Option<(usize, usize, u16)> {
        let n = usize::from(u16_at(data, at + 2)?);
        let mut best: Option<(u8, usize, usize, u16)> = None;
        for i in 0..n.min(64) {
            let r = at + 4 + 8 * i;
            let (pid, eid) = (u16_at(data, r)?, u16_at(data, r + 2)?);
            let off = at + u32_at(data, r + 4)? as usize;
            let format = u16_at(data, off)?;
            let rank = match (pid, eid, format) {
                (3, 10, 12) | (0, 4, 12) | (0, 6, 12) => 3,
                (3, 1, 4) | (0, 3, 4) | (0, 1, 4) | (0, 0, 4) => 2,
                _ => continue,
            };
            let len = if format == 12 {
                u32_at(data, off + 4)? as usize
            } else {
                usize::from(u16_at(data, off + 2)?)
            };
            if off.checked_add(len)? > data.len() {
                continue;
            }
            if best.is_none_or(|b| rank > b.0) {
                best = Some((rank, off, len, format));
            }
        }
        best.map(|(_, o, l, f)| (o, l, f))
    }

    pub fn glyph_index(&self, ch: char) -> u16 {
        self.lookup(u32::from(ch))
            .filter(|&g| g < self.glyphs)
            .unwrap_or(0)
    }

    fn lookup(&self, c: u32) -> Option<u16> {
        let (off, _, format) = self.cmap;
        let d = &self.data;
        if format == 12 {
            let groups = u32_at(d, off + 12)? as usize;
            let (mut lo, mut hi) = (0usize, groups.min(1 << 20));
            while lo < hi {
                let mid = (lo + hi) / 2;
                let g = off + 16 + 12 * mid;
                let (start, end) = (u32_at(d, g)?, u32_at(d, g + 4)?);
                if c < start {
                    hi = mid;
                } else if c > end {
                    lo = mid + 1;
                } else {
                    return u16::try_from(u32_at(d, g + 8)? + (c - start)).ok();
                }
            }
            return None;
        }
        if c > 0xFFFF {
            return None;
        }
        let c = c as u16;
        let segs = usize::from(u16_at(d, off + 6)? / 2);
        let ends = off + 14;
        let starts = ends + 2 * segs + 2;
        let deltas = starts + 2 * segs;
        let ranges = deltas + 2 * segs;
        for i in 0..segs {
            if u16_at(d, ends + 2 * i)? < c {
                continue;
            }
            let start = u16_at(d, starts + 2 * i)?;
            if start > c {
                return None;
            }
            let delta = u16_at(d, deltas + 2 * i)?;
            let ro = u16_at(d, ranges + 2 * i)?;
            if ro == 0 {
                return Some(c.wrapping_add(delta));
            }
            let at = ranges + 2 * i + usize::from(ro) + 2 * usize::from(c - start);
            let g = u16_at(d, at)?;
            return (g != 0).then(|| g.wrapping_add(delta));
        }
        None
    }

    /// Advance width in font units.
    pub fn advance(&self, g: u16) -> f64 {
        let i = usize::from(g.min(self.hmetrics - 1));
        f64::from(u16_at(&self.data, self.hmtx.0 + 4 * i).unwrap_or(0))
    }

    fn glyph_range(&self, g: u16) -> Option<(usize, usize)> {
        if g >= self.glyphs {
            return None;
        }
        let i = usize::from(g);
        let (a, b) = if self.long_loca {
            (
                u32_at(&self.data, self.loca.0 + 4 * i)? as usize,
                u32_at(&self.data, self.loca.0 + 4 * i + 4)? as usize,
            )
        } else {
            (
                2 * usize::from(u16_at(&self.data, self.loca.0 + 2 * i)?),
                2 * usize::from(u16_at(&self.data, self.loca.0 + 2 * i + 2)?),
            )
        };
        if a > b || b > self.glyf.1 {
            return None;
        }
        Some((self.glyf.0 + a, self.glyf.0 + b))
    }

    /// The glyph's contours in font units (x right, y up), as on-curve and
    /// off-curve points; empty for a blank glyph.
    pub fn outline(&self, g: u16) -> Vec<Contour> {
        let mut out = Vec::new();
        self.outline_into(g, [1.0, 0.0, 0.0, 1.0, 0.0, 0.0], 0, &mut out);
        out
    }

    fn outline_into(&self, g: u16, m: [f64; 6], depth: u32, out: &mut Vec<Contour>) -> Option<()> {
        let (start, end) = self.glyph_range(g)?;
        if start == end {
            return Some(());
        }
        let d = &self.data;
        let n = i16_at(d, start)?;
        if n >= 0 {
            self.simple(start, n as usize, end, m, out)
        } else {
            if depth >= MAX_COMPOSITE_DEPTH {
                return None;
            }
            let mut at = start + 10;
            loop {
                let flags = u16_at(d, at)?;
                let child = u16_at(d, at + 2)?;
                at += 4;
                let (dx, dy) = if flags & 1 != 0 {
                    let v = (i16_at(d, at)?, i16_at(d, at + 2)?);
                    at += 4;
                    (f64::from(v.0), f64::from(v.1))
                } else {
                    let v = (*d.get(at)? as i8, *d.get(at + 1)? as i8);
                    at += 2;
                    (f64::from(v.0), f64::from(v.1))
                };
                let f2 = |i: usize| i16_at(d, i).map(|v| f64::from(v) / 16384.0);
                let (mut a, mut b, mut c, mut e) = (1.0, 0.0, 0.0, 1.0);
                if flags & 0x08 != 0 {
                    a = f2(at)?;
                    e = a;
                    at += 2;
                } else if flags & 0x40 != 0 {
                    a = f2(at)?;
                    e = f2(at + 2)?;
                    at += 4;
                } else if flags & 0x80 != 0 {
                    a = f2(at)?;
                    b = f2(at + 2)?;
                    c = f2(at + 4)?;
                    e = f2(at + 6)?;
                    at += 8;
                }
                // Point-matching (ARGS_ARE_XY_VALUES off) is not used by
                // the faces looked up here; such components sit unshifted.
                let (dx, dy) = if flags & 2 != 0 { (dx, dy) } else { (0.0, 0.0) };
                // child = (a b; c e) * p + (dx, dy), then the parent's m.
                let cm = [
                    m[0] * a + m[2] * b,
                    m[1] * a + m[3] * b,
                    m[0] * c + m[2] * e,
                    m[1] * c + m[3] * e,
                    m[0] * dx + m[2] * dy + m[4],
                    m[1] * dx + m[3] * dy + m[5],
                ];
                self.outline_into(child, cm, depth + 1, out)?;
                if flags & 0x20 == 0 {
                    break;
                }
            }
            Some(())
        }
    }

    fn simple(
        &self,
        start: usize,
        n: usize,
        end: usize,
        m: [f64; 6],
        out: &mut Vec<Contour>,
    ) -> Option<()> {
        let d = &self.data;
        let mut ends = Vec::with_capacity(n);
        for i in 0..n {
            ends.push(usize::from(u16_at(d, start + 10 + 2 * i)?));
        }
        let points = ends.last().map_or(0, |e| e + 1);
        if points > 4096 || ends.windows(2).any(|w| w[1] < w[0]) {
            return None;
        }
        let ins = usize::from(u16_at(d, start + 10 + 2 * n)?);
        let mut at = start + 12 + 2 * n + ins;
        let mut flags = Vec::with_capacity(points);
        while flags.len() < points {
            let f = *d.get(at)?;
            at += 1;
            flags.push(f);
            if f & 8 != 0 {
                let r = *d.get(at)?;
                at += 1;
                for _ in 0..r {
                    flags.push(f);
                }
            }
        }
        flags.truncate(points);
        let mut coords = |short: u8, same: u8| -> Option<Vec<f64>> {
            let mut v = 0i32;
            let mut list = Vec::with_capacity(points);
            for &f in &flags {
                if f & short != 0 {
                    let b = i32::from(*d.get(at)?);
                    at += 1;
                    v += if f & same != 0 { b } else { -b };
                } else if f & same == 0 {
                    v += i32::from(i16_at(d, at)?);
                    at += 2;
                }
                list.push(f64::from(v));
            }
            Some(list)
        };
        let xs = coords(0x02, 0x10)?;
        let ys = coords(0x04, 0x20)?;
        if at > end {
            return None;
        }
        let tf = |x: f64, y: f64| (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5]);
        let mut first = 0;
        for &last in &ends {
            let pts: Vec<((f64, f64), bool)> = (first..=last)
                .map(|i| (tf(xs[i], ys[i]), flags[i] & 1 != 0))
                .collect();
            first = last + 1;
            if pts.len() < 2 {
                continue;
            }
            out.push(contour(&pts));
        }
        Some(())
    }
}

/// TrueType points (on/off curve) -> start point and segments.
fn contour(pts: &[((f64, f64), bool)]) -> Contour {
    let mid = |a: (f64, f64), b: (f64, f64)| ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
    let n = pts.len();
    // Start on an on-curve point (or the midpoint of two off-curve ones).
    let (start, first) = match pts.iter().position(|p| p.1) {
        Some(i) => (pts[i].0, i),
        None => (mid(pts[0].0, pts[n - 1].0), 0),
    };
    let mut segs = Vec::with_capacity(n);
    let mut ctrl: Option<(f64, f64)> = None;
    let offset = if pts[first].1 { 1 } else { 0 };
    for k in 0..n {
        let (p, on) = pts[(first + offset + k) % n];
        match (on, ctrl) {
            (true, None) => segs.push(PathSeg::Line(p.0, p.1)),
            (true, Some(c)) => {
                segs.push(PathSeg::Quad(c.0, c.1, p.0, p.1));
                ctrl = None;
            }
            (false, None) => ctrl = Some(p),
            (false, Some(c)) => {
                let m = mid(c, p);
                segs.push(PathSeg::Quad(c.0, c.1, m.0, m.1));
                ctrl = Some(p);
            }
        }
    }
    if let Some(c) = ctrl {
        segs.push(PathSeg::Quad(c.0, c.1, start.0, start.1));
    } else if !matches!(segs.last(), Some(PathSeg::Line(x, y)) if (*x, *y) == start) {
        segs.push(PathSeg::Line(start.0, start.1));
    }
    Contour { start, segs }
}

// ---------------------------------------------------------------------------
// Font: a face at a pixel size, with its glyph cache
// ---------------------------------------------------------------------------

struct Glyph {
    mask: Gray,
    left: i32,
    top: i32,
}

pub struct Font {
    face: Rc<Face>,
    px: i32,
    scale: f64,
    embolden: bool,
    metrics: FontMetrics,
    cache: RefCell<HashMap<char, Rc<Glyph>>>,
    id: usize,
}

thread_local! {
    static NEXT_ID: RefCell<usize> = const { RefCell::new(1) };
}

/// What an edit control needs to draw with a font it was handed by handle.
pub struct FontShared {
    pub face: Rc<Face>,
    pub px: i32,
    pub bold: bool,
}

impl Font {
    /// `px` pixels em height, bold or normal.
    pub fn new(family: &str, px: i32, bold: bool, _quality: Quality) -> Option<Font> {
        let (face, embolden) = resolve(family, bold)?;
        let px = px.clamp(1, 512);
        let scale = f64::from(px) / face.upem;
        let metrics = FontMetrics {
            ascent: (face.ascent * scale).round() as i32,
            descent: (face.descent * scale).round() as i32,
        };
        let id = NEXT_ID.with(|n| {
            let mut n = n.borrow_mut();
            *n += 1;
            *n
        });
        Some(Font {
            face,
            px,
            scale,
            embolden,
            metrics,
            cache: RefCell::new(HashMap::new()),
            id,
        })
    }

    pub fn metrics(&self) -> FontMetrics {
        self.metrics
    }

    /// The pixel advance of one character, rounded as GDI rounds hinted
    /// advances (whole pixels per glyph).
    fn advance_px(&self, ch: char) -> i32 {
        let g = self.face.glyph_index(ch);
        let extra = if self.embolden { 1 } else { 0 };
        (self.face.advance(g) * self.scale).round() as i32 + extra
    }

    /// Text width in pixels.
    pub fn measure(&self, s: &str) -> i32 {
        s.chars().map(|c| self.advance_px(c)).sum()
    }

    /// A number other code can hand back to find this font's face (the
    /// edit control's WM_SETFONT).
    pub fn handle(&self) -> isize {
        let shared = Rc::new(FontShared {
            face: self.face.clone(),
            px: self.px,
            bold: self.embolden,
        });
        HANDLES.with(|h| h.borrow_mut().insert(self.id, shared));
        self.id as isize
    }

    /// The outline of one character in pixels (x right, y up from the pen
    /// origin on the baseline), for the tray icon.
    pub fn outline_px(&self, ch: char) -> Option<Vec<Contour>> {
        let g = self.face.glyph_index(ch);
        let s = self.scale;
        let scaled = |x: f64, y: f64| (x * s, y * s);
        Some(
            self.face
                .outline(g)
                .into_iter()
                .map(|c| Contour {
                    start: scaled(c.start.0, c.start.1),
                    segs: c
                        .segs
                        .iter()
                        .map(|seg| match *seg {
                            PathSeg::Line(x, y) => {
                                let (x, y) = scaled(x, y);
                                PathSeg::Line(x, y)
                            }
                            PathSeg::Quad(cx, cy, x, y) => {
                                let (cx, cy) = scaled(cx, cy);
                                let (x, y) = scaled(x, y);
                                PathSeg::Quad(cx, cy, x, y)
                            }
                        })
                        .collect(),
                })
                .collect(),
        )
    }

    fn glyph(&self, ch: char) -> Rc<Glyph> {
        if let Some(g) = self.cache.borrow().get(&ch) {
            return g.clone();
        }
        let glyph = Rc::new(self.render(ch));
        let mut cache = self.cache.borrow_mut();
        if cache.len() > 512 {
            cache.clear();
        }
        cache.insert(ch, glyph.clone());
        glyph
    }

    fn render(&self, ch: char) -> Glyph {
        let contours = self.outline_px(ch).unwrap_or_default();
        let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        let mut see = |x: f64, y: f64| {
            x0 = x0.min(x);
            x1 = x1.max(x);
            y0 = y0.min(y);
            y1 = y1.max(y);
        };
        for c in &contours {
            see(c.start.0, c.start.1);
            for s in &c.segs {
                match *s {
                    PathSeg::Line(x, y) => see(x, y),
                    PathSeg::Quad(cx, cy, x, y) => {
                        see(cx, cy);
                        see(x, y);
                    }
                }
            }
        }
        if contours.is_empty() || x0 > x1 {
            return Glyph {
                mask: Gray::new(0, 0, 0),
                left: 0,
                top: 0,
            };
        }
        let left = x0.floor() as i32 - 1;
        let right = x1.ceil() as i32 + 2;
        let top = (-y1).floor() as i32 - 1;
        let bottom = (-y0).ceil() as i32 + 1;
        let (w, h) = (
            (right - left).max(1) as usize,
            (bottom - top).max(1) as usize,
        );
        let mut mask = gfx::fill_contours(&contours, f64::from(-left), f64::from(-top), w, h);
        if self.embolden {
            // Synthetic bold: widen every stem by one pixel.
            let src = mask.px.clone();
            for y in 0..h {
                for x in 1..w {
                    let i = y * w + x;
                    mask.px[i] = mask.px[i].max(src[i - 1]);
                }
            }
        }
        // A little stem darkening, as FreeType applies for light text.
        for v in &mut mask.px {
            *v = DARKEN[usize::from(*v)];
        }
        Glyph { mask, left, top }
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
        let baseline = y + self.metrics.ascent;
        let mut pen = x;
        for ch in s.chars() {
            let g = self.glyph(ch);
            let (gx, gy) = (pen + g.left, baseline + g.top);
            for yy in 0..g.mask.h {
                let ty = gy + yy as i32;
                if ty < clip.top || ty >= clip.bottom {
                    continue;
                }
                for xx in 0..g.mask.w {
                    let tx = gx + xx as i32;
                    if tx < clip.left || tx >= clip.right {
                        continue;
                    }
                    let a = u32::from(g.mask.px[yy * g.mask.w + xx]);
                    if a != 0 {
                        crate::os::unix::surface::blend_pixel(
                            &mut px[ty as usize * w + tx as usize],
                            rgb,
                            a,
                        );
                    }
                }
            }
            pen += self.advance_px(ch);
        }
    }
}

thread_local! {
    static HANDLES: RefCell<HashMap<usize, Rc<FontShared>>> = RefCell::new(HashMap::new());
}

/// The font a handle names, if it is still alive.
pub fn from_handle(handle: isize) -> Option<Font> {
    let shared = HANDLES.with(|h| h.borrow().get(&(handle as usize)).cloned())?;
    let scale = f64::from(shared.px) / shared.face.upem;
    let metrics = FontMetrics {
        ascent: (shared.face.ascent * scale).round() as i32,
        descent: (shared.face.descent * scale).round() as i32,
    };
    Some(Font {
        face: shared.face.clone(),
        px: shared.px,
        scale,
        embolden: shared.bold,
        metrics,
        cache: RefCell::new(HashMap::new()),
        id: handle as usize,
    })
}

/// Coverage boost: v' = 255 * (v/255)^0.8.
static DARKEN: [u8; 256] = {
    // const-evaluable approximation of x^0.8 via a cubic fit.
    let mut t = [0u8; 256];
    let mut i = 0;
    while i < 256 {
        let x = i as f64 / 255.0;
        let y = x * (1.62 - x * (0.96 - x * 0.34));
        let v = (y * 255.0 + 0.5) as i64;
        t[i] = if v > 255 { 255 } else { v as u8 };
        i += 1;
    }
    t
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn darken_table() {
        assert_eq!(DARKEN[0], 0);
        assert_eq!(DARKEN[255], 255);
        assert!(DARKEN[128] > 128);
        assert!(DARKEN.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn contour_from_points() {
        // A square with an off-curve corner: the curve closes back to start.
        let pts = [
            ((0.0, 0.0), true),
            ((10.0, 0.0), true),
            ((10.0, 10.0), false),
            ((0.0, 10.0), true),
        ];
        let c = contour(&pts);
        assert_eq!(c.start, (0.0, 0.0));
        assert_eq!(c.segs.len(), 3);
        assert!(matches!(c.segs[1], PathSeg::Quad(10.0, 10.0, 0.0, 10.0)));
        // All off-curve: starts at an implied midpoint.
        let ring = [
            ((0.0, 0.0), false),
            ((10.0, 0.0), false),
            ((10.0, 10.0), false),
        ];
        let c = contour(&ring);
        assert_eq!(c.start, (5.0, 5.0));
        assert_eq!(c.segs.len(), 3);
    }

    #[test]
    fn garbage_is_rejected() {
        assert!(Face::parse(vec![0; 10]).is_none());
        assert!(Face::parse(b"OTTO\0\0\0\0\0\0\0\0".to_vec()).is_none());
        let mut fake = vec![0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0];
        fake.extend_from_slice(b"head");
        fake.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0, 0xFF, 0xFF, 0xFF, 0xFF]);
        assert!(Face::parse(fake).is_none());
    }

    /// The installed system fonts, where there are any.
    #[test]
    fn system_fonts() {
        if families().is_empty() {
            return;
        }
        let f = Font::new("Segoe UI", 12, false, Quality::Default).unwrap();
        let m = f.metrics();
        assert!(m.ascent > 8 && m.ascent < 16 && m.descent > 1 && m.descent < 6);
        let w = f.measure("Week");
        assert!(w > 20 && w < 45, "{w}");
        assert_eq!(f.measure(""), 0);
        let bold = Font::new("Segoe UI", 12, true, Quality::Default).unwrap();
        assert!(bold.measure("Week") >= w);
        let mut px = vec![0u32; 60 * 20];
        let clip = Rect {
            left: 0,
            top: 0,
            right: 60,
            bottom: 20,
        };
        f.draw_text(&mut px, 60, clip, 2, 2, "Wé", 0xFFFFFF);
        let lit = px.iter().filter(|&&p| p != 0).count();
        assert!(lit > 20, "{lit}");
        // Drawing far outside stays inside the buffer.
        f.draw_text(&mut px, 60, clip, -500, 400, "Week", 0xFFFFFF);
        let mono = Font::new("Courier New", 12, false, Quality::Default).unwrap();
        assert_eq!(mono.measure("iiii"), mono.measure("WWWW"));
        assert!(f.outline_px('8').is_some_and(|c| c.len() >= 3));
        let h = f.handle();
        assert!(from_handle(h).is_some_and(|g| g.measure("Week") == w));
    }
}
