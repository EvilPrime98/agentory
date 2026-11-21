
use std::ops::Range;

use super::buffer::{Bias, Buffer, Edit, display_column, map_offset, normalize_edits};
use super::history::{EditKind, History, Transaction};
use super::selection::{Selection, normalize};

#[derive(Clone, Copy, Debug)]
pub enum Target {
    InEdit {
        at: usize,
        anchor: usize,
        head: usize,
    },
    Mapped { anchor: usize, head: usize },
    Exact { anchor: usize, head: usize },
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ClipboardText {
    pub text: String,
    pub whole_lines: bool,
    pub pieces: Vec<String>,
}

pub struct Document {
    pub buffer: Buffer,
    pub selections: Vec<Selection>,
    pub history: History,
    next_selection_id: usize,
    pub tab_size: usize,
    pub hard_tabs: bool,
}

fn in_edit(anchor: usize, head: usize) -> Target {
    Target::InEdit {
        at: 0,
        anchor,
        head,
    }
}

impl Document {
    pub fn new(text: &str) -> Self {
        Self {
            buffer: Buffer::new(text),
            selections: vec![Selection::cursor(0, 0)],
            history: History::default(),
            next_selection_id: 1,
            tab_size: 4,
            hard_tabs: false,
        }
    }

    pub fn reset(&mut self, text: &str, caret: usize) {
        self.buffer = Buffer::new(text);
        self.history = History::default();
        self.set_cursor(caret);
    }

    pub fn indent_unit(&self) -> String {
        if self.hard_tabs {
            "\t".into()
        } else {
            " ".repeat(self.tab_size)
        }
    }

    pub fn newest_selection(&self) -> &Selection {
        self.selections
            .iter()
            .max_by_key(|selection| selection.id)
            .expect("there is always a selection")
    }

    pub fn new_selection_id(&mut self) -> usize {
        self.next_selection_id += 1;
        self.next_selection_id
    }

    pub fn set_selections(&mut self, selections: Vec<Selection>) {
        let len = self.buffer.len();
        let selections = selections
            .into_iter()
            .map(|mut selection| {
                selection.anchor = self.buffer.clip(selection.anchor.min(len));
                selection.head = self.buffer.clip(selection.head.min(len));
                selection
            })
            .collect::<Vec<_>>();
        self.selections = if selections.is_empty() {
            vec![Selection::cursor(self.new_selection_id(), 0)]
        } else {
            normalize(selections)
        };
    }

    pub fn reset_goal(&mut self) {
        for selection in &mut self.selections {
            selection.goal_column = None;
        }
    }

    pub fn set_cursor(&mut self, offset: usize) {
        let id = self.new_selection_id();
        self.set_selections(vec![Selection::cursor(id, offset)]);
    }

    pub fn select_range(&mut self, range: Range<usize>) {
        let id = self.new_selection_id();
        let mut selection = Selection::cursor(id, 0);
        selection.set_range(range, false);
        self.set_selections(vec![selection]);
    }

    pub fn select_all(&mut self) {
        self.select_range(0..self.buffer.len());
    }

    pub fn apply(&mut self, items: Vec<(Option<Edit>, Target)>, kind: EditKind) {
        let mut edits = Vec::with_capacity(items.len());
        let mut targets = Vec::with_capacity(items.len());
        for (edit, target) in items {
            let target = match (target, &edit) {
                (Target::InEdit { anchor, head, .. }, Some((range, _))) => Target::InEdit {
                    at: range.start,
                    anchor,
                    head,
                },
                (target, _) => target,
            };
            edits.extend(edit);
            targets.push(target);
        }
        self.apply_edits(edits, targets, kind);
    }

    pub fn apply_edits(&mut self, edits: Vec<Edit>, targets: Vec<Target>, kind: EditKind) {
        let ids: Vec<usize> = self
            .selections
            .iter()
            .map(|selection| selection.id)
            .collect();
        let (edits, targets) = normalize_crlf(edits, targets);
        let edits = normalize_edits(edits);
        let before = self.selections.clone();
        let changed = !edits.is_empty();
        if changed {
            let transaction = Transaction::record(&self.buffer, &edits);
            self.buffer.apply(&edits);
            self.history.push(transaction, before, Vec::new(), kind);
        }
        let mut selections = Vec::with_capacity(targets.len());
        for (index, target) in targets.into_iter().enumerate() {
            let (anchor, head) = match target {
                Target::InEdit { at, anchor, head } => {
                    let base = map_offset(at, &edits, Bias::Left);
                    (base + anchor, base + head)
                }
                Target::Exact { anchor, head } => (anchor, head),
                Target::Mapped { anchor, head } => (
                    map_offset(anchor, &edits, Bias::Right),
                    map_offset(head, &edits, Bias::Right),
                ),
            };
            let id = match ids.get(index) {
                Some(id) => *id,
                None => self.new_selection_id(),
            };
            selections.push(Selection {
                id,
                anchor,
                head,
                goal_column: None,
            });
        }
        self.set_selections(selections);
        if changed {
            self.history.set_selections_after(self.selections.clone());
        }
    }

    pub fn insert(&mut self, text: &str) {
        self.insert_plain(text, EditKind::Insert);
    }

    pub fn insert_plain(&mut self, text: &str, kind: EditKind) {
        let items = self
            .selections
            .iter()
            .map(|selection| {
                (
                    Some((selection.range(), text.to_string())),
                    in_edit(text.len(), text.len()),
                )
            })
            .collect();
        self.apply(items, kind);
    }

    pub fn newline(&mut self) {
        let mut items = Vec::with_capacity(self.selections.len());
        for selection in &self.selections {
            let range = selection.range();
            let line = self.buffer.line_of(range.start);
            let line_start = self.buffer.line_start(line);
            let text = self.buffer.line(line);
            let column = (range.start - line_start).min(text.len());
            let indent: String = text[..column]
                .chars()
                .take_while(|ch| *ch == ' ' || *ch == '\t')
                .collect();
            let inserted = format!("\n{indent}");
            let caret = inserted.len();
            items.push((Some((range, inserted)), in_edit(caret, caret)));
        }
        self.apply(items, EditKind::Other);
    }

    pub fn backspace(&mut self) {
        let tab_size = self.tab_size;
        let mut items = Vec::with_capacity(self.selections.len());
        for selection in &self.selections {
            let range = selection.range();
            if !range.is_empty() {
                items.push((Some((range, String::new())), in_edit(0, 0)));
                continue;
            }
            let offset = range.start;
            if offset == 0 {
                items.push((None, Target::Exact { anchor: 0, head: 0 }));
                continue;
            }
            let line = self.buffer.line_of(offset);
            let column = offset - self.buffer.line_start(line);
            let text = self.buffer.line(line);
            if column > 0 && text[..column].bytes().all(|byte| byte == b' ') {
                let remove = match column % tab_size {
                    0 => tab_size,
                    rest => rest,
                }
                .min(column);
                items.push((
                    Some((offset - remove..offset, String::new())),
                    in_edit(0, 0),
                ));
                continue;
            }
            let start = self.buffer.previous_boundary(offset);
            items.push((Some((start..offset, String::new())), in_edit(0, 0)));
        }
        self.apply(items, EditKind::Delete);
    }

    pub fn delete_to(&mut self, to: impl Fn(&Buffer, usize) -> usize) {
        let mut items = Vec::with_capacity(self.selections.len());
        for selection in &self.selections {
            let range = if selection.is_empty() {
                let other = to(&self.buffer, selection.head);
                selection.head.min(other)..selection.head.max(other)
            } else {
                selection.range()
            };
            if range.is_empty() {
                items.push((
                    None,
                    Target::Exact {
                        anchor: range.start,
                        head: range.start,
                    },
                ));
            } else {
                items.push((Some((range, String::new())), in_edit(0, 0)));
            }
        }
        self.apply(items, EditKind::Delete);
    }

    pub fn line_blocks(&self) -> Vec<(usize, usize)> {
        let mut blocks: Vec<(usize, usize)> = Vec::new();
        for selection in &self.selections {
            let first = self.buffer.line_of(selection.start());
            let mut last = self.buffer.line_of(selection.end());
            if last > first && self.buffer.line_start(last) == selection.end() {
                last -= 1;
            }
            match blocks.last_mut() {
                Some(block) if first <= block.1 + 1 => block.1 = block.1.max(last),
                _ => blocks.push((first, last)),
            }
        }
        blocks
    }

    pub fn delete_lines(&mut self) {
        let line_count = self.buffer.line_count();
        let mut edits: Vec<Edit> = Vec::new();
        let mut blocks = Vec::new();
        for (first, last) in self.line_blocks() {
            let range = if last + 1 < line_count {
                self.buffer.line_start(first)..self.buffer.line_start(last + 1)
            } else if first > 0 {
                self.buffer.line_end(first - 1)..self.buffer.len()
            } else {
                0..self.buffer.len()
            };
            blocks.push((first, last, range.start));
            edits.push((range, String::new()));
        }
        let targets = self
            .selections
            .iter()
            .map(|selection| {
                let line = self.buffer.line_of(selection.head);
                let at = blocks
                    .iter()
                    .find(|(first, last, _)| (*first..=*last).contains(&line))
                    .map(|block| block.2)
                    .unwrap_or(selection.head);
                Target::InEdit {
                    at,
                    anchor: 0,
                    head: 0,
                }
            })
            .collect();
        self.apply_edits(edits, targets, EditKind::Other);
        let selections = self
            .selections
            .iter()
            .map(|selection| {
                let line = self.buffer.line_of(selection.head);
                Selection::cursor(
                    selection.id,
                    self.buffer.line_start(line) + self.buffer.indent_len(line),
                )
            })
            .collect();
        self.set_selections(selections);
    }

    pub fn move_lines(&mut self, up: bool) {
        let line_count = self.buffer.line_count();
        let blocks = self.line_blocks();
        if (up && blocks.first().is_some_and(|block| block.0 == 0))
            || (!up && blocks.last().is_some_and(|block| block.1 + 1 >= line_count))
        {
            return;
        }
        let mut edits: Vec<Edit> = Vec::new();
        let mut shifts: Vec<(Range<usize>, isize)> = Vec::new();
        for (first, last) in blocks {
            let block_range = self.buffer.line_start(first)..self.buffer.line_end(last);
            let block = self.buffer.slice(block_range.clone());
            if up {
                let other = self.buffer.line(first - 1).into_owned();
                let region = self.buffer.line_start(first - 1)..block_range.end;
                edits.push((region, format!("{block}\n{other}")));
                shifts.push((
                    block_range.start..block_range.end + 1,
                    -(other.len() as isize + 1),
                ));
            } else {
                let other = self.buffer.line(last + 1).into_owned();
                let region = block_range.start..self.buffer.line_end(last + 1);
                edits.push((region, format!("{other}\n{block}")));
                shifts.push((
                    block_range.start..block_range.end + 1,
                    other.len() as isize + 1,
                ));
            }
        }
        let shift = |offset: usize| {
            shifts
                .iter()
                .find(|(range, _)| range.contains(&offset))
                .map(|(_, delta)| (offset as isize + delta) as usize)
                .unwrap_or(offset)
        };
        let targets: Vec<Target> = self
            .selections
            .iter()
            .map(|selection| Target::Exact {
                anchor: shift(selection.anchor),
                head: shift(selection.head),
            })
            .collect();
        self.apply_edits(edits, targets, EditKind::Other);
    }

    pub fn copy_lines(&mut self, down: bool) {
        let blocks = self.line_blocks();
        let mut edits: Vec<Edit> = Vec::new();
        let mut inserted: Vec<(usize, usize, usize)> = Vec::new();
        for (first, last) in blocks {
            let start = self.buffer.line_start(first);
            let block = self.buffer.slice(start..self.buffer.line_end(last));
            let text = format!("{block}\n");
            inserted.push((start, self.buffer.line_end(last), text.len()));
            edits.push((start..start, text));
        }
        let place = |offset: usize| {
            let mut delta = 0;
            for (start, end, len) in &inserted {
                if offset >= *start && offset <= *end {
                    return offset + delta + if down { *len } else { 0 };
                }
                if offset > *end {
                    delta += len;
                }
            }
            offset + delta
        };
        let targets = self
            .selections
            .iter()
            .map(|selection| Target::Exact {
                anchor: place(selection.anchor),
                head: place(selection.head),
            })
            .collect();
        self.apply_edits(edits, targets, EditKind::Other);
    }

    pub fn indent_lines(&mut self, outdent: bool) {
        let unit = self.indent_unit();
        let mut edits: Vec<Edit> = Vec::new();
        for (first, last) in self.line_blocks() {
            for line in first..=last {
                let start = self.buffer.line_start(line);
                if outdent {
                    let text = self.buffer.line(line);
                    let remove = if text.starts_with('\t') {
                        1
                    } else {
                        text.bytes()
                            .take(self.tab_size)
                            .take_while(|byte| *byte == b' ')
                            .count()
                    };
                    if remove > 0 {
                        edits.push((start..start + remove, String::new()));
                    }
                } else if !self.buffer.is_blank_line(line) {
                    edits.push((start..start, unit.clone()));
                }
            }
        }
        let targets = self.mapped_targets();
        self.apply_edits(edits, targets, EditKind::Other);
    }

    fn mapped_targets(&self) -> Vec<Target> {
        self.selections
            .iter()
            .map(|selection| Target::Mapped {
                anchor: selection.anchor,
                head: selection.head,
            })
            .collect()
    }

    pub fn tab(&mut self) {
        let multi_line = self.selections.iter().any(|selection| {
            self.buffer.line_of(selection.start()) != self.buffer.line_of(selection.end())
        });
        if multi_line {
            self.indent_lines(false);
            return;
        }
        let tab_size = self.tab_size;
        let hard_tabs = self.hard_tabs;
        let items = self
            .selections
            .iter()
            .map(|selection| {
                let text = if hard_tabs {
                    "\t".to_string()
                } else {
                    let line = self.buffer.line_of(selection.start());
                    let start = self.buffer.line_start(line);
                    let column = display_column(
                        &self.buffer.line(line),
                        selection.start() - start,
                        tab_size,
                    );
                    " ".repeat(tab_size - column % tab_size)
                };
                let len = text.len();
                (Some((selection.range(), text)), in_edit(len, len))
            })
            .collect();
        self.apply(items, EditKind::Insert);
    }

    pub fn undo(&mut self) -> bool {
        let Some(step) = self.history.undo() else {
            return false;
        };
        self.replay(step.edits, step.selections);
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(step) = self.history.redo() else {
            return false;
        };
        self.replay(step.edits, step.selections);
        true
    }

    fn replay(&mut self, batches: Vec<Vec<Edit>>, selections: Vec<Selection>) {
        for edits in batches {
            self.buffer.apply(&edits);
        }
        self.set_selections(selections);
    }

    pub fn move_heads(&mut self, select: bool, motion: impl Fn(&Buffer, &Selection) -> usize) {
        let selections = self
            .selections
            .iter()
            .map(|selection| {
                let head = motion(&self.buffer, selection);
                let mut selection = selection.clone();
                if select {
                    selection.select_to(head)
                } else {
                    selection.collapse_to(head)
                }
                selection
            })
            .collect();
        self.set_selections(selections);
    }

    pub fn select_next_occurrence(&mut self) -> bool {
        let newest = self.newest_selection().clone();
        if newest.is_empty() {
            if self.buffer.word_range_at(newest.head).is_none() {
                return false;
            }
            let selections = self
                .selections
                .iter()
                .map(|selection| {
                    let mut selection = selection.clone();
                    if let Some(range) = self
                        .buffer
                        .word_range_at(selection.head)
                        .filter(|_| selection.is_empty())
                    {
                        selection.set_range(range, false);
                    }
                    selection
                })
                .collect();
            self.set_selections(selections);
            return true;
        }
        let needle = self.buffer.slice(newest.range());
        let text = self.buffer.text();
        let occurrences = occurrences(&text, &needle);
        let taken = |range: &Range<usize>| {
            self.selections
                .iter()
                .any(|selection| selection.range() == *range)
        };
        let next = occurrences
            .iter()
            .find(|range| range.start >= newest.end() && !taken(range))
            .or_else(|| occurrences.iter().find(|range| !taken(range)))
            .cloned();
        let Some(range) = next else { return false };
        let mut selections = self.selections.clone();
        let mut selection = Selection::cursor(self.new_selection_id(), 0);
        selection.set_range(range, false);
        selections.push(selection);
        self.set_selections(selections);
        true
    }

    pub fn toggle_caret(&mut self, offset: usize) {
        let existing = self
            .selections
            .iter()
            .position(|selection| selection.is_empty() && selection.head == offset);
        let mut selections = self.selections.clone();
        match existing {
            Some(index) if selections.len() > 1 => {
                selections.remove(index);
            }
            _ => {
                let id = self.new_selection_id();
                selections.push(Selection::cursor(id, offset));
            }
        }
        self.set_selections(selections);
    }

    pub fn remove_secondary_cursors(&mut self) {
        let newest = self.newest_selection().clone();
        self.set_selections(vec![newest]);
    }

    pub fn copy_text(&self) -> ClipboardText {
        if self.selections.iter().all(Selection::is_empty) {
            let mut lines: Vec<usize> = self
                .selections
                .iter()
                .map(|selection| self.buffer.line_of(selection.head))
                .collect();
            lines.dedup();
            let pieces: Vec<String> = lines
                .iter()
                .map(|line| format!("{}\n", self.buffer.line(*line)))
                .collect();
            return ClipboardText {
                text: pieces.concat(),
                whole_lines: true,
                pieces,
            };
        }
        let pieces: Vec<String> = self
            .selections
            .iter()
            .filter(|selection| !selection.is_empty())
            .map(|selection| self.buffer.slice(selection.range()))
            .collect();
        ClipboardText {
            text: pieces.join("\n"),
            whole_lines: false,
            pieces,
        }
    }

    pub fn cut(&mut self) -> ClipboardText {
        let copied = self.copy_text();
        if copied.whole_lines {
            self.delete_lines();
        } else {
            self.delete_to(|_, offset| offset);
        }
        copied
    }

    pub fn paste(&mut self, clipboard: &ClipboardText) {
        let count = self.selections.len();
        if count > 1 && clipboard.pieces.len() == count {
            let items = self
                .selections
                .iter()
                .zip(&clipboard.pieces)
                .map(|(selection, piece)| {
                    let piece = piece.trim_end_matches('\n').to_string();
                    let len = piece.len();
                    (Some((selection.range(), piece)), in_edit(len, len))
                })
                .collect();
            self.apply(items, EditKind::Other);
            return;
        }
        if clipboard.whole_lines && self.selections.iter().all(Selection::is_empty) {
            let items = self
                .selections
                .iter()
                .map(|selection| {
                    let start = self.buffer.line_start(self.buffer.line_of(selection.head));
                    (
                        Some((start..start, clipboard.text.clone())),
                        Target::Mapped {
                            anchor: selection.head,
                            head: selection.head,
                        },
                    )
                })
                .collect();
            self.apply(items, EditKind::Other);
            return;
        }
        self.insert_plain(&clipboard.text, EditKind::Other);
    }
}

fn occurrences(text: &str, needle: &str) -> Vec<Range<usize>> {
    if needle.is_empty() {
        return Vec::new();
    }
    text.match_indices(needle)
        .map(|(index, _)| index..index + needle.len())
        .collect()
}

fn normalize_crlf(edits: Vec<Edit>, targets: Vec<Target>) -> (Vec<Edit>, Vec<Target>) {
    if !edits.iter().any(|(_, text)| text.contains('\r')) {
        return (edits, targets);
    }
    let shrink = |text: &str, offset: usize| -> usize {
        let bytes = text.as_bytes();
        let dropped = (0..offset.min(bytes.len()))
            .filter(|&i| bytes[i] == b'\r' && bytes.get(i + 1) == Some(&b'\n'))
            .count();
        offset - dropped
    };
    let targets = targets
        .into_iter()
        .map(|target| match target {
            Target::InEdit { at, anchor, head } => {
                match edits.iter().find(|(range, _)| range.start == at) {
                    Some((_, text)) => Target::InEdit {
                        at,
                        anchor: shrink(text, anchor),
                        head: shrink(text, head),
                    },
                    None => Target::InEdit { at, anchor, head },
                }
            }
            other => other,
        })
        .collect();
    let edits = edits
        .into_iter()
        .map(|(range, text)| {
            let text = if text.contains('\r') {
                text.replace("\r\n", "\n").replace('\r', "\n")
            } else {
                text
            };
            (range, text)
        })
        .collect();
    (edits, targets)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(text: &str) -> Document {
        Document::new(text)
    }

    fn carets(document: &mut Document, offsets: &[usize]) {
        let selections = offsets
            .iter()
            .enumerate()
            .map(|(id, offset)| Selection::cursor(id + 10, *offset))
            .collect();
        document.set_selections(selections);
    }

    fn heads(document: &Document) -> Vec<usize> {
        document
            .selections
            .iter()
            .map(|selection| selection.head)
            .collect()
    }

    #[test]
    fn crlf_text_inserts_as_lf_and_undoes_cleanly() {
        let mut document = doc("XY");
        carets(&mut document, &[1]);
        document.insert("a\r\nb");
        assert_eq!(document.buffer.text(), "Xa\nbY");
        assert_eq!(heads(&document), vec![4]);
        document.undo();
        assert_eq!(document.buffer.text(), "XY");
    }

    #[test]
    fn typing_at_many_carets() {
        let mut document = doc("ab\ncd");
        carets(&mut document, &[1, 4]);
        document.insert("x");
        assert_eq!(document.buffer.text(), "axb\ncxd");
        assert_eq!(heads(&document), vec![2, 6]);
        document.undo();
        assert_eq!(document.buffer.text(), "ab\ncd");
        assert_eq!(heads(&document), vec![1, 4]);
    }

    #[test]
    fn typing_groups_into_one_undo_step() {
        let mut document = doc("");
        document.insert("a");
        document.insert("b");
        document.insert("c");
        assert_eq!(document.buffer.text(), "abc");
        assert!(document.undo());
        assert_eq!(document.buffer.text(), "");
        assert!(document.redo());
        assert_eq!(document.buffer.text(), "abc");
        assert_eq!(heads(&document), vec![3]);
    }

    #[test]
    fn backspace_uses_indent_stops_and_graphemes() {
        let mut document = doc("      x");
        document.set_cursor(6);
        document.backspace();
        assert_eq!(document.buffer.text(), "    x");
        let mut document = doc("e\u{301}x");
        document.set_cursor(3);
        document.backspace();
        assert_eq!(document.buffer.text(), "x");
    }

    #[test]
    fn enter_keeps_the_indent() {
        let mut document = doc("  x");
        document.set_cursor(3);
        document.newline();
        assert_eq!(document.buffer.text(), "  x\n  ");
        assert_eq!(heads(&document), vec![6]);
    }

    #[test]
    fn line_moves_and_copies() {
        let mut document = doc("one\ntwo\nthree");
        document.set_cursor(5);
        document.move_lines(true);
        assert_eq!(document.buffer.text(), "two\none\nthree");
        assert_eq!(heads(&document), vec![1]);
        document.move_lines(false);
        document.move_lines(false);
        assert_eq!(document.buffer.text(), "one\nthree\ntwo");
        assert_eq!(heads(&document), vec![11]);
        document.copy_lines(true);
        assert_eq!(document.buffer.text(), "one\nthree\ntwo\ntwo");
        assert_eq!(heads(&document), vec![15]);
        document.delete_lines();
        assert_eq!(document.buffer.text(), "one\nthree\ntwo");
    }

    #[test]
    fn indent_and_outdent() {
        let mut document = doc("a\nb");
        document.select_range(0..3);
        document.tab();
        assert_eq!(document.buffer.text(), "    a\n    b");
        document.indent_lines(true);
        assert_eq!(document.buffer.text(), "a\nb");
        document.set_cursor(1);
        document.tab();
        assert_eq!(document.buffer.text(), "a   \nb");
    }

    #[test]
    fn occurrences_and_whole_line_clipboard() {
        let mut document = doc("foo bar foo foobar");
        document.set_cursor(1);
        document.select_next_occurrence();
        document.select_next_occurrence();
        let ranges: Vec<_> = document.selections.iter().map(Selection::range).collect();
        assert_eq!(ranges, vec![0..3, 8..11]);
        document.remove_secondary_cursors();
        assert_eq!(document.selections.len(), 1);

        let mut document = doc("one\ntwo");
        document.set_cursor(5);
        let copied = document.copy_text();
        assert!(copied.whole_lines);
        assert_eq!(copied.text, "two\n");
        document.set_cursor(1);
        document.paste(&copied);
        assert_eq!(document.buffer.text(), "two\none\ntwo");
        assert_eq!(heads(&document), vec![5]);
    }

    #[test]
    fn paste_spreads_pieces_over_carets() {
        let mut document = doc("a\nb");
        carets(&mut document, &[1, 3]);
        let clipboard = ClipboardText {
            text: "x\ny".into(),
            whole_lines: false,
            pieces: vec!["x".into(), "y".into()],
        };
        document.paste(&clipboard);
        assert_eq!(document.buffer.text(), "ax\nby");
    }

    #[test]
    fn toggle_caret_adds_and_removes() {
        let mut document = doc("abc");
        document.set_cursor(0);
        document.toggle_caret(2);
        assert_eq!(heads(&document), vec![0, 2]);
        document.toggle_caret(2);
        assert_eq!(heads(&document), vec![0]);
        document.toggle_caret(0);
        assert_eq!(heads(&document), vec![0]);
    }

    #[test]
    fn cut_removes_the_selection() {
        let mut document = doc("hello world");
        document.select_range(0..6);
        let cut = document.cut();
        assert_eq!(cut.text, "hello ");
        assert_eq!(document.buffer.text(), "world");
    }
}
