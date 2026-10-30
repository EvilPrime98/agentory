//! Pure pieces of terminal mouse support: content-anchored selection, word boundaries, copy text
//! assembly, scroll tracking and mouse-report encoding. Nothing here touches GPUI or the PTY.

/// A cell in content coordinates. `line` counts every row that ever entered the buffer (history
/// first, then the live screen), so it keeps pointing at the same text while output scrolls.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Pt {
    pub line: u64,
    pub col: u16,
}

/// An inclusive range of cells, `start <= end`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    pub start: Pt,
    pub end: Pt,
}

impl Selection {
    pub fn between(a: Pt, b: Pt) -> Self {
        if a <= b {
            Self { start: a, end: b }
        } else {
            Self { start: b, end: a }
        }
    }

    pub fn union(self, other: Self) -> Self {
        Self {
            start: self.start.min(other.start),
            end: self.end.max(other.end),
        }
    }

    /// Selected columns (inclusive) on `line`, for a grid `cols` wide.
    pub fn row_range(&self, line: u64, cols: u16) -> Option<(u16, u16)> {
        if line < self.start.line || line > self.end.line || cols == 0 {
            return None;
        }
        let from = if line == self.start.line {
            self.start.col
        } else {
            0
        };
        let to = if line == self.end.line {
            self.end.col
        } else {
            cols - 1
        };
        let to = to.min(cols - 1);
        (from <= to).then_some((from, to))
    }
}

/// One buffer row as plain text: a string per column (empty for the second half of a wide glyph,
/// a space for blanks) and whether the row soft-wraps into the next one.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LineText {
    pub cells: Vec<String>,
    pub wrapped: bool,
}

const WORD_SEPARATORS: &str = ",│`|:\"'()[]{}<>;";

fn is_separator(s: &str) -> bool {
    s.chars()
        .next()
        .is_none_or(|c| c.is_whitespace() || WORD_SEPARATORS.contains(c))
}

/// Columns (inclusive) of the word under `col`. Whitespace and common punctuation end a word;
/// clicking on a run of separators selects that run.
pub fn word_bounds(line: &LineText, col: u16) -> (u16, u16) {
    let n = line.cells.len();
    if n == 0 {
        return (col, col);
    }
    let mut at = usize::from(col).min(n - 1);
    // The right half of a wide glyph belongs to the cell before it.
    if line.cells[at].is_empty() && at > 0 {
        at -= 1;
    }
    let sep = is_separator(&line.cells[at]);
    let same = |i: usize| line.cells[i].is_empty() || is_separator(&line.cells[i]) == sep;
    let mut lo = at;
    while lo > 0 && same(lo - 1) {
        lo -= 1;
    }
    let mut hi = at;
    while hi + 1 < n && same(hi + 1) {
        hi += 1;
    }
    (lo as u16, hi as u16)
}

/// Text of the selected cells. `rows` holds each selected line with its column range, `None` for a
/// line that already fell out of the buffer. Trailing spaces are trimmed where a logical line
/// ends, soft-wrapped rows are joined without a newline, hard line ends keep theirs.
pub fn selected_text(rows: &[Option<(LineText, (u16, u16))>]) -> String {
    let mut out = String::new();
    let last = rows.len().saturating_sub(1);
    for (i, row) in rows.iter().enumerate() {
        let Some((line, (from, to))) = row else {
            continue;
        };
        let end = usize::from(*to).min(line.cells.len().saturating_sub(1));
        let seg: String = line
            .cells
            .get(usize::from(*from)..=end)
            .map(|c| c.concat())
            .unwrap_or_default();
        if line.wrapped && i != last {
            out.push_str(&seg);
        } else {
            out.push_str(seg.trim_end_matches(' '));
            if i != last {
                out.push('\n');
            }
        }
    }
    out
}

/// How many rows the live screen scrolled up between two snapshots of its row hashes, for when the
/// history is full and its length no longer grows. Finds the smallest shift that lines the old
/// screen up with the new one. `None` when nothing lines up, which is taken as an in-place
/// redraw: a burst that scrolls a whole screen at once is indistinguishable from it.
pub fn scroll_distance(before: &[u64], after: &[u64]) -> Option<usize> {
    let rows = before.len().min(after.len());
    for shift in 0..rows {
        let overlap = rows - shift;
        // Tolerate the bottom rows changing (prompt redraw, status line).
        let need = overlap.saturating_sub(2).max(1);
        if (0..need).all(|i| before[shift + i] == after[i]) {
            return Some(shift);
        }
    }
    None
}

/// xterm mouse report for `button` (already including motion/wheel/modifier bits) at 0-based
/// `col`/`row`. `None` when the encoding cannot express the position.
pub fn encode_mouse(
    encoding: vt100::MouseProtocolEncoding,
    button: u8,
    col: u16,
    row: u16,
    release: bool,
) -> Option<Vec<u8>> {
    let (x, y) = (u32::from(col) + 1, u32::from(row) + 1);
    match encoding {
        vt100::MouseProtocolEncoding::Sgr => {
            Some(format!("\x1b[<{button};{x};{y}{}", if release { 'm' } else { 'M' }).into_bytes())
        }
        enc => {
            let b = if release {
                (button & !0b11) | 3
            } else {
                button
            };
            let mut out = b"\x1b[M".to_vec();
            for v in [u32::from(b) + 32, x + 32, y + 32] {
                if enc == vt100::MouseProtocolEncoding::Utf8 {
                    let mut buf = [0u8; 4];
                    out.extend_from_slice(char::from_u32(v)?.encode_utf8(&mut buf).as_bytes());
                } else {
                    out.push(u8::try_from(v).ok()?);
                }
            }
            Some(out)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(s: &str, wrapped: bool) -> LineText {
        LineText {
            cells: s.chars().map(String::from).collect(),
            wrapped,
        }
    }

    #[test]
    fn selection_orders_and_clips_rows() {
        let a = Pt { line: 5, col: 3 };
        let b = Pt { line: 3, col: 7 };
        let s = Selection::between(a, b);
        assert_eq!(s.start, b);
        assert_eq!(s.row_range(3, 10), Some((7, 9)));
        assert_eq!(s.row_range(4, 10), Some((0, 9)));
        assert_eq!(s.row_range(5, 10), Some((0, 3)));
        assert_eq!(s.row_range(6, 10), None);
        assert_eq!(s.row_range(2, 10), None);
    }

    #[test]
    fn words_stop_at_whitespace_and_punctuation() {
        let l = line("git commit -m \"fix bug\" (now)", false);
        assert_eq!(word_bounds(&l, 5), (4, 9));
        assert_eq!(word_bounds(&l, 11), (11, 12));
        assert_eq!(word_bounds(&l, 16), (15, 17));
        assert_eq!(word_bounds(&l, 3), (3, 3));
        let path = line("see src/ui/terminal.rs now", false);
        assert_eq!(word_bounds(&path, 8), (4, 21));
    }

    #[test]
    fn wide_glyph_halves_select_together() {
        let l = LineText {
            cells: vec!["a".into(), "漢".into(), String::new(), "b".into()],
            wrapped: false,
        };
        assert_eq!(word_bounds(&l, 2), (0, 3));
    }

    #[test]
    fn copy_trims_joins_wraps_and_keeps_hard_newlines() {
        let rows = vec![
            Some((line("hello wor", true), (0, 8))),
            Some((line("ld   ", false), (0, 4))),
            Some((line("next  ", false), (0, 5))),
        ];
        assert_eq!(selected_text(&rows), "hello world\nnext");
    }

    #[test]
    fn copy_skips_lines_that_left_the_buffer() {
        let rows = vec![None, Some((line("kept", false), (0, 3)))];
        assert_eq!(selected_text(&rows), "kept");
    }

    #[test]
    fn partial_first_and_last_rows() {
        let rows = vec![
            Some((line("abcdef", false), (2, 5))),
            Some((line("ghijkl", false), (0, 2))),
        ];
        assert_eq!(selected_text(&rows), "cdef\nghi");
    }

    #[test]
    fn scroll_distance_finds_the_shift() {
        let before = [1, 2, 3, 4, 5, 6];
        assert_eq!(scroll_distance(&before, &before), Some(0));
        assert_eq!(scroll_distance(&before, &[3, 4, 5, 6, 7, 8]), Some(2));
        // Bottom rows rewritten (prompt redraw) still lines up.
        assert_eq!(scroll_distance(&before, &[2, 3, 4, 5, 90, 91]), Some(1));
        assert_eq!(scroll_distance(&before, &[70, 71, 72, 73, 74, 75]), None);
    }

    #[test]
    fn sgr_and_legacy_mouse_reports() {
        use vt100::MouseProtocolEncoding as E;
        assert_eq!(
            encode_mouse(E::Sgr, 0, 4, 9, false),
            Some(b"\x1b[<0;5;10M".to_vec())
        );
        assert_eq!(
            encode_mouse(E::Sgr, 0, 4, 9, true),
            Some(b"\x1b[<0;5;10m".to_vec())
        );
        assert_eq!(
            encode_mouse(E::Default, 0, 0, 0, false),
            Some(vec![0x1b, b'[', b'M', 32, 33, 33])
        );
        assert_eq!(
            encode_mouse(E::Default, 0, 0, 0, true),
            Some(vec![0x1b, b'[', b'M', 35, 33, 33])
        );
        assert_eq!(encode_mouse(E::Default, 0, 300, 0, false), None);
        assert!(encode_mouse(E::Utf8, 0, 300, 0, false).is_some());
    }
}
