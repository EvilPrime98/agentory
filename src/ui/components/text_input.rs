//! Single-line text field. GPUI core has no text input, so this handles typing,
//! caret movement, selection (shift+arrows, select all, drag-free), copy/cut/paste, backspace/delete
//! and Enter/Escape.
//! Known gaps: no IME composition, no mouse-drag selection, caret is char-based (not grapheme-based).

use gpui::{
    App, ClipboardItem, Context, EventEmitter, FocusHandle, Focusable, InteractiveElement,
    IntoElement, KeyDownEvent, MouseButton, ParentElement, Render, SharedString, Styled, Window,
    div, prelude::*, px,
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
        }
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
                Some((from, _)) if !shift => {
                    self.anchor = None;
                    self.caret = from;
                }
                _ => self.move_caret(self.caret.saturating_sub(1), shift),
            },
            "right" => match self.selection() {
                Some((_, to)) if !shift => {
                    self.anchor = None;
                    self.caret = to;
                }
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.focus.is_focused(window);
        let caret = div().w(px(1.0)).h(px(15.0)).bg(if focused {
            theme::accent()
        } else {
            gpui::rgba(0x00000000)
        });

        let content = if self.value.is_empty() {
            div().flex().items_center().child(caret).child(
                div()
                    .text_color(gpui::rgb(0x8a8a8a))
                    .child(self.placeholder.clone()),
            )
        } else if let Some((from, to)) = self.selection() {
            let (b_from, b_to) = (self.byte_index(from), self.byte_index(to));
            let selected = div()
                .bg(theme::accent_soft())
                .child(self.value[b_from..b_to].to_string());
            // The caret sits at whichever end of the selection it was moved to.
            let (before, after) = (
                self.value[..b_from].to_string(),
                self.value[b_to..].to_string(),
            );
            let at_start = self.caret == from;
            div()
                .flex()
                .items_center()
                .child(div().child(before))
                .when(at_start, |s| {
                    s.child(div().w(px(1.0)).h(px(15.0)).bg(theme::accent()))
                })
                .child(selected)
                .when(!at_start, |s| {
                    s.child(div().w(px(1.0)).h(px(15.0)).bg(theme::accent()))
                })
                .child(div().child(after))
        } else {
            let (before, after) = self.value.split_at(self.byte_index(self.caret));
            div()
                .flex()
                .items_center()
                .child(div().child(before.to_string()))
                .child(caret)
                .child(div().child(after.to_string()))
        };

        div()
            .id("text-input")
            .key_context("TextInput")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, e, window, cx| this.on_key(e, window, cx)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    window.focus(&this.focus, cx);
                    this.anchor = None;
                    this.caret = this.len();
                    cx.notify();
                }),
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
            .child(content)
    }
}
