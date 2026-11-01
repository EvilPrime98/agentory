//! Single-line text field. GPUI core has no text input, so this handles typing,
//! caret movement (keys and click), selection (shift+arrows, drag, double-click word, select all),
//! word jumps, copy/cut/paste, backspace/delete, horizontal scrolling and Enter/Escape.
//! The text is shaped and painted by [`TextLine`], so the caret is an overlay (takes no space)
//! and clicks map to real glyph positions.
//! Known gaps: no IME composition, caret is char-based (not grapheme-based).

use gpui::{
    App, Bounds, ClipboardItem, Context, Element, ElementId, Entity, EventEmitter, FocusHandle,
    Focusable, GlobalElementId, InspectorElementId, InteractiveElement, IntoElement, KeyDownEvent,
    LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, ParentElement,
    Pixels, Point, Render, ShapedLine, SharedString, Style, Styled, TextAlign, TextRun, Window,
    div, fill, point, px, relative, size,
};

use crate::ui::theme;

pub enum TextInputEvent {
    Changed,
    Submit,
    Cancel,
}

pub struct TextInput {
    value: String,
    /// Caret position in chars.
    caret: usize,
    /// Other end of the selection (in chars); `None` or equal to `caret` means no selection.
    anchor: Option<usize>,
    placeholder: SharedString,
    focus: FocusHandle,
    /// Horizontal scroll of the text inside the field, kept in sync by `TextLine`.
    scroll_x: Pixels,
    /// Layout of the last paint, used to map mouse positions to chars.
    last_layout: Option<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
    dragging: bool,
}

impl EventEmitter<TextInputEvent> for TextInput {}

impl TextInput {
    pub fn new(placeholder: impl Into<SharedString>, cx: &mut Context<Self>) -> Self {
        Self {
            value: String::new(),
            caret: 0,
            anchor: None,
            placeholder: placeholder.into(),
            focus: cx.focus_handle(),
            scroll_x: px(0.0),
            last_layout: None,
            last_bounds: None,
            dragging: false,
        }
    }

    /// Replace the hint shown while empty. Does not notify: call it from the owner's `render`.
    pub fn set_placeholder(&mut self, placeholder: impl Into<SharedString>) {
        self.placeholder = placeholder.into();
    }

    pub fn with_value(mut self, value: impl Into<String>) -> Self {
        self.value = value.into();
        self.caret = self.len();
        self
    }

    pub fn value(&self) -> &str {
        &self.value
    }

    /// Replace the text (no `Changed` event: callers use this to load a value).
    pub fn set_value(&mut self, value: impl Into<String>, cx: &mut Context<Self>) {
        self.value = value.into();
        self.caret = self.len();
        self.anchor = None;
        cx.notify();
    }

    fn len(&self) -> usize {
        self.value.chars().count()
    }

    fn byte_index(&self, chars: usize) -> usize {
        self.value
            .char_indices()
            .nth(chars)
            .map(|(i, _)| i)
            .unwrap_or(self.value.len())
    }

    fn char_index(&self, byte: usize) -> usize {
        self.value[..byte.min(self.value.len())].chars().count()
    }

    /// Start of the word at or before `from` (skips whitespace first).
    fn word_start(&self, from: usize) -> usize {
        let chars: Vec<char> = self.value.chars().collect();
        let mut i = from.min(chars.len());
        while i > 0 && chars[i - 1].is_whitespace() {
            i -= 1;
        }
        while i > 0 && !chars[i - 1].is_whitespace() {
            i -= 1;
        }
        i
    }

    /// End of the word at or after `from` (skips whitespace first).
    fn word_end(&self, from: usize) -> usize {
        let chars: Vec<char> = self.value.chars().collect();
        let mut i = from.min(chars.len());
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
        while i < chars.len() && !chars[i].is_whitespace() {
            i += 1;
        }
        i
    }

    /// Selected range in chars, ordered.
    fn selection(&self) -> Option<(usize, usize)> {
        let anchor = self.anchor?;
        (anchor != self.caret).then(|| (anchor.min(self.caret), anchor.max(self.caret)))
    }

    fn selected_text(&self) -> Option<String> {
        let (from, to) = self.selection()?;
        Some(self.value[self.byte_index(from)..self.byte_index(to)].to_string())
    }

    /// Delete the selection, leaving the caret where it was. Returns whether anything was removed.
    fn delete_selection(&mut self) -> bool {
        let Some((from, to)) = self.selection() else {
            return false;
        };
        let (b_from, b_to) = (self.byte_index(from), self.byte_index(to));
        self.value.replace_range(b_from..b_to, "");
        self.caret = from;
        self.anchor = None;
        true
    }

    fn insert(&mut self, text: &str, cx: &mut Context<Self>) {
        // Single line: drop control characters and newlines.
        let clean: String = text.chars().filter(|c| !c.is_control()).collect();
        if clean.is_empty() {
            return;
        }
        self.delete_selection();
        let at = self.byte_index(self.caret);
        self.value.insert_str(at, &clean);
        self.caret += clean.chars().count();
        cx.emit(TextInputEvent::Changed);
    }

    /// Move the caret, extending the selection when `extend` is set.
    fn move_caret(&mut self, to: usize, extend: bool) {
        if extend {
            self.anchor.get_or_insert(self.caret);
        } else {
            self.anchor = None;
        }
        self.caret = to;
    }

    /// Char index under a window position, from the last painted layout.
    fn index_at(&self, position: Point<Pixels>) -> usize {
        let (Some(layout), Some(bounds)) = (&self.last_layout, self.last_bounds) else {
            return self.len();
        };
        if self.value.is_empty() {
            return 0;
        }
        let x = position.x - bounds.left() + self.scroll_x;
        self.char_index(layout.closest_index_for_x(x))
    }

    fn on_mouse_down(&mut self, e: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        let at = self.index_at(e.position);
        match e.click_count {
            1 => {
                self.dragging = true;
                self.move_caret(at, e.modifiers.shift);
            }
            2 => {
                self.anchor = Some(self.word_start(at));
                self.caret = self.word_end(at);
            }
            _ => {
                self.anchor = Some(0);
                self.caret = self.len();
            }
        }
        cx.notify();
    }

    fn on_mouse_move(&mut self, e: &MouseMoveEvent, cx: &mut Context<Self>) {
        if self.dragging && e.pressed_button == Some(MouseButton::Left) {
            let at = self.index_at(e.position);
            self.move_caret(at, true);
            cx.notify();
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.dragging = false;
    }

    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let ks = &event.keystroke;
        let len = self.len();
        let command = ks.modifiers.control || ks.modifiers.platform;
        let shift = ks.modifiers.shift;
        match ks.key.as_str() {
            "enter" => cx.emit(TextInputEvent::Submit),
            "escape" => {
                cx.emit(TextInputEvent::Cancel);
                window.blur();
            }
            "backspace" => {
                if command && self.selection().is_none() {
                    self.anchor = Some(self.word_start(self.caret));
                }
                if !self.delete_selection() {
                    if self.caret == 0 {
                        return cx.stop_propagation();
                    }
                    let (from, to) = (self.byte_index(self.caret - 1), self.byte_index(self.caret));
                    self.value.replace_range(from..to, "");
                    self.caret -= 1;
                }
                cx.emit(TextInputEvent::Changed);
            }
            "delete" => {
                if !self.delete_selection() {
                    if self.caret >= len {
                        return cx.stop_propagation();
                    }
                    let (from, to) = (self.byte_index(self.caret), self.byte_index(self.caret + 1));
                    self.value.replace_range(from..to, "");
                }
                cx.emit(TextInputEvent::Changed);
            }
            "left" => match self.selection() {
                Some((from, _)) if !shift && !command => {
                    self.anchor = None;
                    self.caret = from;
                }
                _ if command => self.move_caret(self.word_start(self.caret), shift),
                _ => self.move_caret(self.caret.saturating_sub(1), shift),
            },
            "right" => match self.selection() {
                Some((_, to)) if !shift && !command => {
                    self.anchor = None;
                    self.caret = to;
                }
                _ if command => self.move_caret(self.word_end(self.caret), shift),
                _ => self.move_caret((self.caret + 1).min(len), shift),
            },
            "home" => self.move_caret(0, shift),
            "end" => self.move_caret(len, shift),
            "a" if command => {
                self.anchor = Some(0);
                self.caret = len;
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
                    cx.emit(TextInputEvent::Changed);
                }
            }
            "v" if command => {
                if let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) {
                    self.insert(&text, cx);
                }
            }
            _ if command || ks.modifiers.alt => return,
            _ => match &ks.key_char {
                Some(ch) => self.insert(ch, cx),
                None => return,
            },
        }
        cx.stop_propagation();
        cx.notify();
    }
}

impl Focusable for TextInput {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for TextInput {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("text-input")
            .key_context("TextInput")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, e, window, cx| this.on_key(e, window, cx)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, e, w, cx| this.on_mouse_down(e, w, cx)),
            )
            .on_mouse_move(cx.listener(|this, e, _, cx| this.on_mouse_move(e, cx)))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, e, w, cx| this.on_mouse_up(e, w, cx)),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, e, w, cx| this.on_mouse_up(e, w, cx)),
            )
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .items_center()
            .overflow_hidden()
            .text_size(px(13.0))
            .text_color(theme::text())
            .cursor_text()
            .child(TextLine { input: cx.entity() })
    }
}

/// Paints the text, selection and caret of a [`TextInput`] as one shaped line.
struct TextLine {
    input: Entity<TextInput>,
}

struct Prepaint {
    line: ShapedLine,
    selection: Option<PaintQuad>,
    caret: Option<PaintQuad>,
    scroll_x: Pixels,
}

impl IntoElement for TextLine {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for TextLine {
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
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = window.line_height().into();
        (window.request_layout(style, [], cx), ())
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
        let input = self.input.read(cx);
        let empty = input.value.is_empty();
        let text: SharedString = if empty {
            input.placeholder.clone()
        } else {
            input.value.clone().into()
        };
        let style = window.text_style();
        let run = TextRun {
            len: text.len(),
            font: style.font(),
            color: gpui::rgb_to_hsla(if empty {
                theme::text_faint()
            } else {
                theme::text()
            }),
            background_color: None,
            underline: None,
            strikethrough: None,
            letter_spacing: None,
        };
        let font_size = style.font_size.to_pixels(window.rem_size());
        let line = window
            .text_system()
            .shape_line(text, font_size, &[run], None);

        let x_of = |chars: usize| {
            if empty {
                px(0.0)
            } else {
                line.x_for_index(input.byte_index(chars))
            }
        };
        let caret_x = x_of(input.caret);

        // Keep the caret inside the field.
        let mut scroll_x = input.scroll_x;
        if empty {
            scroll_x = px(0.0);
        } else {
            if caret_x - scroll_x > bounds.size.width - px(2.0) {
                scroll_x = caret_x - bounds.size.width + px(2.0);
            }
            if caret_x < scroll_x {
                scroll_x = caret_x;
            }
            scroll_x = scroll_x
                .min(line.width - bounds.size.width + px(2.0))
                .max(px(0.0));
        }

        let at = |x: Pixels, w: Pixels| {
            Bounds::new(
                point(bounds.left() + x - scroll_x, bounds.top()),
                size(w, bounds.size.height),
            )
        };
        let selection = input
            .selection()
            .map(|(from, to)| fill(at(x_of(from), x_of(to) - x_of(from)), theme::accent_soft()));
        let caret = input
            .focus
            .is_focused(window)
            .then(|| fill(at(caret_x, px(1.0)), theme::accent()));

        Prepaint {
            line,
            selection,
            caret,
            scroll_x,
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
        if let Some(selection) = prepaint.selection.take() {
            window.paint_quad(selection);
        }
        let origin = point(bounds.left() - prepaint.scroll_x, bounds.top());
        let _ = prepaint.line.paint(
            origin,
            window.line_height(),
            TextAlign::Left,
            None,
            window,
            cx,
        );
        if let Some(caret) = prepaint.caret.take() {
            window.paint_quad(caret);
        }
        let line = prepaint.line.clone();
        let scroll_x = prepaint.scroll_x;
        self.input.update(cx, |input, _| {
            input.last_layout = Some(line);
            input.last_bounds = Some(bounds);
            input.scroll_x = scroll_x;
        });
    }
}
