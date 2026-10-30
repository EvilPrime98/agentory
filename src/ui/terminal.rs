//! Terminal widget: a PTY (ConPTY on Windows) feeding a `vt100` screen, drawn as a grid of styled
//! runs. Input is translated from GPUI key events to the byte sequences a terminal sends.
//!
//! Known gaps: no mouse selection/copy, no mouse reporting, no IME, wide glyphs are clipped to their
//! cell width.

use std::io::Read as _;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use futures_channel::mpsc::unbounded;
use futures_util::StreamExt as _;
use gpui::{
    App, ClipboardItem, Context, EventEmitter, FocusHandle, Focusable, InteractiveElement,
    IntoElement, KeyDownEvent, Keystroke, MouseButton, ParentElement, Pixels, Render, Rgba,
    ScrollWheelEvent, SharedString, Size, Styled, Window, canvas, div, prelude::*, px, rgb,
};

use crate::platform::pty::{Pty, Spawned};
use crate::ui::theme;

const SCROLLBACK_LINES: usize = 5000;

/// Emitted once when the child process is gone.
pub struct TerminalExited;

enum Msg {
    Output,
    /// Answer to a terminal query found in the output (cursor position, device attributes).
    Reply(Vec<u8>),
    Exited,
}

const QUERIES: [&[u8]; 3] = [b"\x1b[6n", b"\x1b[c", b"\x1b[0c"];

/// Feed `data` to the screen and collect the replies a real terminal would send. ConPTY asks for
/// the cursor position on startup and holds the child's input until it gets an answer.
fn process_with_replies(parser: &Mutex<vt100::Parser>, data: &[u8]) -> Vec<u8> {
    let mut replies = Vec::new();
    let mut rest = data;
    loop {
        let hit = QUERIES
            .iter()
            .filter_map(|q| {
                rest.windows(q.len())
                    .position(|w| w == *q)
                    .map(|at| (at, *q))
            })
            .min_by_key(|(at, _)| *at);
        let Some((at, query)) = hit else {
            lock(parser).process(rest);
            return replies;
        };
        let end = at + query.len();
        let mut p = lock(parser);
        p.process(&rest[..end]);
        if query == QUERIES[0] {
            let (row, col) = p.screen().cursor_position();
            replies.extend_from_slice(format!("\x1b[{};{}R", row + 1, col + 1).as_bytes());
        } else {
            replies.extend_from_slice(b"\x1b[?1;0c");
        }
        drop(p);
        rest = &rest[end..];
    }
}

pub struct Terminal {
    parser: Arc<Mutex<vt100::Parser>>,
    pty: Option<Box<dyn Pty>>,
    rows: u16,
    cols: u16,
    /// Cell size in px, measured from the font on every render.
    cell: (f32, f32),
    font_size: f32,
    wheel_remainder: f32,
    pub exited: bool,
    focus: FocusHandle,
}

impl EventEmitter<TerminalExited> for Terminal {}

fn lock(p: &Mutex<vt100::Parser>) -> MutexGuard<'_, vt100::Parser> {
    p.lock().unwrap_or_else(|e| e.into_inner())
}

impl Terminal {
    pub fn new(
        spawned: Spawned,
        rows: u16,
        cols: u16,
        font_size: f32,
        cx: &mut Context<Self>,
    ) -> Self {
        let (rows, cols) = (rows.max(3), cols.max(10));
        let parser = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, SCROLLBACK_LINES)));

        // Reader thread: blocking reads, parse into the shared screen, wake the UI.
        let (tx, mut rx) = unbounded::<Msg>();
        let thread_parser = parser.clone();
        let mut reader = spawned.reader;
        let spawn_reader = std::thread::Builder::new()
            .name("agentory-pty-reader".into())
            .spawn(move || {
                let mut buf = [0u8; 16 * 1024];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            let replies = process_with_replies(&thread_parser, &buf[..n]);
                            if !replies.is_empty() && tx.unbounded_send(Msg::Reply(replies)).is_err()
                            {
                                return;
                            }
                            if tx.unbounded_send(Msg::Output).is_err() {
                                return;
                            }
                        }
                    }
                }
                let _ = tx.unbounded_send(Msg::Exited);
            });
        if let Err(e) = spawn_reader {
            tracing::error!("could not start the pty reader thread: {e}");
        }

        cx.spawn(async move |this, cx| {
            while let Some(first) = rx.next().await {
                let mut exited = false;
                let mut replies = Vec::new();
                let mut absorb = |msg: Msg| match msg {
                    Msg::Exited => exited = true,
                    Msg::Reply(bytes) => replies.extend(bytes),
                    Msg::Output => {}
                };
                absorb(first);
                while let Ok(next) = rx.try_recv() {
                    absorb(next);
                }
                let alive = this
                    .update(cx, |t, cx| {
                        t.write_raw(&replies);
                        if exited {
                            t.mark_exited(cx);
                        }
                        cx.notify();
                    })
                    .is_ok();
                if !alive {
                    break;
                }
                // Cap the repaint rate while output streams.
                cx.background_executor()
                    .timer(Duration::from_millis(8))
                    .await;
            }
        })
        .detach();

        Self {
            parser,
            pty: Some(spawned.pty),
            rows,
            cols,
            cell: (font_size * 0.6, (font_size * 1.35).round()),
            font_size,
            wheel_remainder: 0.0,
            exited: false,
            focus: cx.focus_handle(),
        }
    }

    fn mark_exited(&mut self, cx: &mut Context<Self>) {
        if !self.exited {
            self.exited = true;
            cx.emit(TerminalExited);
        }
    }

    /// Poll the child for exit (the reader may not see EOF on every platform).
    pub fn check_exit(&mut self, cx: &mut Context<Self>) {
        if !self.exited && self.pty.as_mut().is_some_and(|p| p.has_exited()) {
            self.mark_exited(cx);
            cx.notify();
        }
    }

    pub fn set_font_size(&mut self, size: f32, cx: &mut Context<Self>) {
        if (self.font_size - size).abs() > f32::EPSILON {
            self.font_size = size;
            cx.notify();
        }
    }

    pub fn kill(&mut self) {
        if let Some(p) = &mut self.pty {
            p.kill();
        }
    }

    pub fn send(&mut self, bytes: &[u8]) {
        if self.exited {
            return;
        }
        lock(&self.parser).set_scrollback(0);
        self.write_raw(bytes);
    }

    fn write_raw(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        if let Some(p) = &mut self.pty {
            if let Err(e) = p.write(bytes) {
                tracing::warn!("pty write failed: {e}");
            }
        }
    }

    /// Fit the grid to the pixel area the widget got.
    fn set_viewport(&mut self, size: Size<Pixels>, cx: &mut Context<Self>) {
        let (cw, ch) = self.cell;
        let cols = ((f32::from(size.width) / cw).floor() as u16).max(10);
        let rows = ((f32::from(size.height) / ch).floor() as u16).max(3);
        if cols == self.cols && rows == self.rows {
            return;
        }
        self.cols = cols;
        self.rows = rows;
        lock(&self.parser).set_size(rows, cols);
        if let Some(p) = &self.pty {
            p.resize(rows, cols);
        }
        cx.notify();
    }

    fn on_key(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let ks = &event.keystroke;
        let m = &ks.modifiers;
        if (m.control && m.shift && ks.key == "v") || (m.shift && ks.key == "insert") {
            if let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) {
                self.paste(&text);
            }
            cx.stop_propagation();
            return;
        }
        if m.control && m.shift && ks.key == "c" {
            // No selection yet: copy the visible screen instead of nothing.
            let text = lock(&self.parser).screen().contents();
            cx.write_to_clipboard(ClipboardItem::new_string(text));
            cx.stop_propagation();
            return;
        }
        let app_cursor = lock(&self.parser).screen().application_cursor();
        if let Some(bytes) = key_bytes(ks, app_cursor) {
            self.send(&bytes);
            cx.stop_propagation();
            cx.notify();
        }
    }

    fn paste(&mut self, text: &str) {
        let bracketed = lock(&self.parser).screen().bracketed_paste();
        let text = text.replace("\r\n", "\r").replace('\n', "\r");
        if bracketed {
            self.send(format!("\x1b[200~{text}\x1b[201~").as_bytes());
        } else {
            self.send(text.as_bytes());
        }
    }

    fn on_wheel(&mut self, ev: &ScrollWheelEvent, cx: &mut Context<Self>) {
        let lines_f =
            f32::from(ev.delta.pixel_delta(px(self.cell.1)).y) / self.cell.1 + self.wheel_remainder;
        let lines = lines_f.trunc();
        self.wheel_remainder = lines_f - lines;
        if lines == 0.0 {
            return;
        }
        let mut p = lock(&self.parser);
        let current = p.screen().scrollback() as i64;
        p.set_scrollback((current + lines as i64).max(0) as usize);
        drop(p);
        cx.notify();
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        self.kill();
    }
}

impl Focusable for Terminal {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

// ---------------------------------------------------------------------------------------------
// Input
// ---------------------------------------------------------------------------------------------

/// xterm modifier parameter (`1 + shift + 2·alt + 4·ctrl`), `None` when no modifier is held.
fn modifier_param(ks: &Keystroke) -> Option<u8> {
    let m = &ks.modifiers;
    let n = u8::from(m.shift) + 2 * u8::from(m.alt) + 4 * u8::from(m.control);
    (n != 0).then_some(1 + n)
}

/// The bytes a terminal sends for a key press, or `None` when the key is not for the PTY.
pub fn key_bytes(ks: &Keystroke, application_cursor: bool) -> Option<Vec<u8>> {
    let m = &ks.modifiers;
    if m.platform {
        return None;
    }
    let key = ks.key.as_str();

    let csi = |final_byte: char| -> Vec<u8> {
        match modifier_param(ks) {
            Some(p) => format!("\x1b[1;{p}{final_byte}").into_bytes(),
            None if application_cursor => format!("\x1bO{final_byte}").into_bytes(),
            None => format!("\x1b[{final_byte}").into_bytes(),
        }
    };
    let tilde = |n: u8| -> Vec<u8> {
        match modifier_param(ks) {
            Some(p) => format!("\x1b[{n};{p}~").into_bytes(),
            None => format!("\x1b[{n}~").into_bytes(),
        }
    };

    let named: Option<Vec<u8>> = match key {
        // Alt/Shift+Enter inserts a newline in Claude Code.
        "enter" => Some(if m.alt || m.shift {
            b"\x1b\r".to_vec()
        } else {
            b"\r".to_vec()
        }),
        "tab" => Some(if m.shift {
            b"\x1b[Z".to_vec()
        } else {
            b"\t".to_vec()
        }),
        "escape" => Some(b"\x1b".to_vec()),
        "backspace" => Some(if m.alt {
            b"\x1b\x7f".to_vec()
        } else {
            b"\x7f".to_vec()
        }),
        "up" => Some(csi('A')),
        "down" => Some(csi('B')),
        "right" => Some(csi('C')),
        "left" => Some(csi('D')),
        "home" => Some(csi('H')),
        "end" => Some(csi('F')),
        "insert" => Some(tilde(2)),
        "delete" => Some(tilde(3)),
        "pageup" => Some(tilde(5)),
        "pagedown" => Some(tilde(6)),
        "f1" => Some(b"\x1bOP".to_vec()),
        "f2" => Some(b"\x1bOQ".to_vec()),
        "f3" => Some(b"\x1bOR".to_vec()),
        "f4" => Some(b"\x1bOS".to_vec()),
        "f5" => Some(tilde(15)),
        "f6" => Some(tilde(17)),
        "f7" => Some(tilde(18)),
        "f8" => Some(tilde(19)),
        "f9" => Some(tilde(20)),
        "f10" => Some(tilde(21)),
        "f11" => Some(tilde(23)),
        "f12" => Some(tilde(24)),
        "space" if m.control => Some(vec![0]),
        _ => None,
    };
    if named.is_some() {
        return named;
    }

    if m.control {
        let mut chars = key.chars();
        if let (Some(c), None) = (chars.next(), chars.next()) {
            if c.is_ascii_alphabetic() {
                return Some(vec![(c.to_ascii_lowercase() as u8) & 0x1f]);
            }
            let code = match c {
                '[' => 0x1b,
                '\\' => 0x1c,
                ']' => 0x1d,
                '^' | '6' => 0x1e,
                '_' | '-' => 0x1f,
                _ => return None,
            };
            return Some(vec![code]);
        }
        return None;
    }

    let text = ks.key_char.as_deref().filter(|s| !s.is_empty())?;
    let mut out = Vec::with_capacity(text.len() + 1);
    if m.alt {
        out.push(0x1b);
    }
    out.extend_from_slice(text.as_bytes());
    Some(out)
}

// ---------------------------------------------------------------------------------------------
// Output
// ---------------------------------------------------------------------------------------------

/// A horizontal span of cells sharing one style.
#[derive(Debug, Clone, PartialEq)]
struct Run {
    text: String,
    cells: u16,
    fg: vt100::Color,
    bg: vt100::Color,
    bold: bool,
    italic: bool,
    underline: bool,
    inverse: bool,
    cursor: bool,
}

fn build_rows(screen: &vt100::Screen, show_cursor: bool) -> Vec<Vec<Run>> {
    let (rows, cols) = screen.size();
    let cursor = (show_cursor && !screen.hide_cursor() && screen.scrollback() == 0)
        .then(|| screen.cursor_position());
    (0..rows)
        .map(|r| {
            let mut runs: Vec<Run> = Vec::new();
            for c in 0..cols {
                let Some(cell) = screen.cell(r, c) else {
                    continue;
                };
                if cell.is_wide_continuation() {
                    continue;
                }
                let width: u16 = if cell.is_wide() { 2 } else { 1 };
                let contents = cell.contents();
                let text = if contents.is_empty() { " " } else { &contents };
                let is_cursor = cursor == Some((r, c));
                let next = Run {
                    text: text.to_string(),
                    cells: width,
                    fg: cell.fgcolor(),
                    bg: cell.bgcolor(),
                    bold: cell.bold(),
                    italic: cell.italic(),
                    underline: cell.underline(),
                    inverse: cell.inverse(),
                    cursor: is_cursor,
                };
                match runs.last_mut() {
                    Some(last) if !last.cursor && !next.cursor && same_style(last, &next) => {
                        last.text.push_str(&next.text);
                        last.cells += next.cells;
                    }
                    _ => runs.push(next),
                }
            }
            runs
        })
        .collect()
}

fn same_style(a: &Run, b: &Run) -> bool {
    a.fg == b.fg
        && a.bg == b.bg
        && a.bold == b.bold
        && a.italic == b.italic
        && a.underline == b.underline
        && a.inverse == b.inverse
}

const ANSI: [u32; 16] = [
    0x1e1e24, 0xcd3131, 0x0dbc79, 0xe5e510, 0x2472c8, 0xbc3fbc, 0x11a8cd, 0xe5e5e5, 0x666666,
    0xf14c4c, 0x23d18b, 0xf5f543, 0x3b8eea, 0xd670d6, 0x29b8db, 0xffffff,
];

fn indexed(i: u8) -> Rgba {
    match i {
        0..=15 => rgb(ANSI[i as usize]),
        16..=231 => {
            let i = i - 16;
            let level = |n: u8| if n == 0 { 0u32 } else { 55 + 40 * n as u32 };
            rgb((level(i / 36) << 16) | (level((i / 6) % 6) << 8) | level(i % 6))
        }
        232..=255 => {
            let v = 8 + 10 * (i as u32 - 232);
            rgb((v << 16) | (v << 8) | v)
        }
    }
}

fn resolve(color: vt100::Color, default: Rgba) -> Rgba {
    match color {
        vt100::Color::Default => default,
        vt100::Color::Idx(i) => indexed(i),
        vt100::Color::Rgb(r, g, b) => rgb(((r as u32) << 16) | ((g as u32) << 8) | b as u32),
    }
}

/// Foreground and background after `inverse` and the cursor block are applied.
fn run_colors(run: &Run) -> (Rgba, Rgba) {
    let fg = resolve(run.fg, theme::text());
    let bg = resolve(run.bg, theme::bg_terminal());
    let (fg, bg) = if run.inverse { (bg, fg) } else { (fg, bg) };
    if run.cursor {
        (theme::on_accent(), theme::accent())
    } else {
        (fg, bg)
    }
}

/// Advance of `m` in the terminal font, and the line height.
fn measure(window: &Window, font_size: f32) -> (f32, f32) {
    let ts = window.text_system();
    let font = gpui::font(theme::FONT_MONO);
    let font_id = ts.resolve_font(&font);
    let width = ts
        .advance(font_id, px(font_size), 'm')
        .map(|s| f32::from(s.width))
        .unwrap_or(font_size * 0.6);
    (width, (font_size * 1.35).round())
}

impl Render for Terminal {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.cell = measure(window, self.font_size);
        let (cw, ch) = self.cell;
        let focused = self.focus.is_focused(window);
        let rows = {
            let p = lock(&self.parser);
            build_rows(p.screen(), focused)
        };
        let me = cx.entity();

        div()
            .id("terminal")
            .key_context("Terminal")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    window.focus(&this.focus, cx);
                    cx.notify();
                }),
            )
            .on_scroll_wheel(cx.listener(|this, ev: &ScrollWheelEvent, _, cx| {
                this.on_wheel(ev, cx);
                cx.stop_propagation();
            }))
            .flex_1()
            .min_w_0()
            .min_h_0()
            .p(px(8.0))
            .bg(theme::bg_terminal())
            .overflow_hidden()
            .font_family(theme::FONT_MONO)
            .text_size(px(self.font_size))
            .line_height(px(ch))
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_col()
                    .size_full()
                    .child(
                        canvas(
                            move |bounds, _, cx| {
                                me.update(cx, |t, cx| t.set_viewport(bounds.size, cx));
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .size_full(),
                    )
                    .children(rows.into_iter().map(move |runs| {
                        div()
                            .flex()
                            .flex_none()
                            .h(px(ch))
                            .children(runs.into_iter().map(move |run| {
                                let (fg, bg) = run_colors(&run);
                                div()
                                    .flex_none()
                                    .w(px(cw * run.cells as f32))
                                    .h(px(ch))
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .bg(bg)
                                    .text_color(fg)
                                    .when(run.bold, |s| s.font_weight(gpui::FontWeight::BOLD))
                                    .when(run.italic, |s| s.italic())
                                    .when(run.underline, |s| s.underline())
                                    .child(SharedString::from(run.text))
                            }))
                    })),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(s: &str, app: bool) -> Option<Vec<u8>> {
        key_bytes(&Keystroke::parse(s).unwrap(), app)
    }

    #[test]
    fn plain_and_control_keys() {
        assert_eq!(keys("enter", false), Some(b"\r".to_vec()));
        assert_eq!(keys("ctrl-c", false), Some(vec![3]));
        assert_eq!(keys("escape", false), Some(vec![0x1b]));
        assert_eq!(keys("backspace", false), Some(vec![0x7f]));
        assert_eq!(keys("shift-tab", false), Some(b"\x1b[Z".to_vec()));
        assert_eq!(keys("alt-enter", false), Some(b"\x1b\r".to_vec()));
    }

    #[test]
    fn arrows_follow_cursor_mode_and_modifiers() {
        assert_eq!(keys("up", false), Some(b"\x1b[A".to_vec()));
        assert_eq!(keys("up", true), Some(b"\x1bOA".to_vec()));
        assert_eq!(keys("ctrl-left", false), Some(b"\x1b[1;5D".to_vec()));
        assert_eq!(keys("delete", false), Some(b"\x1b[3~".to_vec()));
    }

    #[test]
    fn app_shortcuts_are_not_sent() {
        assert_eq!(keys("cmd-c", false), None);
    }

    #[test]
    fn printable_text_uses_key_char() {
        let mut ks = Keystroke::parse("a").unwrap();
        ks.key_char = Some("a".into());
        assert_eq!(key_bytes(&ks, false), Some(b"a".to_vec()));
        let mut alt = Keystroke::parse("alt-b").unwrap();
        alt.key_char = Some("b".into());
        assert_eq!(key_bytes(&alt, false), Some(b"\x1bb".to_vec()));
    }

    #[test]
    fn screen_text_is_grouped_into_styled_runs() {
        let mut p = vt100::Parser::new(2, 20, 0);
        p.process(b"hi \x1b[31mred\x1b[0m!");
        let rows = build_rows(p.screen(), false);
        let first = &rows[0];
        assert_eq!(first[0].text, "hi ");
        assert_eq!(first[1].text, "red");
        assert_eq!(first[1].fg, vt100::Color::Idx(1));
        assert_eq!(first[2].text.chars().next(), Some('!'));
        let total: u16 = first.iter().map(|r| r.cells).sum();
        assert_eq!(total, 20);
    }

    #[test]
    fn cursor_position_query_is_answered() {
        let parser = Mutex::new(vt100::Parser::new(5, 20, 0));
        let replies = process_with_replies(&parser, b"ab\x1b[6ncd");
        assert_eq!(replies, b"\x1b[1;3R");
        assert_eq!(lock(&parser).screen().contents(), "abcd");
    }

    #[test]
    fn palette_covers_cube_and_greys() {
        assert_eq!(indexed(16), rgb(0x000000));
        assert_eq!(indexed(231), rgb(0xffffff));
        assert_eq!(indexed(232), rgb(0x080808));
    }
}
