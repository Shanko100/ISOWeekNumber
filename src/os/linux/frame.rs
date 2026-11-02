//! The frame of a pop-up: rounded corners with the border following the
//! curve, and a soft drop shadow drawn into the transparent margin around
//! it -- what DWM gives the flyout on Windows 11. Compositors show the
//! margin's alpha; without one the corners are cut with the X Shape
//! extension instead (see `shape_rows`).

/// Premultiplied 0xAARRGGBB.
type Px = u32;

/// Coverage (0..=1) of the pixel at (px, py), relative to a corner's
/// top-left, by the quarter circle of radius `r` centred at (c, c);
/// 4x4 samples.
fn coverage(px: f64, py: f64, c: f64, r: f64) -> f64 {
    let mut inside = 0;
    for sy in 0..4 {
        for sx in 0..4 {
            let (dx, dy) = (
                c - (px + (f64::from(sx) + 0.5) / 4.0),
                c - (py + (f64::from(sy) + 0.5) / 4.0),
            );
            if dx <= 0.0 || dy <= 0.0 || dx * dx + dy * dy <= r * r {
                inside += 1;
            }
        }
    }
    f64::from(inside) / 16.0
}

fn mix(a: Px, wa: f64, b: Px, wb: f64) -> Px {
    let ch = |s: u32| {
        let v = f64::from((a >> s) & 0xFF) * wa + f64::from((b >> s) & 0xFF) * wb;
        (v.round() as u32).min(255) << s
    };
    ch(24) | ch(16) | ch(8) | ch(0)
}

/// Round the corners of an opaque `w` x `h` card in place, redrawing its
/// 1px border (the colour at the middle of its left edge) along the curve.
pub fn round(buf: &mut [Px], w: usize, h: usize, r: i32) {
    if r <= 0 || w < 2 || h < 2 || buf.len() < w * h {
        return;
    }
    let border = buf[(h / 2) * w];
    let rf = f64::from(r);
    let n = (r as usize).min(w / 2).min(h / 2);
    for yy in 0..n {
        for xx in 0..n {
            let outer = coverage(xx as f64, yy as f64, rf, rf);
            if outer >= 1.0 {
                continue;
            }
            let inner = coverage(xx as f64, yy as f64, rf, (rf - 1.0).max(0.0));
            for (x, y) in [
                (xx, yy),
                (w - 1 - xx, yy),
                (xx, h - 1 - yy),
                (w - 1 - xx, h - 1 - yy),
            ] {
                let p = &mut buf[y * w + x];
                *p = mix(border, outer - inner, *p, inner);
            }
        }
    }
}

/// The shadow's alpha (0..=255) around a `w` x `h` card with corner radius
/// `r`, for a buffer with an `inset` margin on every side: the rounded
/// card's silhouette moved down by `dy`, box-blurred three times
/// (about Gaussian) with radius `blur`, at `opacity`.
pub fn shadow(
    w: usize,
    h: usize,
    r: i32,
    inset: usize,
    blur: usize,
    dy: usize,
    opacity: f64,
) -> Vec<u8> {
    let (bw, bh) = (w + 2 * inset, h + 2 * inset);
    let mut m = vec![0f32; bw * bh];
    let mut card = vec![0xFF00_0000u32; w * h];
    round(&mut card, w, h, r);
    for y in 0..h {
        let ty = y + inset + dy;
        if ty >= bh {
            break;
        }
        for x in 0..w {
            m[ty * bw + x + inset] = (card[y * w + x] >> 24) as f32 / 255.0;
        }
    }
    let mut tmp = vec![0f32; bw.max(bh)];
    for _ in 0..3 {
        for row in m.chunks_exact_mut(bw) {
            box_blur(row, 1, bw, blur, &mut tmp);
        }
        for x in 0..bw {
            box_blur(&mut m[x..], bw, bh, blur, &mut tmp);
        }
    }
    m.iter()
        .map(|v| (f64::from(*v) * opacity * 255.0).round().clamp(0.0, 255.0) as u8)
        .collect()
}

/// One box blur over `n` values `stride` apart, starting at `v[0]`.
fn box_blur(v: &mut [f32], stride: usize, n: usize, r: usize, tmp: &mut [f32]) {
    if r == 0 || n == 0 {
        return;
    }
    let get = |v: &[f32], i: isize| -> f32 {
        if i < 0 || i as usize >= n {
            0.0
        } else {
            v[i as usize * stride]
        }
    };
    let k = (2 * r + 1) as f32;
    let mut sum: f32 = (-(r as isize)..=r as isize).map(|i| get(v, i)).sum();
    for (i, t) in tmp.iter_mut().enumerate().take(n) {
        *t = sum / k;
        let i = i as isize;
        sum += get(v, i + r as isize + 1) - get(v, i - r as isize);
    }
    for (i, t) in tmp.iter().enumerate().take(n) {
        v[i * stride] = *t;
    }
}

/// The card (already rounded) over its shadow, in a buffer `inset` larger
/// on every side.
pub fn compose(card: &[Px], w: usize, h: usize, inset: usize, shadow: &[u8]) -> Vec<Px> {
    let bw = w + 2 * inset;
    let mut out: Vec<Px> = shadow.iter().map(|a| u32::from(*a) << 24).collect();
    if out.len() < bw * (h + 2 * inset) || card.len() < w * h {
        return card.to_vec();
    }
    for y in 0..h {
        for x in 0..w {
            let c = card[y * w + x];
            let o = &mut out[(y + inset) * bw + x + inset];
            let ca = f64::from(c >> 24) / 255.0;
            *o = mix(c, 1.0, *o, 1.0 - ca);
        }
    }
    out
}

/// The rounded rectangle as rows of (x, y, width, height) rectangles, for
/// the Shape extension where no compositor shows transparency.
pub fn shape_rows(w: i32, h: i32, r: i32) -> Vec<(i16, i16, u16, u16)> {
    let r = r.min(w / 2).min(h / 2).max(0);
    let mut rows = Vec::new();
    for y in 0..r {
        // Inset of row y: where the circle centred at (r, r) crosses it.
        let dy = f64::from(r) - (f64::from(y) + 0.5);
        let dx = (f64::from(r * r) - dy * dy).max(0.0).sqrt();
        let inset = (f64::from(r) - dx).round() as i32;
        let width = (w - 2 * inset).max(0) as u16;
        rows.push((inset as i16, y as i16, width, 1));
        rows.push((inset as i16, (h - 1 - y) as i16, width, 1));
    }
    rows.push((0, r as i16, w as u16, (h - 2 * r).max(0) as u16));
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corners_and_border() {
        // A white card with a 1px grey border.
        let (w, h) = (40, 30);
        let mut buf = vec![0xFFFF_FFFFu32; w * h];
        for y in 0..h {
            for x in 0..w {
                if x == 0 || y == 0 || x == w - 1 || y == h - 1 {
                    buf[y * w + x] = 0xFF80_8080;
                }
            }
        }
        round(&mut buf, w, h, 8);
        assert_eq!(buf[0] >> 24, 0, "corner cleared");
        assert_eq!(buf[w - 1] >> 24, 0);
        assert_eq!(buf[(h - 1) * w] >> 24, 0);
        assert_eq!(buf[15 * w + 20], 0xFFFF_FFFF, "middle untouched");
        // Premultiplied everywhere.
        assert!(buf.iter().all(|p| (p & 0xFF) <= p >> 24));
        // The curve carries the border: some pixel on the diagonal is grey.
        let diag: Vec<u32> = (0..8).map(|i| buf[i * w + i]).collect();
        assert!(
            diag.iter().any(|p| p >> 24 > 200 && (p & 0xFF) < 0xC0),
            "{diag:x?}"
        );
    }

    #[test]
    fn shadow_falls_off_and_sits_below() {
        let (w, h, inset) = (60, 40, 14);
        let s = shadow(w, h, 8, inset, 4, 2, 0.3);
        let bw = w + 2 * inset;
        assert_eq!(s.len(), bw * (h + 2 * inset));
        // Strongest under the card, nothing at the buffer's corner.
        assert_eq!(s[0], 0);
        let mid = s[(inset + h / 2) * bw + inset + w / 2];
        assert!((70..=80).contains(&mid), "{mid}");
        // Offset down: more shadow below the card than above it.
        let above = s[(inset - 4) * bw + bw / 2];
        let below = s[(inset + h + 4) * bw + bw / 2];
        assert!(below > above, "{above} {below}");
    }

    #[test]
    fn compose_keeps_card_and_premultiplies() {
        let (w, h, inset) = (20, 10, 6);
        let mut card = vec![0xFF20_4060u32; w * h];
        round(&mut card, w, h, 4);
        let s = shadow(w, h, 4, inset, 2, 1, 0.3);
        let out = compose(&card, w, h, inset, &s);
        let bw = w + 2 * inset;
        assert_eq!(out[(inset + 5) * bw + inset + 10], 0xFF20_4060);
        assert!(out.iter().all(|p| ((p >> 16) & 0xFF) <= p >> 24));
        // Shadow pixels are black with some alpha.
        let p = out[(inset + h + 1) * bw + bw / 2];
        assert!(p >> 24 > 0 && p & 0x00FF_FFFF == 0, "{p:x}");
    }

    #[test]
    fn shape_covers_the_rounded_rect() {
        let rows = shape_rows(100, 50, 8);
        let area: u32 = rows.iter().map(|r| u32::from(r.2) * u32::from(r.3)).sum();
        // A bit less than the full rectangle: the four corners are cut.
        assert!(area < 5000 && area > 5000 - 4 * 64, "{area}");
        assert_eq!(rows[0].1, 0);
        assert!(rows[0].0 > 0, "top row is inset");
    }
}
