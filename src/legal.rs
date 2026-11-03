//! The Legal notices text: composed and deflated by build.rs, read with the
//! same distrust as the other embedded data (size-capped inflate, strict
//! UTF-8, no control characters but newlines), then split into styled runs
//! exactly as weeknumber_license_dialog.split_sections / styled_runs do.

use crate::pack::{self, Cursor};

static LEGAL: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/legal.bin"));
const MAX_LEGAL_BYTES: usize = 1024 * 1024;
/// A rule line of build.rs's section(): 76 equals signs; 20 is enough.
const RULE_MIN: usize = 20;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Style {
    /// A section title, in the accent colour.
    Heading,
    /// A Markdown heading inside a section, in bold.
    Subheading,
    /// The opening paragraph, as prose.
    Intro,
    /// Everything else, monospaced.
    Body,
    /// The notices themselves, reflowed as prose in the UI font (the
    /// Legal notices window's GUI improvement; not made by styled_runs).
    Prose,
}

/// The whole embedded text, or None if it fails any check.
pub fn text() -> Option<String> {
    let mut c = Cursor::new(LEGAL);
    if c.bytes(4)? != b"WNL1" {
        return None;
    }
    let len = c.count(MAX_LEGAL_BYTES)?;
    let raw = pack::inflate(c.rest(), len, MAX_LEGAL_BYTES)?;
    let s = String::from_utf8(raw).ok()?;
    (!s.chars().any(|ch| ch.is_control() && ch != '\n')).then_some(s)
}

/// (title, body) pairs in order. A title is the line between two rule
/// lines, or a line underlined with "=" to its length; text before the
/// first title has no title.
pub fn split_sections(text: &str) -> Vec<(Option<String>, String)> {
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<&str> = text.split('\n').collect();
    let rule = |i: usize, minimum: usize| {
        let line = lines.get(i).map_or("", |l| l.trim());
        line.chars().count() >= minimum && !line.is_empty() && line.chars().all(|c| c == '=')
    };
    let mut sections: Vec<(Option<String>, Vec<&str>)> = vec![(None, Vec::new())];
    let mut i = 0;
    while i < lines.len() {
        if rule(i, RULE_MIN) && rule(i + 2, RULE_MIN) && !lines[i + 1].trim().is_empty() {
            sections.push((Some(lines[i + 1].trim().to_string()), Vec::new()));
            i += 3;
            continue;
        }
        let title = lines[i].trim();
        let underline = RULE_MIN.min(title.chars().count().max(3));
        if !title.is_empty()
            && !rule(i, 1)
            && rule(i + 1, underline)
            && !(i > 0 && rule(i - 1, RULE_MIN))
        {
            sections.push((Some(title.to_string()), Vec::new()));
            i += 2;
            continue;
        }
        if let Some(last) = sections.last_mut() {
            last.1.push(lines[i]);
        }
        i += 1;
    }
    sections
        .into_iter()
        .filter_map(|(title, body)| {
            let joined = body.join("\n").trim_matches('\n').to_string();
            (title.is_some() || !joined.trim().is_empty()).then_some((title, joined))
        })
        .collect()
}

/// A Markdown heading ("## Privacy"): one to three '#', a space, then
/// text starting with a non-space. Returns the text.
fn subheading(line: &str) -> Option<&str> {
    let hashes = line.len() - line.trim_start_matches('#').len();
    let rest = line[hashes..].strip_prefix(' ')?;
    ((1..=3).contains(&hashes) && rest.chars().next().is_some_and(|c| !c.is_whitespace()))
        .then_some(rest)
}

/// The whole text as (chunk, style) runs, in order, each ending in a
/// newline (a heading after the first also starts with one).
pub fn styled_runs(text: &str) -> Vec<(String, Style)> {
    let mut runs = Vec::new();
    let mut first = true;
    for (title, body) in split_sections(text) {
        if let Some(t) = title.filter(|t| !t.is_empty()) {
            let lead = if first { "" } else { "\n" };
            runs.push((format!("{lead}{t}\n"), Style::Heading));
        }
        if !body.is_empty() {
            if first {
                runs.push((format!("{body}\n"), Style::Intro));
            } else {
                let mut chunk: Vec<&str> = Vec::new();
                for line in body.split('\n') {
                    match subheading(line.trim()) {
                        None => chunk.push(line),
                        Some(h) => {
                            if !chunk.is_empty() {
                                runs.push((chunk.join("\n") + "\n", Style::Body));
                                chunk.clear();
                            }
                            runs.push((format!("{h}\n"), Style::Subheading));
                        }
                    }
                }
                if !chunk.is_empty() {
                    runs.push((chunk.join("\n") + "\n", Style::Body));
                }
            }
        }
        first = false;
    }
    runs
}

/// Tk's word wrap: as many characters as fit; a line that overflows breaks
/// after its last blank, keeping the blanks (they may pass the margin); a
/// word longer than the line breaks between characters.
pub fn wrap(chars: &[char], widths: &[i32], avail: i32) -> Vec<(usize, usize)> {
    wrap_hanging(chars, widths, avail, avail)
}

/// wrap() with a different width for the lines after the first (a list
/// item's hanging indent).
pub fn wrap_hanging(chars: &[char], widths: &[i32], first: i32, rest: i32) -> Vec<(usize, usize)> {
    let mut out: Vec<(usize, usize)> = Vec::new();
    let mut start = 0;
    loop {
        let avail = if out.is_empty() { first } else { rest };
        let mut x = 0;
        let mut i = start;
        let mut last_break = None;
        while i < chars.len() {
            let blank = chars[i] == ' ';
            if !blank && x + widths[i] > avail {
                break;
            }
            if blank {
                last_break = Some(i + 1);
            }
            x += widths[i];
            i += 1;
        }
        if i >= chars.len() {
            out.push((start, chars.len()));
            return out;
        }
        let mut end = match last_break {
            Some(b) if b > start => b,
            _ => i.max(start + 1),
        };
        while end < chars.len() && chars[end] == ' ' {
            end += 1;
        }
        out.push((start, end));
        start = end;
    }
}

/// A Markdown section body as readable prose: its paragraphs and list
/// items, each joined into one line ("- " items start with a bullet, their
/// indented continuation lines join them), code backticks dropped. Returns
/// (text, is a list item, ends a block of text).
pub fn reflow(body: &str) -> Vec<(String, bool, bool)> {
    let mut out: Vec<(String, bool, bool)> = Vec::new();
    let mut open = false;
    for line in body.split('\n') {
        let t = line.trim();
        if t.is_empty() {
            if let Some(last) = out.last_mut() {
                last.2 = true;
            }
            open = false;
            continue;
        }
        let t = t.replace('`', "");
        if let Some(item) = t.strip_prefix("- ") {
            out.push((format!("\u{2022}  {item}"), true, false));
            open = true;
        } else if open && let Some(last) = out.last_mut() {
            last.0.push(' ');
            last.0.push_str(&t);
        } else {
            out.push((t, false, false));
            open = true;
        }
    }
    if let Some(last) = out.last_mut() {
        last.2 = true;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_text_reads() {
        let t = text().expect("legal text");
        assert!(t.starts_with("WeekNumber Workplace Edition"));
        for needle in [
            "Legal notices",
            "A non-profit project",
            "WeekNumber: LICENSE",
            "Third-party notices",
            "Build environment",
        ] {
            assert!(t.contains(needle), "{needle}");
        }
        // Only this platform's notices and license texts are included.
        let (mine, others): (&[&str], &[&str]) = if cfg!(windows) {
            (
                &[
                    "for Windows",
                    "licenses/windows-rs-MIT.txt",
                    "Windows is a trademark",
                ],
                &[
                    "licenses/musl-MIT.txt",
                    "Linus Torvalds",
                    "Apple, Mac and macOS",
                    "<!--",
                ],
            )
        } else if cfg!(target_os = "macos") {
            (
                &[
                    "for macOS (Apple silicon)",
                    "Apple, Mac and macOS",
                    "Login Items",
                ],
                &[
                    "licenses/windows-rs-MIT.txt",
                    "licenses/musl-MIT.txt",
                    "Linus Torvalds",
                    "Linux packages",
                    "<!--",
                ],
            )
        } else {
            (
                &[
                    "for Linux",
                    "Linus Torvalds",
                    "licenses/Rust-standard-library-unix.txt",
                    "Linux packages",
                    "org.freedesktop.Platform",
                ],
                &[
                    "licenses/windows-rs-MIT.txt",
                    "Apple, Mac and macOS",
                    "HKEY_CURRENT_USER",
                    "<!--",
                ],
            )
        };
        for needle in mine {
            assert!(t.contains(needle), "missing {needle}");
        }
        for needle in others {
            assert!(!t.contains(needle), "unexpected {needle}");
        }
    }

    #[test]
    fn sections_and_runs() {
        let rule = "=".repeat(76);
        let text = format!(
            "Title here\n==========\n\nIntro line.\n\n\n{rule}\nLegal notices\n{rule}\n\n\
             Para.\n\n## Privacy\n\nNo data.\n#### not a heading\n\n\n{rule}\nLICENSE\n{rule}\n\nMIT\n"
        );
        let s = split_sections(&text);
        assert_eq!(s.len(), 3);
        assert_eq!(s[0], (Some("Title here".into()), "Intro line.".into()));
        assert_eq!(s[1].0.as_deref(), Some("Legal notices"));
        assert_eq!(s[2], (Some("LICENSE".into()), "MIT".into()));
        let runs = styled_runs(&text);
        let styles: Vec<Style> = runs.iter().map(|r| r.1).collect();
        use Style::*;
        assert_eq!(
            styles,
            [
                Heading, Intro, Heading, Body, Subheading, Body, Heading, Body
            ]
        );
        assert_eq!(runs[0].0, "Title here\n");
        assert_eq!(runs[2].0, "\nLegal notices\n");
        assert_eq!(runs[4].0, "Privacy\n");
        assert!(runs[5].0.contains("#### not a heading"));
        assert_eq!(subheading("# A"), Some("A"));
        assert_eq!(subheading("#  A"), None);
        assert_eq!(subheading("#### A"), None);
        assert_eq!(subheading("#A"), None);
    }

    #[test]
    fn word_wrap() {
        let text: Vec<char> = "aa bb cccccccc d".chars().collect();
        let w = vec![1; text.len()];
        // Blanks stay on the line they end; a long word breaks by character.
        assert_eq!(wrap(&text, &w, 5), vec![(0, 6), (6, 11), (11, 16)]);
        assert_eq!(wrap(&text, &w, 100), vec![(0, 16)]);
        assert_eq!(wrap(&[], &[], 5), vec![(0, 0)]);
        // A hanging indent narrows the lines after the first.
        assert_eq!(
            wrap_hanging(&text, &w, 6, 4),
            vec![(0, 6), (6, 10), (10, 15), (15, 16)]
        );
    }

    #[test]
    fn markdown_reflow() {
        let body = "First line\nsecond `code` line.\n\n- Item one\n  goes on\n- Item two\n\nLast.";
        assert_eq!(
            reflow(body),
            vec![
                ("First line second code line.".to_string(), false, true),
                ("\u{2022}  Item one goes on".to_string(), true, false),
                ("\u{2022}  Item two".to_string(), true, true),
                ("Last.".to_string(), false, true),
            ]
        );
    }
}
