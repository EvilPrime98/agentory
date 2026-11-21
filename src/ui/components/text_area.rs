
use std::cell::{Cell, RefCell};
use std::ops::Range;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    App, AvailableSpace, Bounds, ClipboardItem, Context, Element, ElementId, ElementInputHandler,
    Entity, EntityInputHandler, EventEmitter, FocusHandle, Focusable, GlobalElementId,
    InspectorElementId, InteractiveElement, IntoElement, KeyDownEvent, LayoutId, MouseButton,
    MouseDownEvent, MouseMoveEvent, PaintQuad, ParentElement, Pixels, Point, Render, SharedString,
    StatefulInteractiveElement, Style, Styled, TextAlign, TextRun, UTF16Selection, Window, div,
    fill, point, px, relative, size,
};

use crate::ui::components::scrollbar::Scrollbar;
use crate::ui::editor::buffer::{Buffer, CharClass, char_class};
use crate::ui::editor::document::{ClipboardText, Document};
use crate::ui::editor::history::EditKind;
use crate::ui::editor::layout::{Params, WrapLayout};
use crate::ui::editor::selection::Selection;
use crate::ui::theme;

const TEXT_SIZE: f32 = 13.0;
const LINE_HEIGHT: f32 = 19.0;
const PAD: f32 = 10.0;
const BLINK_HALF_PERIOD: Duration = Duration::from_millis(530);

pub enum TextAreaEvent {
    Changed,
    /// Ctrl+S.
    Save,
    /// Ctrl+Enter.
    Submit,
}

#[derive(Clone)]
enum Drag {
    Char,
    Word(Range<usize>),
    Line(Range<usize>),
}

pub struct TextArea {
    doc: Document,
    focus: FocusHandle,
    scrollbar: Scrollbar,
    layout: RefCell<Option<Rc<WrapLayout>>>,
    params: RefCell<Option<Params>>,
    bounds: Cell<Bounds<Pixels>>,
    rows: Cell<usize>,
    marked: Option<Range<usize>>,
    drag: Option<Drag>,
    clip: Option<ClipboardText>,
    submit_on_enter: bool,
    placeholder: SharedString,
    line_numbers: bool,
    last_activity: Instant,
    blink_pending: bool,
}

impl EventEmitter<TextAreaEvent> for TextArea {}

fn sanitize(text: &str) -> String {
    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .chars()
        .map(|c| if c == '\t' { ' ' } else { c })
        .filter(|c| *c == '\n' || !c.is_control())
        .collect()
}

fn prev_word(buffer: &Buffer, offset: usize) -> usize {
    let mut at = offset;
    let mut chars = buffer.reversed_chars_at(offset).peekable();
    while let Some(&c) = chars.peek() {
        if c != '\n' && c.is_whitespace() {
            at -= c.len_utf8();
            chars.next();
        } else {
            break;
        }
    }
    if let Some(&c) = chars.peek() {
        let class = char_class(c);
        if matches!(class, CharClass::Word | CharClass::Punctuation) {
            while let Some(&c) = chars.peek() {
                if char_class(c) != class {
                    break;
                }
                at -= c.len_utf8();
                chars.next();
            }
        }
    }
    if at == offset {
        buffer.previous_boundary(offset)
    } else {
        at
    }
}

fn next_word(buffer: &Buffer, offset: usize) -> usize {
    let mut at = offset;
    let mut chars = buffer.chars_at(offset).peekable();
    if let Some(&c) = chars.peek() {
        let class = char_class(c);
        if matches!(class, CharClass::Word | CharClass::Punctuation) {
            while let Some(&c) = chars.peek() {
                if char_class(c) != class {
                    break;
                }
                at += c.len_utf8();
                chars.next();
            }
        }
    }
    while let Some(&c) = chars.peek() {
        if c != '\n' && c.is_whitespace() {
            at += c.len_utf8();
            chars.next();
        } else {
            break;
        }
    }
    if at == offset {
        buffer.next_boundary(offset)
    } else {
        at
    }
}

fn line_range(buffer: &Buffer, line: usize) -> Range<usize> {
    let start = buffer.line_start(line);
    let end = if line + 1 < buffer.line_count() {
        buffer.line_start(line + 1)
    } else {
        buffer.len()
    };
    start..end
}

fn word_or_run(buffer: &Buffer, offset: usize) -> Range<usize> {
    if let Some(word) = buffer.word_range_at(offset) {
        return word;
    }
    let Some(ch) = buffer.char_at(offset).filter(|ch| *ch != '\n') else {
        return offset..offset;
    };
    if ch.is_whitespace() {
        let class = char_class(ch);
        let start = offset
            - buffer
                .reversed_chars_at(offset)
                .take_while(|other| char_class(*other) == class)
                .map(char::len_utf8)
                .sum::<usize>();
        let end = offset
            + buffer
                .chars_at(offset)
                .take_while(|other| char_class(*other) == class)
                .map(char::len_utf8)
                .sum::<usize>();
        return start..end;
    }
    offset..offset + ch.len_utf8()
}

impl TextArea {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            doc: Document::new(""),
            focus: cx.focus_handle(),
            scrollbar: Scrollbar::new(),
            layout: RefCell::new(None),
            rows: Cell::new(1),
            params: RefCell::new(None),
            bounds: Cell::new(Bounds::default()),
            marked: None,
            drag: None,
            clip: None,
            submit_on_enter: false,
            placeholder: SharedString::default(),
            line_numbers: false,
            last_activity: Instant::now(),
            blink_pending: false,
        }
    }

    pub fn submit_on_enter(mut self) -> Self {
        self.submit_on_enter = true;
        self
    }

    pub fn with_placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    pub fn with_value(mut self, value: &str) -> Self {
        self.doc.reset(value, usize::MAX);
        self
    }

    pub fn with_line_numbers(mut self) -> Self {
        self.line_numbers = true;
        self
    }

    pub fn rows(&self) -> usize {
        self.rows.get()
    }

    pub fn height_for_rows(rows: usize) -> f32 {
        LINE_HEIGHT * rows as f32 + 2.0 * PAD
    }

    pub fn text_before_caret(&self) -> String {
        self.doc.buffer.slice(0..self.doc.newest_selection().head)
    }

    pub fn replace_before_caret(&mut self, chars: usize, with: &str, cx: &mut Context<Self>) {
        let head = self.doc.newest_selection().head;
        let buffer = &self.doc.buffer;
        let mut start = head;
        for ch in buffer.reversed_chars_at(head).take(chars) {
            if ch == '\n' {
                return;
            }
            start -= ch.len_utf8();
        }
        if buffer.reversed_chars_at(head).take(chars).count() < chars {
            return;
        }
        self.doc.select_range(start..head);
        self.doc.insert_plain(&sanitize(with), EditKind::Other);
        self.marked = None;
        self.touch();
        cx.emit(TextAreaEvent::Changed);
        cx.notify();
    }

    pub fn set_text(&mut self, value: &str, cx: &mut Context<Self>) {
        self.set_value(value, cx);
        self.doc.set_cursor(usize::MAX);
    }

    pub fn caret_on_first_line(&self) -> bool {
        self.doc.buffer.line_of(self.doc.newest_selection().head) == 0
    }

    pub fn caret_on_last_line(&self) -> bool {
        self.doc.buffer.line_of(self.doc.newest_selection().head) + 1
            == self.doc.buffer.line_count()
    }

    pub fn is_empty(&self) -> bool {
        self.doc.buffer.rope().chars().all(|c| c == '\n')
    }

    pub fn value(&self) -> String {
        self.doc.buffer.text()
    }

    /// Replace the text (no `Changed` event: callers use this to load a file).
    pub fn set_value(&mut self, value: &str, cx: &mut Context<Self>) {
        self.doc.reset(value, 0);
        self.marked = None;
        self.drag = None;
        self.scrollbar.handle().set_offset(point(px(0.0), px(0.0)));
        cx.notify();
    }

    pub fn has_selection(&self) -> bool {
        self.doc.selections.iter().any(|s| !s.is_empty())
    }

    fn touch(&mut self) {
        self.last_activity = Instant::now();
    }

    fn current_params(&self) -> Option<Params> {
        self.params.borrow().clone()
    }

    fn layout_for(&self, params: Params, window: &Window) -> Rc<WrapLayout> {
        if let Some(layout) = &*self.layout.borrow()
            && layout.is_current(&self.doc.buffer, &params)
        {
            return layout.clone();
        }
        let layout = Rc::new(WrapLayout::new(&self.doc.buffer, params.clone(), window));
        *self.layout.borrow_mut() = Some(layout.clone());
        *self.params.borrow_mut() = Some(params);
        layout
    }

    fn current_layout(&self, window: &Window) -> Option<Rc<WrapLayout>> {
        Some(self.layout_for(self.current_params()?, window))
    }

    fn reveal_caret(&self, window: &Window) {
        let handle = self.scrollbar.handle();
        let view = handle.bounds().size.height;
        if view <= px(0.0) {
            return;
        }
        let Some(layout) = self.current_layout(window) else {
            return;
        };
        let head = self.doc.newest_selection().head;
        let (row, _) = layout.locate_offset(&self.doc.buffer, head);
        let top = px(PAD) + layout.line_height() * row as f32;
        let bottom = top + layout.line_height() + px(PAD);
        let mut offset = handle.offset();
        if top + offset.y < px(0.0) {
            offset.y = -top;
        } else if bottom + offset.y > view {
            offset.y = view - bottom;
        }
        handle.set_offset(offset);
    }

    fn finish(&mut self, changed: bool, window: &Window, cx: &mut Context<Self>) {
        if changed {
            cx.emit(TextAreaEvent::Changed);
        }
        self.touch();
        self.reveal_caret(window);
        cx.notify();
    }

    fn offset_at_point(&self, position: Point<Pixels>, window: &Window) -> Option<usize> {
        let layout = self.current_layout(window)?;
        let bounds = self.bounds.get();
        Some(layout.offset_for_point(
            &self.doc.buffer,
            position.x - bounds.left() - layout.gutter,
            position.y - bounds.top(),
        ))
    }

    fn in_gutter(&self, position: Point<Pixels>, window: &Window) -> bool {
        self.current_layout(window)
            .is_some_and(|l| l.gutter > px(0.0) && position.x < self.bounds.get().left() + l.gutter)
    }

    fn select_single(&mut self, range: Range<usize>, reversed: bool) {
        let id = self.doc.new_selection_id();
        let mut selection = Selection::cursor(id, 0);
        selection.set_range(range, reversed);
        self.doc.set_selections(vec![selection]);
    }

    fn update_newest(&mut self, update: impl FnOnce(&mut Selection)) {
        let mut newest = self.doc.newest_selection().clone();
        update(&mut newest);
        let mut selections: Vec<Selection> = self
            .doc
            .selections
            .iter()
            .filter(|s| s.id != newest.id)
            .cloned()
            .collect();
        selections.push(newest);
        self.doc.set_selections(selections);
    }

    fn on_mouse_down(&mut self, e: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        let Some(offset) = self.offset_at_point(e.position, window) else {
            return;
        };
        self.marked = None;
        let in_gutter = self.in_gutter(e.position, window);
        let buffer = &self.doc.buffer;
        if in_gutter || e.click_count >= 3 {
            let range = line_range(buffer, buffer.line_of(offset));
            self.select_single(range.clone(), false);
            self.drag = Some(Drag::Line(range));
        } else if e.click_count == 2 {
            let range = word_or_run(buffer, offset);
            if e.modifiers.alt {
                let id = self.doc.new_selection_id();
                let mut selection = Selection::cursor(id, 0);
                selection.set_range(range.clone(), false);
                let mut selections = self.doc.selections.clone();
                selections.push(selection);
                self.doc.set_selections(selections);
            } else {
                self.select_single(range.clone(), false);
            }
            self.drag = Some(Drag::Word(range));
        } else if e.modifiers.shift {
            self.update_newest(|s| s.select_to(offset));
            self.drag = Some(Drag::Char);
        } else if e.modifiers.alt {
            self.doc.toggle_caret(offset);
            self.drag = Some(Drag::Char);
        } else {
            self.doc.set_cursor(offset);
            self.drag = Some(Drag::Char);
        }
        self.doc.history.break_group();
        self.touch();
        cx.notify();
    }

    fn on_mouse_move(&mut self, e: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(drag) = self.drag.clone() else {
            return;
        };
        if !e.dragging() {
            self.drag = None;
            return;
        }
        let Some(offset) = self.offset_at_point(e.position, window) else {
            return;
        };
        let buffer = &self.doc.buffer;
        match drag {
            Drag::Char => self.update_newest(|s| s.select_to(offset)),
            Drag::Word(origin) => {
                let word = word_or_run(buffer, offset);
                self.update_newest(|s| {
                    if offset < origin.start {
                        s.set_range(word.start..origin.end, true);
                    } else {
                        s.set_range(origin.start..word.end.max(origin.end), false);
                    }
                });
            }
            Drag::Line(origin) => {
                let line = line_range(buffer, buffer.line_of(offset));
                self.update_newest(|s| {
                    if line.start < origin.start {
                        s.set_range(line.start..origin.end, true);
                    } else {
                        s.set_range(origin.start..line.end.max(origin.end), false);
                    }
                });
            }
        }
        self.finish(false, window, cx);
    }

    fn move_vertical(&mut self, rows: isize, select: bool, window: &Window) {
        let Some(layout) = self.current_layout(window) else {
            return;
        };
        let buffer = &self.doc.buffer;
        let selections = self
            .doc
            .selections
            .iter()
            .map(|selection| {
                let mut selection = selection.clone();
                let from = if select || selection.is_empty() {
                    selection.head
                } else if rows < 0 {
                    selection.start()
                } else {
                    selection.end()
                };
                let (row, x) = layout.locate_offset(buffer, from);
                let goal = selection.goal_column.map_or(x, |g| px(g as f32 / 16.0));
                let target = row as isize + rows;
                let head = if target < 0 {
                    0
                } else if target as usize >= layout.total_rows() {
                    buffer.len()
                } else {
                    layout.offset_at(buffer, target as usize, goal)
                };
                if select {
                    selection.head = head;
                } else {
                    selection.collapse_to(head);
                }
                selection.goal_column = Some((f32::from(goal) * 16.0).max(0.0) as usize);
                selection
            })
            .collect();
        self.doc.set_selections(selections);
    }

    fn add_cursors(&mut self, rows: isize, window: &Window) {
        let Some(layout) = self.current_layout(window) else {
            return;
        };
        let mut selections = self.doc.selections.clone();
        let current = self.doc.selections.clone();
        for selection in &current {
            let (row, x) = layout.locate_offset(&self.doc.buffer, selection.head);
            let goal = selection.goal_column.map_or(x, |g| px(g as f32 / 16.0));
            let target = row as isize + rows;
            if target < 0 || target as usize >= layout.total_rows() {
                continue;
            }
            let offset = layout.offset_at(&self.doc.buffer, target as usize, goal);
            let id = self.doc.new_selection_id();
            let mut added = Selection::cursor(id, offset);
            added.goal_column = Some((f32::from(goal) * 16.0).max(0.0) as usize);
            selections.push(added);
        }
        self.doc.set_selections(selections);
    }

    fn copy(&mut self, cx: &mut Context<Self>) -> bool {
        let copied = self.doc.copy_text();
        if copied.text.trim().is_empty() {
            return false;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(copied.text.clone()));
        self.clip = Some(copied);
        true
    }

    fn paste(&mut self, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) else {
            return;
        };
        let text = sanitize(&text);
        if text.is_empty() {
            return;
        }
        let clipboard = match &self.clip {
            Some(clip) if clip.text == text => clip.clone(),
            _ => ClipboardText {
                pieces: vec![text.clone()],
                text,
                whole_lines: false,
            },
        };
        self.doc.paste(&clipboard);
    }

    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let ks = &event.keystroke;
        let altgr = super::is_altgr(ks);
        let command = (ks.modifiers.control && !altgr) || ks.modifiers.platform;
        let alt = ks.modifiers.alt && !altgr;
        let shift = ks.modifiers.shift;
        let before = self.doc.buffer.version();
        let mut vertical = false;
        self.marked = None;
        match ks.key.as_str() {
            "z" if command && !shift => {
                self.doc.undo();
            }
            "y" | "z" if command => {
                self.doc.redo();
            }
            "s" if command => cx.emit(TextAreaEvent::Save),
            "enter" if command || (self.submit_on_enter && !shift) => {
                cx.emit(TextAreaEvent::Submit)
            }
            "enter" => self.doc.newline(),
            "tab" if shift => self.doc.indent_lines(true),
            "tab" => self.doc.tab(),
            "backspace" if command => self.doc.delete_to(prev_word),
            "backspace" => self.doc.backspace(),
            "delete" if command => self.doc.delete_to(next_word),
            "delete" => self.doc.delete_to(|b, o| b.next_boundary(o)),
            "left" if command => self.doc.move_heads(shift, |b, s| prev_word(b, s.head)),
            "left" => self.doc.move_heads(shift, |b, s| {
                if !shift && !s.is_empty() {
                    s.start()
                } else {
                    b.previous_boundary(s.head)
                }
            }),
            "right" if command => self.doc.move_heads(shift, |b, s| next_word(b, s.head)),
            "right" => self.doc.move_heads(shift, |b, s| {
                if !shift && !s.is_empty() {
                    s.end()
                } else {
                    b.next_boundary(s.head)
                }
            }),
            "up" | "down" if command && alt => {
                let rows = if ks.key == "up" { -1 } else { 1 };
                self.add_cursors(rows, window);
            }
            "up" | "down" if alt => {
                self.doc.reset_goal();
                if shift {
                    self.doc.copy_lines(ks.key == "down");
                } else {
                    self.doc.move_lines(ks.key == "up");
                }
            }
            "up" | "down" => {
                vertical = true;
                let rows = if ks.key == "up" { -1 } else { 1 };
                self.move_vertical(rows, shift, window);
            }
            "home" if command => self.doc.move_heads(shift, |_, _| 0),
            "home" => self
                .doc
                .move_heads(shift, |b, s| b.line_start(b.line_of(s.head))),
            "end" if command => self.doc.move_heads(shift, |b, _| b.len()),
            "end" => self
                .doc
                .move_heads(shift, |b, s| b.line_end(b.line_of(s.head))),
            "a" if command => self.doc.select_all(),
            "d" if command => {
                self.doc.select_next_occurrence();
            }
            "k" if command && shift => self.doc.delete_lines(),
            "c" if command => {
                self.copy(cx);
            }
            "x" if command => {
                if self.copy(cx) {
                    self.doc.cut();
                }
            }
            "v" if command => self.paste(cx),
            "escape" => {
                if self.doc.selections.len() > 1 {
                    self.doc.remove_secondary_cursors();
                } else {
                    return;
                }
            }
            _ => return,
        }
        if !vertical {
            self.doc.reset_goal();
        }
        let changed = self.doc.buffer.version() != before;
        self.finish(changed, window, cx);
        cx.stop_propagation();
    }

    fn to_utf16(&self, offset: usize) -> usize {
        let rope = self.doc.buffer.rope();
        rope.char_to_utf16_cu(rope.byte_to_char(offset.min(rope.len_bytes())))
    }

    fn from_utf16(&self, offset: usize) -> usize {
        let rope = self.doc.buffer.rope();
        rope.char_to_byte(rope.utf16_cu_to_char(offset.min(rope.len_utf16_cu())))
    }

    fn range_from_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.from_utf16(range.start)..self.from_utf16(range.end)
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.to_utf16(range.start)..self.to_utf16(range.end)
    }
}

impl EntityInputHandler for TextArea {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.doc.buffer.slice(range))
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let newest = self.doc.newest_selection();
        Some(UTF16Selection {
            range: self.range_to_utf16(&newest.range()),
            reversed: newest.reversed(),
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked.as_ref().map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.marked = None;
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = sanitize(text);
        let range = range_utf16
            .map(|range| self.range_from_utf16(&range))
            .or(self.marked.take());
        self.marked = None;
        let before = self.doc.buffer.version();
        match range {
            Some(range) => {
                self.doc.select_range(range);
                self.doc.insert_plain(&text, EditKind::Insert);
            }
            None if text.is_empty() => return,
            None => self.doc.insert(&text),
        }
        self.doc.reset_goal();
        let changed = self.doc.buffer.version() != before;
        self.finish(changed, window, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = sanitize(text);
        let range = range_utf16
            .map(|range| self.range_from_utf16(&range))
            .or(self.marked.clone())
            .unwrap_or_else(|| self.doc.newest_selection().range());
        let start = range.start;
        self.doc.select_range(range);
        self.doc.insert_plain(&text, EditKind::Insert);
        self.marked = (!text.is_empty()).then(|| start..start + text.len());
        if let Some(selected) = new_selected_range_utf16 {
            let byte_at = |utf16: usize| {
                let mut count = 0;
                for (index, ch) in text.char_indices() {
                    if count >= utf16 {
                        return index;
                    }
                    count += ch.len_utf16();
                }
                text.len()
            };
            self.doc
                .select_range(start + byte_at(selected.start)..start + byte_at(selected.end));
        }
        self.finish(true, window, cx);
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        _: Bounds<Pixels>,
        window: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let range = self.range_from_utf16(&range_utf16);
        let layout = self.current_layout(window)?;
        let bounds = self.bounds.get();
        let (row, x) = layout.locate_offset(&self.doc.buffer, range.start);
        let end_x = if range.end > range.start {
            let (end_row, end_x) = layout.locate_offset(&self.doc.buffer, range.end);
            if end_row == row { end_x } else { x + px(1.0) }
        } else {
            x + px(1.0)
        };
        Some(Bounds::new(
            point(
                bounds.left() + layout.gutter + x,
                bounds.top() + layout.line_height() * row as f32,
            ),
            size((end_x - x).max(px(1.0)), layout.line_height()),
        ))
    }

    fn character_index_for_point(
        &mut self,
        position: Point<Pixels>,
        window: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        let offset = self.offset_at_point(position, window)?;
        Some(self.to_utf16(offset))
    }
}

impl Focusable for TextArea {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for TextArea {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let scroll = div().id("text-area-scroll").overflow_y_scroll().child(
            div()
                .w_full()
                .p(px(PAD))
                .child(EditorElement { area: cx.entity() }),
        );
        div()
            .id("text-area")
            .key_context("TextArea")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, e, window, cx| this.on_key(e, window, cx)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, e, window, cx| this.on_mouse_down(e, window, cx)),
            )
            .on_mouse_move(cx.listener(|this, e, window, cx| this.on_mouse_move(e, window, cx)))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.drag = None),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.drag = None),
            )
            .flex()
            .flex_col()
            .size_full()
            .min_h_0()
            .text_size(px(TEXT_SIZE))
            .text_color(theme::text())
            .cursor_text()
            .child(self.scrollbar.overlay(scroll))
    }
}

struct EditorElement {
    area: Entity<TextArea>,
}

struct Prepaint {
    layout: Rc<WrapLayout>,
    selections: Vec<PaintQuad>,
    marks: Vec<PaintQuad>,
    carets: Vec<PaintQuad>,
    lines: Range<usize>,
    numbers: Vec<(usize, SharedString)>,
}

impl IntoElement for EditorElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for EditorElement {
    type RequestLayoutState = ();
    type PrepaintState = Prepaint;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let style = window.text_style();
        let font = style.font();
        let font_size = style.font_size.to_pixels(window.rem_size());
        let area = self.area.clone();
        let (line_numbers, placeholder) = {
            let a = area.read(cx);
            (a.line_numbers, a.placeholder.clone())
        };
        let mut layout_style = Style::default();
        layout_style.size.width = relative(1.).into();
        let id =
            window.request_measured_layout(layout_style, move |known, available, window, cx| {
                let width = known.width.unwrap_or(match available.width {
                    AvailableSpace::Definite(width) => width,
                    _ => px(400.0),
                });
                let params = Params {
                    font: font.clone(),
                    font_size,
                    line_height: px(LINE_HEIGHT),
                    width,
                    line_numbers,
                    placeholder: placeholder.clone(),
                    color: gpui::rgb_to_hsla(theme::text()),
                    faint: gpui::rgb_to_hsla(theme::text_faint()),
                };
                let layout = area.read(cx).layout_for(params, window);
                size(width, layout.height())
            });
        (id, ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Prepaint {
        let area = self.area.read(cx);
        let layout = area
            .current_layout(window)
            .expect("measured before prepaint");
        let buffer = &area.doc.buffer;
        let rows = layout.total_rows().max(1);
        let rows_changed = area.rows.replace(rows) != rows;
        let lh = layout.line_height();
        let mask = window.content_mask().bounds;
        let first_row = ((mask.top() - bounds.top()) / lh).floor().max(0.0) as usize;
        let last_row = (((mask.bottom() - bounds.top()) / lh).ceil().max(0.0) as usize)
            .min(layout.total_rows().max(1));
        let rows = first_row..last_row;
        let lines =
            layout.line_of_row(first_row)..layout.line_of_row(last_row.saturating_sub(1)) + 1;
        let origin_x = bounds.left() + layout.gutter;
        let rect = |row: usize, x0: Pixels, x1: Pixels, top: Pixels, height: Pixels| {
            Bounds::new(
                point(origin_x + x0, bounds.top() + lh * row as f32 + top),
                size(x1 - x0, height),
            )
        };

        let mut selections = Vec::new();
        let mut carets = Vec::new();
        let focused = area.focus.is_focused(window);
        let elapsed = area.last_activity.elapsed();
        let phase = BLINK_HALF_PERIOD.as_millis();
        let blink_on = (elapsed.as_millis() / phase) % 2 == 0;
        for selection in &area.doc.selections {
            for (row, x0, x1) in layout.range_rects(buffer, selection.range(), true, rows.clone()) {
                selections.push(fill(rect(row, x0, x1, px(0.0), lh), theme::accent_soft()));
            }
            if focused && blink_on && rows.contains(&layout.locate_offset(buffer, selection.head).0)
            {
                let (row, x) = layout.locate_offset(buffer, selection.head);
                carets.push(fill(
                    rect(row, x, x + px(1.0), px(1.5), lh - px(3.0)),
                    theme::accent(),
                ));
            }
        }
        let marks = area
            .marked
            .clone()
            .map(|range| {
                layout
                    .range_rects(buffer, range, false, rows.clone())
                    .into_iter()
                    .map(|(row, x0, x1)| {
                        fill(rect(row, x0, x1, lh - px(2.0), px(1.0)), theme::text())
                    })
                    .collect()
            })
            .unwrap_or_default();

        let numbers = if layout.gutter > px(0.0) {
            lines
                .clone()
                .map(|line| (line, SharedString::from((line + 1).to_string())))
                .collect()
        } else {
            Vec::new()
        };

        let blink_pending = area.blink_pending;
        if rows_changed {
            let entity = self.area.clone();
            cx.defer(move |cx| entity.update(cx, |_, cx| cx.notify()));
        }
        if focused && !blink_pending {
            let wait = Duration::from_millis((phase - elapsed.as_millis() % phase) as u64);
            let timer = cx.background_executor().timer(wait);
            let entity = self.area.clone();
            entity.update(cx, |area, _| area.blink_pending = true);
            window
                .spawn(cx, async move |cx| {
                    timer.await;
                    let _ = entity.update(cx, |area, cx| {
                        area.blink_pending = false;
                        cx.notify();
                    });
                })
                .detach();
        }

        Prepaint {
            layout,
            selections,
            marks,
            carets,
            lines,
            numbers,
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        prepaint: &mut Prepaint,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus = self.area.read(cx).focus.clone();
        window.handle_input(
            &focus,
            ElementInputHandler::new(bounds, self.area.clone()),
            cx,
        );
        let layout = prepaint.layout.clone();
        let lh = layout.line_height();
        for quad in prepaint.selections.drain(..) {
            window.paint_quad(quad);
        }

        if layout.gutter > px(0.0) {
            for (line, number) in prepaint.numbers.drain(..) {
                let run = TextRun {
                    len: number.len(),
                    font: layout.params.font.clone(),
                    color: layout.params.faint,
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                    letter_spacing: None,
                };
                let shaped =
                    window
                        .text_system()
                        .shape_line(number, layout.params.font_size, &[run], None);
                let x = bounds.left() + layout.gutter - px(8.0) - shaped.width;
                let y = bounds.top() + lh * layout.row_starts[line] as f32;
                let _ = shaped.paint(point(x, y), lh, TextAlign::Left, None, window, cx);
            }
        }

        let origin_x = bounds.left() + layout.gutter;
        let mut y = bounds.top();
        for line in &layout.placeholder {
            let _ = line.paint(point(origin_x, y), lh, TextAlign::Left, None, window, cx);
            y += line.size(lh).height;
        }
        for line in prepaint.lines.clone() {
            let y = bounds.top() + lh * layout.row_starts[line] as f32;
            let _ =
                layout.lines[line].paint(point(origin_x, y), lh, TextAlign::Left, None, window, cx);
        }

        for quad in prepaint.marks.drain(..) {
            window.paint_quad(quad);
        }
        for quad in prepaint.carets.drain(..) {
            window.paint_quad(quad);
        }
        self.area.update(cx, |area, _| area.bounds.set(bounds));
    }
}
