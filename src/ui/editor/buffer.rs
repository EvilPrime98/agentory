use std::borrow::Cow;
use std::ops::Range;

use ropey::Rope;
use unicode_segmentation::UnicodeSegmentation as _;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct Point {
    pub line: usize,
    pub column: usize,
}

pub type Edit = (Range<usize>, String);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bias {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CharClass {
    Whitespace,
    Newline,
    Word,
    Punctuation,
}

pub fn char_class(ch: char) -> CharClass {
    if ch == '\n' {
        CharClass::Newline
    } else if ch.is_whitespace() {
        CharClass::Whitespace
    } else if ch.is_alphanumeric() || ch == '_' || ch == '$' {
        CharClass::Word
    } else {
        CharClass::Punctuation
    }
}

pub fn is_word_char(ch: char) -> bool {
    char_class(ch) == CharClass::Word
}

#[derive(Clone)]
pub struct Buffer {
    rope: Rope,
    version: usize,
}

impl Default for Buffer {
    fn default() -> Self {
        Self::new("")
    }
}

pub fn normalize_line_endings(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

impl Buffer {
    pub fn new(text: &str) -> Self {
        let rope = if text.contains('\r') {
            Rope::from_str(&normalize_line_endings(text))
        } else {
            Rope::from_str(text)
        };
        Self { rope, version: 0 }
    }

    pub fn rope(&self) -> &Rope {
        &self.rope
    }

    pub fn version(&self) -> usize {
        self.version
    }

    pub fn len(&self) -> usize {
        self.rope.len_bytes()
    }

    pub fn is_empty(&self) -> bool {
        self.rope.len_bytes() == 0
    }

    pub fn text(&self) -> String {
        self.rope.to_string()
    }

    pub fn line_count(&self) -> usize {
        self.rope.len_lines()
    }

    pub fn line_start(&self, line: usize) -> usize {
        let line = line.min(self.line_count().saturating_sub(1));
        self.rope.line_to_byte(line)
    }

    pub fn line_end(&self, line: usize) -> usize {
        let line = line.min(self.line_count().saturating_sub(1));
        if line + 1 < self.line_count() {
            self.rope.line_to_byte(line + 1) - 1
        } else {
            self.len()
        }
    }

    pub fn line(&self, line: usize) -> Cow<'_, str> {
        let start = self.line_start(line);
        let end = self.line_end(line);
        self.rope.byte_slice(start..end).into()
    }

    pub fn slice(&self, range: Range<usize>) -> String {
        let range = self.clip(range.start)..self.clip(range.end);
        self.rope.byte_slice(range).to_string()
    }

    pub fn line_of(&self, offset: usize) -> usize {
        self.rope.byte_to_line(offset.min(self.len()))
    }

    pub fn offset_to_point(&self, offset: usize) -> Point {
        let offset = offset.min(self.len());
        let line = self.rope.byte_to_line(offset);
        Point {
            line,
            column: offset - self.rope.line_to_byte(line),
        }
    }

    pub fn clip(&self, offset: usize) -> usize {
        let offset = offset.min(self.len());
        let char_index = self.rope.byte_to_char(offset);
        self.rope.char_to_byte(char_index)
    }

    pub fn char_at(&self, offset: usize) -> Option<char> {
        if offset >= self.len() {
            return None;
        }
        self.rope.get_char(self.rope.byte_to_char(offset))
    }

    pub fn char_before(&self, offset: usize) -> Option<char> {
        if offset == 0 || offset > self.len() {
            return None;
        }
        let index = self.rope.byte_to_char(offset);
        if index == 0 {
            None
        } else {
            self.rope.get_char(index - 1)
        }
    }

    pub fn chars_at(&self, offset: usize) -> impl Iterator<Item = char> + '_ {
        self.rope
            .chars_at(self.rope.byte_to_char(offset.min(self.len())))
    }

    pub fn reversed_chars_at(&self, offset: usize) -> impl Iterator<Item = char> + '_ {
        let mut chars = self
            .rope
            .chars_at(self.rope.byte_to_char(offset.min(self.len())));
        std::iter::from_fn(move || chars.prev())
    }

    pub fn next_boundary(&self, offset: usize) -> usize {
        if offset >= self.len() {
            return self.len();
        }
        let point = self.offset_to_point(offset);
        let line = self.line(point.line);
        if point.column >= line.len() {
            return offset + 1;
        }
        let next = line[point.column..]
            .graphemes(true)
            .next()
            .map(str::len)
            .unwrap_or(1);
        offset + next
    }

    pub fn previous_boundary(&self, offset: usize) -> usize {
        if offset == 0 {
            return 0;
        }
        let point = self.offset_to_point(offset);
        if point.column == 0 {
            return offset - 1;
        }
        let line = self.line(point.line);
        let previous = line[..point.column]
            .graphemes(true)
            .next_back()
            .map(str::len)
            .unwrap_or(1);
        offset - previous
    }

    pub fn word_range_at(&self, offset: usize) -> Option<Range<usize>> {
        let after = self.char_at(offset).filter(|ch| is_word_char(*ch));
        let before = self.char_before(offset).filter(|ch| is_word_char(*ch));
        if after.is_none() && before.is_none() {
            return None;
        }
        let start = offset
            - self
                .reversed_chars_at(offset)
                .take_while(|ch| is_word_char(*ch))
                .map(char::len_utf8)
                .sum::<usize>();
        let end = offset
            + self
                .chars_at(offset)
                .take_while(|ch| is_word_char(*ch))
                .map(char::len_utf8)
                .sum::<usize>();
        Some(start..end)
    }

    pub fn indent_len(&self, line: usize) -> usize {
        self.line(line)
            .bytes()
            .take_while(|byte| *byte == b' ' || *byte == b'\t')
            .count()
    }

    pub fn is_blank_line(&self, line: usize) -> bool {
        self.line(line).trim().is_empty()
    }

    pub fn apply(&mut self, edits: &[Edit]) {
        for (range, text) in edits.iter().rev() {
            let start = self.clip(range.start);
            let end = self.clip(range.end.max(start));
            let start_char = self.rope.byte_to_char(start);
            let end_char = self.rope.byte_to_char(end);
            if end_char > start_char {
                self.rope.remove(start_char..end_char);
            }
            let text = if text.contains('\r') {
                Cow::Owned(normalize_line_endings(text))
            } else {
                Cow::Borrowed(text.as_str())
            };
            if !text.is_empty() {
                self.rope.insert(start_char, &text);
            }
        }
        self.version += 1;
    }
}

pub fn display_column(line: &str, column: usize, tab_size: usize) -> usize {
    let mut display = 0;
    for ch in line[..column.min(line.len())].chars() {
        if ch == '\t' {
            display += tab_size - display % tab_size;
        } else {
            display += 1;
        }
    }
    display
}

pub fn normalize_edits(mut edits: Vec<Edit>) -> Vec<Edit> {
    edits.sort_by_key(|(range, _)| (range.start, range.end));
    let mut merged: Vec<Edit> = Vec::with_capacity(edits.len());
    for (range, text) in edits {
        if let Some((last, last_text)) = merged.last_mut()
            && range.start < last.end
        {
            last.end = last.end.max(range.end);
            last_text.push_str(&text);
            continue;
        }
        merged.push((range, text));
    }
    merged
}

pub fn map_offset(offset: usize, edits: &[Edit], bias: Bias) -> usize {
    let mut delta: isize = 0;
    for (range, text) in edits {
        if range.start > offset {
            break;
        }
        let inserted = text.len() as isize;
        let removed = (range.end - range.start) as isize;
        if range.end < offset {
            delta += inserted - removed;
            continue;
        }
        let start = range.start as isize + delta;
        let after = (start + inserted) as usize;
        return match (offset == range.start, offset == range.end, bias) {
            (_, true, _) if range.end > range.start => after,
            (true, _, Bias::Left) => start as usize,
            (true, _, Bias::Right) => after,
            (_, _, Bias::Left) => start as usize,
            (_, _, Bias::Right) => after,
        };
    }
    (offset as isize + delta) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_and_points() {
        let buffer = Buffer::new("one\ntwo\n\nfour");
        assert_eq!(buffer.line_count(), 4);
        assert_eq!(buffer.line(1), "two");
        assert_eq!(buffer.line(2), "");
        assert_eq!(buffer.line_end(0), 3);
        assert_eq!(buffer.offset_to_point(5), Point { line: 1, column: 1 });
        assert_eq!(Buffer::new("a\n").line_count(), 2);
    }

    #[test]
    fn line_endings_are_normalised() {
        assert_eq!(Buffer::new("a\r\nb\rc").text(), "a\nb\nc");
        assert_eq!(Buffer::new("").text(), "");
        assert_eq!(Buffer::new("").line_count(), 1);
    }

    #[test]
    fn apply_batches() {
        let mut buffer = Buffer::new("hello world");
        let edits = vec![
            (0..5, "goodbye".to_string()),
            (6..11, "moon\nrise".to_string()),
        ];
        buffer.apply(&edits);
        assert_eq!(buffer.text(), "goodbye moon\nrise");
        assert_eq!(buffer.version(), 1);
    }

    #[test]
    fn graphemes_and_words() {
        let buffer = Buffer::new("e\u{301}x foo_bar.baz");
        assert_eq!(buffer.next_boundary(0), 3);
        assert_eq!(buffer.previous_boundary(3), 0);
        assert_eq!(buffer.word_range_at(7), Some(5..12));
        assert_eq!(buffer.word_range_at(4), Some(3..4));
        assert_eq!(Buffer::new("a  b").word_range_at(2), None);
    }

    #[test]
    fn offsets_map_through_edits() {
        let edits = vec![(2..2, "xx".to_string()), (5..7, String::new())];
        assert_eq!(map_offset(1, &edits, Bias::Right), 1);
        assert_eq!(map_offset(2, &edits, Bias::Left), 2);
        assert_eq!(map_offset(2, &edits, Bias::Right), 4);
        assert_eq!(map_offset(6, &edits, Bias::Left), 7);
        assert_eq!(map_offset(9, &edits, Bias::Left), 9);
        let merged = normalize_edits(vec![(4..8, String::new()), (0..5, String::new())]);
        assert_eq!(merged, vec![(0..8, String::new())]);
    }

    #[test]
    fn tabs_advance_to_stops() {
        assert_eq!(display_column("a\tb", 2, 4), 4);
        assert_eq!(display_column("abcd\tb", 5, 4), 8);
    }
}
