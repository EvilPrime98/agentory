//! Multiline text editor, the counterpart of `TextInput` for whole files. Typing and newlines,
//! caret and selection (mouse click/drag and shift+arrows), up/down/home/end, copy/cut/paste,
//! vertical scroll, Tab inserts spaces, undo/redo (consecutive typing is one step).
//! Lines soft-wrap at the editor width, hard-breaking at the column (not at word boundaries).
//! Known gaps: no IME composition, caret is
//! char-based (not grapheme-based), and mouse placement assumes the monospace app font.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    App, ClipboardItem, Context, EventEmitter, FocusHandle, Focusable, InteractiveElement,
    IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, ParentElement, Pixels,
    Point, Render, ScrollHandle, StatefulInteractiveElement, Styled, Window, div, px,
};

use crate::ui::theme;

const TEXT_SIZE: f32 = 13.0;
const LINE_HEIGHT: f32 = 19.0;
const PAD: f32 = 10.0;
/// Undo steps kept.
const HISTORY: usize = 200;
const TAB: &str = "    ";

pub enum TextAreaEvent {
    Changed,
    /// Ctrl+S.
    Save,
    /// Ctrl+Enter.
    Submit,
}

pub struct TextArea {
    /// Text split by `\n`; always holds at least one line.
    lines: Vec<String>,
    /// Caret as (line, column in chars).
    caret: (usize, usize),
    /// Other end of the selection; `None` or equal to `caret` means no selection.
    anchor: Option<(usize, usize)>,
    /// Column kept while moving up/down across shorter lines.
    goal_col: Option<usize>,
    dragging: bool,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    /// The last undo step was plain typing, so the next typed character joins it.
    typing: bool,
    focus: FocusHandle,
    scroll: ScrollHandle,
    /// Window position of the text origin and the glyph advance, written by `render`.
    metrics: Rc<Cell<Metrics>>,
}

#[derive(Clone)]
struct Snapshot {
    lines: Vec<String>,
    caret: (usize, usize),
}

#[derive(Clone, Copy, Default)]
struct Metrics {
    origin: Point<Pixels>,
    char_w: f32,
    /// Columns that fit in one visual row; 0 until the first layout (no wrapping).
    cols: usize,
}

impl EventEmitter<TextAreaEvent> for TextArea {}

impl TextArea {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            lines: vec![String::new()],
            caret: (0, 0),
            anchor: None,
            goal_col: None,
            dragging: false,
            undo: Vec::new(),
            redo: Vec::new(),
            typing: false,
            focus: cx.focus_handle(),
            scroll: ScrollHandle::new(),
            metrics: Rc::default(),
        }
    }

    pub fn value(&self) -> String {
        self.lines.join("\n")
    }

    /// Replace the text (no `Changed` event: callers use this to load a file).
    pub fn set_value(&mut self, value: &str, cx: &mut Context<Self>) {
        self.lines = split_lines(value);
        self.caret = (0, 0);
        self.anchor = None;
        self.goal_col = None;
        self.undo.clear();
        self.redo.clear();
        self.typing = false;
        cx.notify();
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            lines: self.lines.clone(),
            caret: self.caret,
        }
    }

    fn restore(&mut self, snap: Snapshot) {
        self.lines = snap.lines;
        self.caret = snap.caret;
        self.anchor = None;
        self.goal_col = None;
        self.typing = false;
    }

    fn undo(&mut self) -> bool {
        let Some(prev) = self.undo.pop() else {
            return false;
        };
        self.redo.push(self.snapshot());
        self.restore(prev);
        true
    }

    fn redo(&mut self) -> bool {
        let Some(next) = self.redo.pop() else {
            return false;
        };
        self.undo.push(self.snapshot());
        self.restore(next);
        true
    }

    fn line_len(&self, line: usize) -> usize {
        self.lines[line].chars().count()
    }

    fn byte_index(&self, line: usize, col: usize) -> usize {
        let s = &self.lines[line];
        s.char_indices().nth(col).map(|(i, _)| i).unwrap_or(s.len())
    }

    /// Selection ordered start..end.
    fn selection(&self) -> Option<((usize, usize), (usize, usize))> {
        let anchor = self.anchor?;
        (anchor != self.caret).then(|| (anchor.min(self.caret), anchor.max(self.caret)))
    }

    fn selected_text(&self) -> Option<String> {
        let (from, to) = self.selection()?;
        if from.0 == to.0 {
            let l = &self.lines[from.0];
            return Some(l[self.byte_index(from.0, from.1)..self.byte_index(to.0, to.1)].to_string());
        }
        let mut out = self.lines[from.0][self.byte_index(from.0, from.1)..].to_string();
        for l in &self.lines[from.0 + 1..to.0] {
            out.push('\n');
            out.push_str(l);
        }
        out.push('\n');
        out.push_str(&self.lines[to.0][..self.byte_index(to.0, to.1)]);
        Some(out)
    }

    fn delete_selection(&mut self) -> bool {
        let Some((from, to)) = self.selection() else {
            return false;
        };
        let tail = self.lines[to.0][self.byte_index(to.0, to.1)..].to_string();
        let head = self.byte_index(from.0, from.1);
        self.lines[from.0].truncate(head);
        self.lines[from.0].push_str(&tail);
        self.lines.drain(from.0 + 1..=to.0);
        self.caret = from;
        self.anchor = None;
        true
    }

    fn insert(&mut self, text: &str) {
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        let clean: String = text
            .chars()
            .map(|c| if c == '\t' { ' ' } else { c })
            .filter(|c| *c == '\n' || !c.is_control())
            .collect();
        if clean.is_empty() {
            return;
        }
        self.delete_selection();
        let (line, col) = self.caret;
        let at = self.byte_index(line, col);
        let tail = self.lines[line].split_off(at);
        let mut parts = clean.split('\n');
        self.lines[line].push_str(parts.next().unwrap_or(""));
        let mut cur = line;
        for part in parts {
            cur += 1;
            self.lines.insert(cur, part.to_string());
        }
        let col = self.line_len(cur);
        self.lines[cur].push_str(&tail);
        self.caret = (cur, col);
        self.goal_col = None;
    }

    fn move_caret(&mut self, to: (usize, usize), extend: bool) {
        if extend {
            self.anchor.get_or_insert(self.caret);
        } else {
            self.anchor = None;
        }
        self.caret = to;
    }

    fn prev(&self, (line, col): (usize, usize)) -> (usize, usize) {
        if col > 0 {
            (line, col - 1)
        } else if line > 0 {
            (line - 1, self.line_len(line - 1))
        } else {
            (0, 0)
        }
    }

    fn next(&self, (line, col): (usize, usize)) -> (usize, usize) {
        if col < self.line_len(line) {
            (line, col + 1)
        } else if line + 1 < self.lines.len() {
            (line + 1, 0)
        } else {
            (line, col)
        }
    }

    /// Columns per visual row.
    fn cols(&self) -> usize {
        match self.metrics.get().cols {
            0 => usize::MAX,
            c => c,
        }
    }

    /// Visual rows a line takes; a caret at the very end of a full row sits on one more row.
    fn rows_of(&self, line: usize) -> usize {
        self.line_len(line) / self.cols() + 1
    }

    /// Visual row index of a position, counted from the top of the text.
    fn row_index(&self, (line, col): (usize, usize)) -> usize {
        (0..line).map(|l| self.rows_of(l)).sum::<usize>() + col / self.cols()
    }

    /// Position at visual row `row` and column offset `x` within it (both clamped).
    fn pos_at_row(&self, row: usize, x: usize) -> (usize, usize) {
        let cols = self.cols();
        let mut left = row;
        for line in 0..self.lines.len() {
            let n = self.rows_of(line);
            if left < n {
                return (line, (left * cols).saturating_add(x).min(self.line_len(line)).min(
                    (left * cols).saturating_add(cols - 1).max(left * cols),
                ));
            }
            left -= n;
        }
        let last = self.lines.len() - 1;
        (last, self.line_len(last))
    }

    /// Keep the caret row inside the viewport.
    fn reveal_caret(&self) {
        let view = self.scroll.bounds().size.height;
        if view <= px(0.0) {
            return;
        }
        let top = px(PAD + self.row_index(self.caret) as f32 * LINE_HEIGHT);
        let bottom = top + px(LINE_HEIGHT + PAD);
        let mut offset = self.scroll.offset();
        if top + offset.y < px(0.0) {
            offset.y = -top;
        } else if bottom + offset.y > view {
            offset.y = view - bottom;
        }
        self.scroll.set_offset(offset);
    }

    fn position_at(&self, pos: Point<Pixels>) -> (usize, usize) {
        let m = self.metrics.get();
        let y = f32::from(pos.y - m.origin.y);
        let row = (y / LINE_HEIGHT).floor().max(0.0) as usize;
        let x = f32::from(pos.x - m.origin.x);
        let col = if m.char_w > 0.0 {
            (x / m.char_w).round().max(0.0) as usize
        } else {
            0
        };
        self.pos_at_row(row, col)
    }

    fn on_mouse_down(&mut self, e: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        let to = self.position_at(e.position);
        self.move_caret(to, e.modifiers.shift);
        self.goal_col = None;
        self.dragging = true;
        cx.notify();
    }

    fn on_mouse_move(&mut self, e: &MouseMoveEvent, cx: &mut Context<Self>) {
        if !self.dragging || !e.dragging() {
            self.dragging = false;
            return;
        }
        let to = self.position_at(e.position);
        self.move_caret(to, true);
        self.reveal_caret();
        cx.notify();
    }

    fn on_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let ks = &event.keystroke;
        let command = ks.modifiers.control || ks.modifiers.platform;
        let shift = ks.modifiers.shift;
        let (line, col) = self.caret;
        let mut vertical = false;
        let before = self.snapshot();
        let typed = !command && !ks.modifiers.alt && ks.key_char.is_some() && ks.key != "enter";
        match ks.key.as_str() {
            "z" if command && !shift => {
                if self.undo() {
                    cx.emit(TextAreaEvent::Changed);
                }
            }
            "y" | "z" if command => {
                if self.redo() {
                    cx.emit(TextAreaEvent::Changed);
                }
            }
            "s" if command => cx.emit(TextAreaEvent::Save),
            "enter" if command => cx.emit(TextAreaEvent::Submit),
            "enter" => {
                self.insert("\n");
                cx.emit(TextAreaEvent::Changed);
            }
            "tab" => {
                self.insert(TAB);
                cx.emit(TextAreaEvent::Changed);
            }
            "backspace" => {
                if !self.delete_selection() {
                    let from = self.prev(self.caret);
                    if from == self.caret {
                        return cx.stop_propagation();
                    }
                    self.anchor = Some(from);
                    self.delete_selection();
                }
                cx.emit(TextAreaEvent::Changed);
            }
            "delete" => {
                if !self.delete_selection() {
                    let to = self.next(self.caret);
                    if to == self.caret {
                        return cx.stop_propagation();
                    }
                    self.anchor = Some(to);
                    self.delete_selection();
                }
                cx.emit(TextAreaEvent::Changed);
            }
            "left" => match self.selection() {
                Some((from, _)) if !shift => self.move_caret(from, false),
                _ => self.move_caret(self.prev(self.caret), shift),
            },
            "right" => match self.selection() {
                Some((_, to)) if !shift => self.move_caret(to, false),
                _ => self.move_caret(self.next(self.caret), shift),
            },
            "up" | "down" => {
                vertical = true;
                let cols = self.cols();
                let goal = *self.goal_col.get_or_insert(col % cols);
                let row = self.row_index(self.caret);
                let end = (
                    self.lines.len() - 1,
                    self.line_len(self.lines.len() - 1),
                );
                let last_row = self.row_index(end);
                let to = if ks.key == "up" {
                    if row == 0 {
                        (0, 0)
                    } else {
                        self.pos_at_row(row - 1, goal)
                    }
                } else if row >= last_row {
                    end
                } else {
                    self.pos_at_row(row + 1, goal)
                };
                self.move_caret(to, shift);
            }
            "home" => {
                let to = if command { (0, 0) } else { (line, 0) };
                self.move_caret(to, shift);
            }
            "end" => {
                let to = if command {
                    let last = self.lines.len() - 1;
                    (last, self.line_len(last))
                } else {
                    (line, self.line_len(line))
                };
                self.move_caret(to, shift);
            }
            "a" if command => {
                self.anchor = Some((0, 0));
                let last = self.lines.len() - 1;
                self.caret = (last, self.line_len(last));
            }
            "c" if command => {
                if let Some(text) = self.selected_text() {
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                }
            }
            "x" if command => {
                if let Some(text) = self.selected_text() {
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                    self.delete_selection();
                    cx.emit(TextAreaEvent::Changed);
                }
            }
            "v" if command => {
                if let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) {
                    self.insert(&text);
                    cx.emit(TextAreaEvent::Changed);
                }
            }
            "escape" => return,
            _ if command || ks.modifiers.alt => return,
            _ => match &ks.key_char {
                Some(ch) => {
                    self.insert(ch);
                    cx.emit(TextAreaEvent::Changed);
                }
                None => return,
            },
        }
        if self.lines != before.lines && !matches!(ks.key.as_str(), "z" | "y") {
            // Typing joins the previous step; everything else is a step of its own.
            if !(typed && self.typing) {
                self.undo.push(before);
                if self.undo.len() > HISTORY {
                    self.undo.remove(0);
                }
            }
            self.redo.clear();
            self.typing = typed;
        } else if !matches!(ks.key.as_str(), "z" | "y") {
            self.typing = self.typing && typed;
        }
        if !vertical {
            self.goal_col = None;
        }
        self.reveal_caret();
        cx.stop_propagation();
        cx.notify();
    }

    /// One visual row (`k`-th chunk of line `i`): text with the selected part highlighted. The
    /// caret is an absolutely positioned bar so it never takes width and the glyphs stay put.
    fn render_row(&self, i: usize, k: usize, focused: bool, char_w: f32) -> gpui::Div {
        let text = &self.lines[i];
        let len = self.line_len(i);
        let cols = self.cols();
        let start = k.saturating_mul(cols).min(len);
        let end = start.saturating_add(cols).min(len);
        let last = k + 1 == self.rows_of(i);
        let (sel_from, sel_to) = match self.selection() {
            Some((from, to)) if from.0 <= i && i <= to.0 => (
                if from.0 == i { from.1 } else { 0 },
                // A selection running past the line end also highlights the newline cell.
                if to.0 == i { to.1 } else { len + 1 },
            ),
            _ => (0, 0),
        };
        let has_sel = sel_to > sel_from;
        let caret_col = (self.caret.0 == i && focused && self.caret.1 / cols == k)
            .then(|| self.caret.1 - k * cols);

        // Split at the selection edges and style each piece.
        let mut cuts = vec![start, end];
        if has_sel {
            cuts.extend([sel_from, sel_to].map(|c| c.clamp(start, end)));
        }
        cuts.sort_unstable();
        cuts.dedup();

        let byte = |col: usize| {
            text.char_indices()
                .nth(col)
                .map(|(b, _)| b)
                .unwrap_or(text.len())
        };
        let mut row = div()
            .relative()
            .flex()
            .flex_none()
            .h(px(LINE_HEIGHT))
            .items_center();
        for w in cuts.windows(2) {
            let (a, b) = (w[0], w[1]);
            if a == b {
                continue;
            }
            let piece = div().flex_none().child(text[byte(a)..byte(b)].to_string());
            row = row.child(if has_sel && a >= sel_from && b <= sel_to {
                piece.bg(theme::accent_soft())
            } else {
                piece
            });
        }
        if has_sel && sel_to > len && last {
            // Newline cell, so a multi-line selection reads as continuous.
            row = row.child(div().flex_none().w(px(6.0)).h_full().bg(theme::accent_soft()));
        }
        if let Some(col) = caret_col {
            row = row.child(
                div()
                    .absolute()
                    .left(px(col as f32 * char_w))
                    .top(px(1.5))
                    .w(px(1.0))
                    .h(px(LINE_HEIGHT - 3.0))
                    .bg(theme::accent()),
            );
        }
        row
    }
}

fn split_lines(text: &str) -> Vec<String> {
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    text.split('\n').map(str::to_string).collect()
}

impl Focusable for TextArea {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for TextArea {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.focus.is_focused(window);
        let font_size = px(TEXT_SIZE);
        let font = window.text_style().font();
        let ts = window.text_system();
        let char_w = ts
            .advance(ts.resolve_font(&font), font_size, 'm')
            .map(|s| f32::from(s.width))
            .unwrap_or(0.0);

        // Text origin = viewport origin + padding + current scroll offset.
        let bounds = self.scroll.bounds();
        let mut m = self.metrics.get();
        m.char_w = char_w;
        m.origin = Point {
            x: bounds.origin.x + px(PAD),
            y: bounds.origin.y + px(PAD) + self.scroll.offset().y,
        };
        self.metrics.set(m);

        // Wrap width: the viewport minus padding. Changing it re-renders on the next frame.
        let cols = if char_w > 0.0 {
            (f32::from(bounds.size.width - px(2.0 * PAD)) / char_w).floor() as usize
        } else {
            0
        };
        if cols > 0 && cols != m.cols {
            m.cols = cols;
            self.metrics.set(m);
            cx.notify();
        }

        let lines: Vec<_> = (0..self.lines.len())
            .flat_map(|i| (0..self.rows_of(i)).map(move |k| (i, k)))
            .map(|(i, k)| self.render_row(i, k, focused, char_w))
            .collect();

        div()
            .id("text-area")
            .key_context("TextArea")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, e, _, cx| this.on_key(e, cx)))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, e, w, cx| this.on_mouse_down(e, w, cx)))
            .on_mouse_move(cx.listener(|this, e, _, cx| this.on_mouse_move(e, cx)))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.dragging = false),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.dragging = false),
            )
            .flex()
            .flex_col()
            .size_full()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .p(px(PAD))
            .text_size(font_size)
            .text_color(theme::text())
            .cursor_text()
            .children(lines)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_crlf() {
        assert_eq!(split_lines("a\r\nb\rc"), vec!["a", "b", "c"]);
        assert_eq!(split_lines(""), vec![""]);
    }
}
