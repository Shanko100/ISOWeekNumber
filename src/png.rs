//! The one image format the app reads: the PNG frames of its own icon
//! resource (app.ico), for the About window. Only what those frames use is
//! accepted -- 8-bit RGBA or RGB, not interlaced -- and everything is
//! checked: signature, chunk lengths and CRCs, dimensions (at most 1024 px),
//! the inflated size, and the filter type of every row.

use crate::gfx::Rgba;

const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
const MAX_SIDE: u32 = 1024;

fn crc32(parts: &[&[u8]]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for part in parts {
        for &b in *part {
            crc ^= u32::from(b);
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
    }
    !crc
}

fn be32(b: &[u8]) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(..4)?.try_into().ok()?))
}

/// Decode a PNG into straight-alpha RGBA, or None if it is anything else.
pub fn decode(data: &[u8]) -> Option<Rgba> {
    if data.get(..8)? != SIGNATURE {
        return None;
    }
    let mut i = 8;
    let mut header: Option<(u32, u32, usize)> = None;
    let mut idat = Vec::new();
    loop {
        let len = be32(data.get(i..)?)? as usize;
        let kind = data.get(i + 4..i + 8)?;
        let body = data.get(i + 8..(i + 8).checked_add(len)?)?;
        let crc = be32(data.get(i + 8 + len..)?)?;
        if crc32(&[kind, body]) != crc {
            return None;
        }
        match kind {
            b"IHDR" => {
                let (w, h) = (be32(body)?, be32(body.get(4..)?)?);
                let channels = match body.get(8..13)? {
                    [8, 6, 0, 0, 0] => 4,
                    [8, 2, 0, 0, 0] => 3,
                    _ => return None,
                };
                if len != 13 || !(1..=MAX_SIDE).contains(&w) || !(1..=MAX_SIDE).contains(&h) {
                    return None;
                }
                header = Some((w, h, channels));
            }
            b"IDAT" => idat.extend_from_slice(body),
            b"IEND" => break,
            // Ancillary chunks (lower-case first letter) are skipped; any
            // other critical chunk (a palette) is not ours.
            _ if kind[0].is_ascii_lowercase() => {}
            _ => return None,
        }
        i += 12 + len;
    }
    let (w, h, channels) = header?;
    let (w, h) = (w as usize, h as usize);
    let stride = w * channels;
    let raw_len = (stride + 1) * h;
    let raw = miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(&idat, raw_len).ok()?;
    if raw.len() != raw_len {
        return None;
    }
    let mut out = vec![0u8; stride * h];
    for y in 0..h {
        let filter = raw[y * (stride + 1)];
        let src = &raw[y * (stride + 1) + 1..(y + 1) * (stride + 1)];
        let (done, rest) = out.split_at_mut(y * stride);
        let prev = if y > 0 {
            &done[(y - 1) * stride..]
        } else {
            &[][..]
        };
        let row = &mut rest[..stride];
        for x in 0..stride {
            let a = if x >= channels {
                i32::from(row[x - channels])
            } else {
                0
            };
            let b = prev.get(x).map_or(0, |&v| i32::from(v));
            let c = if x >= channels {
                prev.get(x - channels).map_or(0, |&v| i32::from(v))
            } else {
                0
            };
            let predicted = match filter {
                0 => 0,
                1 => a,
                2 => b,
                3 => (a + b) / 2,
                4 => {
                    let p = a + b - c;
                    let (pa, pb, pc) = ((p - a).abs(), (p - b).abs(), (p - c).abs());
                    if pa <= pb && pa <= pc {
                        a
                    } else if pb <= pc {
                        b
                    } else {
                        c
                    }
                }
                _ => return None,
            };
            row[x] = src[x].wrapping_add(predicted as u8);
        }
    }
    let px = out
        .chunks_exact(channels)
        .map(|p| [p[0], p[1], p[2], if channels == 4 { p[3] } else { 255 }])
        .collect();
    Some(Rgba { w, h, px })
}

/// From an icon group directory (RT_GROUP_ICON), the resource id of the
/// largest frame (a width byte of 0 means 256).
#[cfg_attr(not(windows), allow(dead_code))]
pub fn largest_icon_id(group: &[u8]) -> Option<u16> {
    let count = usize::from(u16::from_le_bytes(group.get(4..6)?.try_into().ok()?));
    (0..count)
        .filter_map(|i| {
            let e = group.get(6 + 14 * i..20 + 14 * i)?;
            let side = if e[0] == 0 { 256 } else { u32::from(e[0]) };
            Some((side, u16::from_le_bytes([e[12], e[13]])))
        })
        .max_by_key(|&(side, _)| side)
        .map(|(_, id)| id)
}

/// The frames of an .ico file as (side, PNG bytes), in file order (a width
/// byte of 0 means 256). Frames that are not PNG are left out.
#[cfg_attr(windows, allow(dead_code))]
pub fn ico_frames(ico: &[u8]) -> Vec<(u32, &[u8])> {
    let Some(count) = ico
        .get(4..6)
        .map(|b| usize::from(u16::from_le_bytes([b[0], b[1]])))
    else {
        return Vec::new();
    };
    (0..count)
        .filter_map(|i| {
            let e = ico.get(6 + 16 * i..22 + 16 * i)?;
            let side = if e[0] == 0 { 256 } else { u32::from(e[0]) };
            let size = u32::from_le_bytes(e[8..12].try_into().ok()?) as usize;
            let off = u32::from_le_bytes(e[12..16].try_into().ok()?) as usize;
            let data = ico.get(off..off.checked_add(size)?)?;
            data.starts_with(&SIGNATURE).then_some((side, data))
        })
        .collect()
}

/// The decoded frame of an .ico that best serves `side` px: the smallest
/// one at least that large, else the largest.
#[cfg_attr(windows, allow(dead_code))]
pub fn ico_image(ico: &[u8], side: u32) -> Option<Rgba> {
    let frames = ico_frames(ico);
    let pick = frames
        .iter()
        .filter(|(s, _)| *s >= side)
        .min_by_key(|(s, _)| *s)
        .or_else(|| frames.iter().max_by_key(|(s, _)| *s))?;
    decode(pick.1)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The largest frame of app.ico, against Pillow 13's decode of it:
    /// pixel count, a few pixels and a checksum of all of them.
    #[test]
    fn app_icon_frame() {
        let ico = std::fs::read("assets/app.ico").unwrap();
        let frames = ico_frames(&ico);
        assert_eq!(frames.len(), 14);
        let (side, frame) = *frames.iter().max_by_key(|(s, _)| *s).unwrap();
        assert_eq!(side, 256);
        let img = ico_image(&ico, 999).expect("decodes");
        assert_eq!((img.w, img.h), (256, 256));
        let sum: u64 = img
            .px
            .iter()
            .enumerate()
            .map(|(i, p)| {
                (i as u64 % 251 + 1)
                    * (u64::from(p[0])
                        + 3 * u64::from(p[1])
                        + 5 * u64::from(p[2])
                        + 7 * u64::from(p[3]))
            })
            .sum();
        assert_eq!(sum, PILLOW_SUM);
        assert_eq!(img.get(128, 128), PILLOW_CENTRE);
        assert_eq!(img.get(0, 0), PILLOW_CORNER);
        // A damaged byte fails the CRC.
        assert_eq!(ico_image(&ico, 30).map(|i| i.w), Some(32));
        let mut bad = frame.to_vec();
        bad[40] ^= 1;
        assert!(decode(&bad).is_none());
    }

    #[test]
    fn group_directory() {
        let mut g = vec![0, 0, 1, 0, 2, 0];
        g.extend_from_slice(&[16, 16, 0, 0, 1, 0, 32, 0, 0, 0, 0, 0, 1, 0]);
        g.extend_from_slice(&[0, 0, 0, 0, 1, 0, 32, 0, 0, 0, 0, 0, 14, 0]);
        assert_eq!(largest_icon_id(&g), Some(14));
        assert_eq!(largest_icon_id(&g[..10]), None);
    }

    const PILLOW_SUM: u64 = 23_042_972_257;
    const PILLOW_CENTRE: [u8; 4] = [240, 237, 252, 255];
    const PILLOW_CORNER: [u8; 4] = [0, 0, 0, 0];
}
