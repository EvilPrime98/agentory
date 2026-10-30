//! Terminal widget: a PTY (ConPTY on Windows) feeding a `vt100` screen, drawn as a grid of styled
//! runs. Input is translated from GPUI key events to the byte sequences a terminal sends.
//!
//! Mouse: drag/double/triple-click select text anchored to the content (so it survives scrolling and
//! new output), programs that enable mouse tracking get the events instead unless Shift is held.
//!
//! Known gaps: no IME, wide glyphs are clipped to their cell width.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash as _, Hasher as _};
use std::io::Read as _;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use futures_channel::mpsc::unbounded;
use futures_util::StreamExt as _;
use gpui::{
    App, ClipboardItem, ColorExt, Context, EventEmitter, FocusHandle, Focusable,
    InteractiveElement, IntoElement, KeyDownEvent, Keystroke, Modifiers, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Point, Render, Rgba,
    ScrollWheelEvent, SharedString, Size, Styled, Window, canvas, div, prelude::*, px, rgb,
};

use crate::platform::pty::{Pty, Spawned};
use crate::ui::selection::{
    LineText, Pt, Selection, encode_mouse, scroll_distance, selected_text, word_bounds,
};
use crate::ui::theme;

const SCROLLBACK_LINES: usize = 5000;

/// Emitted once when the child process is gone.
pub struct TerminalExited;

/// Emitted when the program sets a new window title (OSC 0/2).
pub struct TerminalTitle(pub String);

enum Msg {
    Output,
    /// Answer to a terminal query found in the output (cursor position, device attributes).
    Reply(Vec<u8>),
    Exited,
}

const QUERIES: [&[u8]; 3] = [b"\x1b[6n", b"\x1b[c", b"\x1b[0c"];

/// Feed `data` to the screen and collect the replies a real terminal would send. ConPTY asks for
/// the cursor position on startup and holds the child's input until it gets an answer.
fn process_with_replies(
    parser: &Mutex<vt100::Parser>,
    scrolled: &AtomicU64,
    data: &[u8],
) -> Vec<u8> {
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
            feed(&mut lock(parser), scrolled, rest);
            return replies;
        };
        let end = at + query.len();
        let mut p = lock(parser);
        feed(&mut p, scrolled, &rest[..end]);
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

/// What a held mouse button is doing.
#[derive(Clone, Copy)]
enum Drag {
    /// Extending a selection from `anchor` (the word/line/cell first clicked).
    Select { anchor: Selection, unit: Unit },
    /// Moving the scrollbar thumb, grabbed `grab` px below its top.
    Scrollbar { grab: f32 },
    /// Forwarded to the program as mouse reports.
    Report(MouseButton),
}

#[derive(Clone, Copy, PartialEq)]
enum Unit {
    Cell,
    Word,
    Line,
}

pub struct Terminal {
    parser: Arc<Mutex<vt100::Parser>>,
    /// Rows that have scrolled into the history since the start. Gives selections a stable
    /// coordinate: the row at the top of the view is `scrolled - view_offset`.
    scrolled: Arc<AtomicU64>,
    pty: Option<Box<dyn Pty>>,
    rows: u16,
    cols: u16,
    /// Cell size in px, measured from the font on every render.
    cell: (f32, f32),
    font_size: f32,
    wheel_remainder: f32,
    /// Top-left of the cell grid in window coordinates, from the last layout.
    origin: Point<Pixels>,
    selection: Option<Selection>,
    drag: Option<Drag>,
    /// Last pointer position while selecting, for edge auto-scroll.
    pointer: Point<Pixels>,
    autoscrolling: bool,
    /// Cell last reported to a program that tracks mouse motion.
    reported: Option<(u16, u16)>,
    was_alt_screen: bool,
    /// Scrollbar thumb `(top, height)` and the track height, in px.
    thumb: (f32, f32),
    track_h: f32,
    pub exited: bool,
    title: String,
    focus: FocusHandle,
}

impl EventEmitter<TerminalExited> for Terminal {}
impl EventEmitter<TerminalTitle> for Terminal {}

fn lock(p: &Mutex<vt100::Parser>) -> MutexGuard<'_, vt100::Parser> {
    p.lock().unwrap_or_else(|e| e.into_inner())
}

/// Rows of history above the live screen. vt100 offers no accessor, but it clamps the view offset
/// to this length.
fn history_len(p: &mut vt100::Parser) -> usize {
    let view = p.screen().scrollback();
    p.set_scrollback(usize::MAX);
    let len = p.screen().scrollback();
    p.set_scrollback(view);
    len
}

fn live_hashes(p: &mut vt100::Parser) -> Vec<u64> {
    let view = p.screen().scrollback();
    p.set_scrollback(0);
    let cols = p.screen().size().1;
    let hashes = p
        .screen()
        .rows(0, cols)
        .map(|row| {
            let mut h = DefaultHasher::new();
            row.hash(&mut h);
            h.finish()
        })
        .collect();
    p.set_scrollback(view);
    hashes
}

/// Feed `data` to the screen and count the rows that scrolled into the history. Once the history
/// is full its length stops growing, so the shift is then recovered by comparing the live screen
/// before and after.
fn feed(p: &mut vt100::Parser, scrolled: &AtomicU64, data: &[u8]) {
    let before_len = history_len(p);
    let before = (before_len + data.len() >= SCROLLBACK_LINES).then(|| live_hashes(p));
    p.process(data);
    let after_len = history_len(p);
    let mut delta = after_len.saturating_sub(before_len);
    if after_len >= SCROLLBACK_LINES {
        if let Some(shift) = before.and_then(|b| scroll_distance(&b, &live_hashes(p))) {
            delta = delta.max(shift);
        }
    }
    if delta > 0 {
        scrolled.fetch_add(delta as u64, Ordering::Relaxed);
    }
}

/// Text of the buffer row with content coordinate `line`, or `None` once it has left the history.
/// Moves the view to read it and puts it back.
fn read_line(p: &mut vt100::Parser, scrolled: u64, line: u64) -> Option<LineText> {
    let len = history_len(p) as u64;
    let rows = u64::from(p.screen().size().0);
    let idx = line.checked_sub(scrolled.checked_sub(len)?)?;
    if idx >= len + rows {
        return None;
    }
    let view = p.screen().scrollback();
    let (offset, row) = if idx < len {
        (len - idx, 0)
    } else {
        (0, idx - len)
    };
    p.set_scrollback(offset as usize);
    let screen = p.screen();
    let cols = screen.size().1;
    let row = row as u16;
    let cells = (0..cols)
        .map(|c| match screen.cell(row, c) {
            Some(cell) if cell.is_wide_continuation() => String::new(),
            Some(cell) if cell.has_contents() => cell.contents(),
            _ => " ".to_string(),
        })
        .collect();
    let wrapped = screen.row_wrapped(row);
    p.set_scrollback(view);
    Some(LineText { cells, wrapped })
}

/// xterm modifier bits for a mouse report (Shift never gets here: it bypasses reporting).
fn mouse_modifier_bits(m: &Modifiers) -> u8 {
    8 * u8::from(m.alt) + 16 * u8::from(m.control)
}

fn mouse_button_code(button: MouseButton) -> Option<u8> {
    match button {
        MouseButton::Left => Some(0),
        MouseButton::Middle => Some(1),
        MouseButton::Right => Some(2),
        _ => None,
    }
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
        let scrolled = Arc::new(AtomicU64::new(0));
        let thread_scrolled = scrolled.clone();
        let mut reader = spawned.reader;
        let spawn_reader = std::thread::Builder::new()
            .name("agentory-pty-reader".into())
            .spawn(move || {
                let mut buf = [0u8; 16 * 1024];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            let replies =
                                process_with_replies(&thread_parser, &thread_scrolled, &buf[..n]);
                            if !replies.is_empty()
                                && tx.unbounded_send(Msg::Reply(replies)).is_err()
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
                        t.sync_alt_screen();
                        t.sync_title(cx);
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
            scrolled,
            pty: Some(spawned.pty),
            rows,
            cols,
            cell: (font_size * 0.6, (font_size * 1.35).round()),
            font_size,
            wheel_remainder: 0.0,
            origin: Point::default(),
            selection: None,
            drag: None,
            pointer: Point::default(),
            autoscrolling: false,
            reported: None,
            was_alt_screen: false,
            thumb: (0.0, 0.0),
            track_h: 0.0,
            exited: false,
            title: String::new(),
            focus: cx.focus_handle(),
        }
    }

    /// The alternate screen has no history, so a selection made on the other screen no longer
    /// points at anything.
    fn sync_alt_screen(&mut self) {
        let alt = lock(&self.parser).screen().alternate_screen();
        if alt != self.was_alt_screen {
            self.was_alt_screen = alt;
            self.selection = None;
            if matches!(self.drag, Some(Drag::Select { .. })) {
                self.drag = None;
            }
        }
    }

    fn sync_title(&mut self, cx: &mut Context<Self>) {
        let title = lock(&self.parser).screen().title().to_string();
        if title != self.title {
            self.title.clone_from(&title);
            cx.emit(TerminalTitle(title));
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
            // Only ever copies a selection, so it cannot clobber the clipboard by accident.
            self.copy_selection(cx);
            cx.stop_propagation();
            return;
        }
        if m.control && !m.shift && !m.alt && ks.key == "c" && self.selection.is_some() {
            // Ctrl+C copies while text is selected and interrupts otherwise.
            let copied = self.copy_selection(cx);
            self.selection = None;
            cx.notify();
            if copied {
                cx.stop_propagation();
                return;
            }
        }
        if m.shift && !m.control && !m.alt {
            let page = i64::from(self.rows.saturating_sub(1).max(1));
            let handled = match ks.key.as_str() {
                "pageup" => {
                    self.scroll_view(page);
                    true
                }
                "pagedown" => {
                    self.scroll_view(-page);
                    true
                }
                "home" => {
                    lock(&self.parser).set_scrollback(usize::MAX);
                    true
                }
                "end" => {
                    lock(&self.parser).set_scrollback(0);
                    true
                }
                _ => false,
            };
            if handled {
                cx.stop_propagation();
                cx.notify();
                return;
            }
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
        let (mode, encoding) = self.mouse_protocol();
        if mode != vt100::MouseProtocolMode::None && !ev.modifiers.shift {
            let (row, col) = self.grid_pos(ev.position);
            let button = if lines > 0.0 { 64 } else { 65 } + mouse_modifier_bits(&ev.modifiers);
            for _ in 0..(lines.abs() as usize).min(10) {
                if let Some(bytes) = encode_mouse(encoding, button, col, row, false) {
                    self.send(&bytes);
                }
            }
            return;
        }
        self.scroll_view(lines as i64);
        cx.notify();
    }

    /// Move the view `delta` rows into the history (negative: back towards the live screen).
    fn scroll_view(&mut self, delta: i64) {
        let mut p = lock(&self.parser);
        let current = p.screen().scrollback() as i64;
        p.set_scrollback((current + delta).max(0) as usize);
    }

    fn mouse_protocol(&self) -> (vt100::MouseProtocolMode, vt100::MouseProtocolEncoding) {
        let p = lock(&self.parser);
        (
            p.screen().mouse_protocol_mode(),
            p.screen().mouse_protocol_encoding(),
        )
    }

    /// Row and column under `pos`, clamped to the grid.
    fn grid_pos(&self, pos: Point<Pixels>) -> (u16, u16) {
        let x = f32::from(pos.x - self.origin.x) / self.cell.0;
        let y = f32::from(pos.y - self.origin.y) / self.cell.1;
        let col = (x.floor().max(0.0) as u16).min(self.cols.saturating_sub(1));
        let row = (y.floor().max(0.0) as u16).min(self.rows.saturating_sub(1));
        (row, col)
    }

    fn in_grid(&self, pos: Point<Pixels>) -> bool {
        let x = f32::from(pos.x - self.origin.x);
        let y = f32::from(pos.y - self.origin.y);
        x >= 0.0
            && y >= 0.0
            && x < self.cols as f32 * self.cell.0
            && y < self.rows as f32 * self.cell.1
    }

    /// Content coordinate of the cell under `pos`.
    fn point_at(&self, pos: Point<Pixels>) -> Pt {
        let (row, mut col) = self.grid_pos(pos);
        let p = lock(&self.parser);
        let screen = p.screen();
        if col > 0
            && screen
                .cell(row, col)
                .is_some_and(|c| c.is_wide_continuation())
        {
            col -= 1;
        }
        let top = self
            .scrolled
            .load(Ordering::Relaxed)
            .saturating_sub(screen.scrollback() as u64);
        Pt {
            line: top + u64::from(row),
            col,
        }
    }

    /// The cell, word or logical line containing `pt`.
    fn unit_at(&self, pt: Pt, unit: Unit) -> Selection {
        let cell = Selection::between(pt, pt);
        if unit == Unit::Cell {
            return cell;
        }
        let scrolled = self.scrolled.load(Ordering::Relaxed);
        let mut p = lock(&self.parser);
        let Some(here) = read_line(&mut p, scrolled, pt.line) else {
            return cell;
        };
        if unit == Unit::Word {
            let (lo, hi) = word_bounds(&here, pt.col);
            return Selection::between(
                Pt {
                    line: pt.line,
                    col: lo,
                },
                Pt {
                    line: pt.line,
                    col: hi,
                },
            );
        }
        // A logical line runs over every soft-wrapped row around it.
        let (mut first, mut last) = (pt.line, pt.line);
        while first > 0
            && pt.line - first < 200
            && read_line(&mut p, scrolled, first - 1).is_some_and(|l| l.wrapped)
        {
            first -= 1;
        }
        while last - pt.line < 200 && read_line(&mut p, scrolled, last).is_some_and(|l| l.wrapped) {
            last += 1;
        }
        let cols = p.screen().size().1;
        Selection::between(
            Pt {
                line: first,
                col: 0,
            },
            Pt {
                line: last,
                col: cols.saturating_sub(1),
            },
        )
    }

    fn selection_text(&self) -> Option<String> {
        let sel = self.selection?;
        let scrolled = self.scrolled.load(Ordering::Relaxed);
        let mut p = lock(&self.parser);
        let cols = p.screen().size().1;
        let rows: Vec<_> = (sel.start.line..=sel.end.line)
            .map(|line| {
                let range = sel.row_range(line, cols)?;
                read_line(&mut p, scrolled, line).map(|text| (text, range))
            })
            .collect();
        let text = selected_text(&rows);
        (!text.is_empty()).then_some(text)
    }

    fn copy_selection(&mut self, cx: &mut Context<Self>) -> bool {
        match self.selection_text() {
            Some(text) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                true
            }
            None => false,
        }
    }

    fn report_mouse(&mut self, button: u8, pos: Point<Pixels>, mods: &Modifiers, release: bool) {
        let (_, encoding) = self.mouse_protocol();
        let (row, col) = self.grid_pos(pos);
        if let Some(bytes) = encode_mouse(
            encoding,
            button + mouse_modifier_bits(mods),
            col,
            row,
            release,
        ) {
            self.send(&bytes);
        }
    }

    fn on_mouse_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        let (mode, _) = self.mouse_protocol();
        if mode != vt100::MouseProtocolMode::None && !ev.modifiers.shift {
            if let Some(code) = mouse_button_code(ev.button) {
                self.drag = Some(Drag::Report(ev.button));
                self.reported = Some(self.grid_pos(ev.position));
                self.report_mouse(code, ev.position, &ev.modifiers, false);
            }
            return;
        }
        if ev.button != MouseButton::Left {
            return;
        }
        let unit = match ev.click_count {
            0 | 1 => Unit::Cell,
            2 => Unit::Word,
            _ => Unit::Line,
        };
        let anchor = self.unit_at(self.point_at(ev.position), unit);
        self.selection = (unit != Unit::Cell).then_some(anchor);
        self.drag = Some(Drag::Select { anchor, unit });
        self.pointer = ev.position;
        cx.notify();
    }

    /// Pointer moves, delivered to the window so a drag keeps working outside the pane.
    fn on_mouse_move(&mut self, ev: &MouseMoveEvent, cx: &mut Context<Self>) {
        match self.drag {
            Some(Drag::Select { .. }) => {
                self.pointer = ev.position;
                self.update_selection();
                self.start_autoscroll(cx);
                cx.notify();
            }
            Some(Drag::Scrollbar { grab }) => {
                self.drag_scrollbar(f32::from(ev.position.y - self.origin.y), grab);
                cx.notify();
            }
            Some(Drag::Report(button)) => {
                let (mode, _) = self.mouse_protocol();
                if matches!(
                    mode,
                    vt100::MouseProtocolMode::ButtonMotion | vt100::MouseProtocolMode::AnyMotion
                ) {
                    self.report_motion(mouse_button_code(button).unwrap_or(0), ev);
                }
            }
            None => {
                let (mode, _) = self.mouse_protocol();
                if mode == vt100::MouseProtocolMode::AnyMotion
                    && !ev.modifiers.shift
                    && self.in_grid(ev.position)
                {
                    self.report_motion(3, ev);
                }
            }
        }
    }

    fn report_motion(&mut self, button: u8, ev: &MouseMoveEvent) {
        let at = self.grid_pos(ev.position);
        if self.reported != Some(at) {
            self.reported = Some(at);
            self.report_mouse(32 + button, ev.position, &ev.modifiers, false);
        }
    }

    fn on_mouse_up(&mut self, ev: &MouseUpEvent, cx: &mut Context<Self>) {
        match self.drag {
            Some(Drag::Report(button)) if button == ev.button => {
                self.drag = None;
                let (mode, _) = self.mouse_protocol();
                if mode != vt100::MouseProtocolMode::None && mode != vt100::MouseProtocolMode::Press
                {
                    if let Some(code) = mouse_button_code(button) {
                        self.report_mouse(code, ev.position, &ev.modifiers, true);
                    }
                }
            }
            Some(Drag::Select { .. } | Drag::Scrollbar { .. })
                if ev.button == MouseButton::Left =>
            {
                self.drag = None;
            }
            _ => {}
        }
        cx.notify();
    }

    /// Grow the selection to the unit under the pointer.
    fn update_selection(&mut self) {
        let Some(Drag::Select { anchor, unit }) = self.drag else {
            return;
        };
        let head = self.unit_at(self.point_at(self.pointer), unit);
        self.selection = if unit == Unit::Cell && head == anchor {
            None
        } else {
            Some(anchor.union(head))
        };
    }

    /// Rows per tick to scroll while the pointer is above (positive) or below (negative) the grid.
    fn edge_scroll(&self) -> i64 {
        let y = f32::from(self.pointer.y - self.origin.y);
        let height = self.rows as f32 * self.cell.1;
        let (past, sign) = if y < 0.0 {
            (-y, 1)
        } else if y >= height {
            (y - height, -1)
        } else {
            return 0;
        };
        sign * (1 + (past / self.cell.1) as i64).min(8)
    }

    fn start_autoscroll(&mut self, cx: &mut Context<Self>) {
        if self.autoscrolling || self.edge_scroll() == 0 {
            return;
        }
        self.autoscrolling = true;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(40))
                    .await;
                match this.update(cx, |t, cx| t.autoscroll_tick(cx)) {
                    Ok(true) => {}
                    _ => break,
                }
            }
        })
        .detach();
    }

    fn autoscroll_tick(&mut self, cx: &mut Context<Self>) -> bool {
        let step = self.edge_scroll();
        if step == 0 || !matches!(self.drag, Some(Drag::Select { .. })) {
            self.autoscrolling = false;
            return false;
        }
        self.scroll_view(step);
        self.update_selection();
        cx.notify();
        true
    }

    fn on_scrollbar_down(
        &mut self,
        ev: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus, cx);
        let y = f32::from(ev.position.y - self.origin.y);
        let (top, height) = self.thumb;
        let grab = if y >= top && y <= top + height {
            y - top
        } else {
            height / 2.0
        };
        self.drag = Some(Drag::Scrollbar { grab });
        self.drag_scrollbar(y, grab);
        cx.stop_propagation();
        cx.notify();
    }

    /// Put the thumb's top at `y - grab` (px from the track top) and scroll to match.
    fn drag_scrollbar(&mut self, y: f32, grab: f32) {
        let travel = (self.track_h - self.thumb.1).max(1.0);
        let frac = ((y - grab) / travel).clamp(0.0, 1.0);
        let mut p = lock(&self.parser);
        let len = history_len(&mut p);
        p.set_scrollback(((1.0 - frac) * len as f32).round() as usize);
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

    // Windows reports AltGr as Ctrl+Alt. When the layout produced a different printable
    // character (AltGr+2 -> `@`), send it as-is instead of a control code or ESC prefix.
    if m.control && m.alt {
        if let Some(text) = ks.key_char.as_deref() {
            if text != key && text.chars().all(|c| !c.is_control()) && !text.is_empty() {
                return Some(text.as_bytes().to_vec());
            }
        }
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

/// Runs never depend on the selection, so dragging one does not re-shape any text.
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

/// Opacity of the selection highlight laid over the text.
const SELECTION_ALPHA: f32 = 0.35;

/// `(x, y, width)` in pixels of the highlight of each selected row; `selection[r]` is the
/// inclusive column range of screen row `r`, if any.
fn selection_rects(selection: &[Option<(u16, u16)>], cw: f32, ch: f32) -> Vec<(f32, f32, f32)> {
    selection
        .iter()
        .enumerate()
        .filter_map(|(r, range)| {
            let (from, to) = (*range)?;
            Some((from as f32 * cw, r as f32 * ch, (to - from + 1) as f32 * cw))
        })
        .collect()
}

/// Advance of `m` in the terminal font, and the line height.
fn measure(window: &Window, font_size: f32) -> (f32, f32) {
    let ts = window.text_system();
    let font = gpui::font(theme::FONT);
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
        let (rows, selection, history, offset) = {
            let mut p = lock(&self.parser);
            let history = history_len(&mut p);
            let offset = p.screen().scrollback();
            let (screen_rows, cols) = p.screen().size();
            let top = self
                .scrolled
                .load(Ordering::Relaxed)
                .saturating_sub(offset as u64);
            let selection: Vec<_> = (0..u64::from(screen_rows))
                .map(|r| self.selection.and_then(|s| s.row_range(top + r, cols)))
                .collect();
            (build_rows(p.screen(), focused), selection, history, offset)
        };

        // Scrollbar thumb: sized by the share of the buffer on screen, parked by the view offset.
        self.track_h = self.rows as f32 * ch;
        let show_bar = history > 0;
        let thumb_h = (self.track_h * self.rows as f32 / (history + usize::from(self.rows)) as f32)
            .clamp(24.0_f32.min(self.track_h), self.track_h);
        let thumb_top = (self.track_h - thumb_h) * (1.0 - offset as f32 / history.max(1) as f32);
        self.thumb = if show_bar {
            (thumb_top, thumb_h)
        } else {
            (0.0, 0.0)
        };
        let highlights = selection_rects(&selection, cw, ch);
        let dragging_bar = matches!(self.drag, Some(Drag::Scrollbar { .. }));

        let me = cx.entity();
        let (move_me, up_me) = (me.clone(), me.clone());

        div()
            .id("terminal")
            .key_context("Terminal")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_down(MouseButton::Middle, cx.listener(Self::on_mouse_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::on_mouse_down))
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
            .font_family(theme::FONT)
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
                                me.update(cx, |t, cx| {
                                    t.origin = bounds.origin;
                                    t.set_viewport(bounds.size, cx);
                                });
                            },
                            // Window-level listeners, so drags keep tracking outside the pane.
                            move |_, _, window, _| {
                                let me = move_me.clone();
                                window.on_mouse_event(move |ev: &MouseMoveEvent, phase, _, cx| {
                                    if phase.bubble() {
                                        me.update(cx, |t, cx| t.on_mouse_move(ev, cx));
                                    }
                                });
                                let me = up_me.clone();
                                window.on_mouse_event(move |ev: &MouseUpEvent, phase, _, cx| {
                                    if phase.bubble() {
                                        me.update(cx, |t, cx| t.on_mouse_up(ev, cx));
                                    }
                                });
                            },
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
                    }))
                    .children(highlights.into_iter().map(|(x, y, w)| {
                        div()
                            .absolute()
                            .left(px(x))
                            .top(px(y))
                            .w(px(w))
                            .h(px(ch))
                            .bg(theme::accent().opacity(SELECTION_ALPHA))
                    }))
                    .when(show_bar, |s| {
                        s.child(
                            div()
                                .id("terminal-scrollbar")
                                .absolute()
                                .top_0()
                                .bottom_0()
                                .right(px(-8.0))
                                .w(px(8.0))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(Self::on_scrollbar_down),
                                )
                                .child(
                                    div()
                                        .id("terminal-scrollbar-thumb")
                                        .absolute()
                                        .top(px(thumb_top))
                                        .left(px(2.0))
                                        .w(px(4.0))
                                        .h(px(thumb_h))
                                        .rounded(px(2.0))
                                        .bg(if dragging_bar {
                                            theme::accent()
                                        } else {
                                            theme::text_muted().opacity(0.35)
                                        })
                                        .hover(|s| s.bg(theme::text_muted().opacity(0.6))),
                                ),
                        )
                    }),
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
    fn altgr_sends_the_layout_character() {
        for (key, ch) in [("2", "@"), ("\\", "\\"), ("4", "{"), ("[", "["), ("]", "]")] {
            let mut ks = Keystroke::parse(&format!("ctrl-alt-{key}")).unwrap();
            ks.key_char = Some(ch.into());
            // A character equal to `key` is not distinguishable from a real chord.
            let expected = (ch != key).then(|| ch.as_bytes().to_vec());
            if let Some(e) = expected {
                assert_eq!(key_bytes(&ks, false), Some(e), "AltGr+{key}");
            }
        }
        let mut ks = Keystroke::parse("ctrl-alt-4").unwrap();
        ks.key_char = Some("{".into());
        assert_eq!(key_bytes(&ks, false), Some(b"{".to_vec()));
    }

    #[test]
    fn ctrl_alt_chords_are_unchanged() {
        assert_eq!(keys("ctrl-alt-c", false), Some(vec![3]));
        let mut ks = Keystroke::parse("ctrl-alt-c").unwrap();
        ks.key_char = Some("c".into());
        assert_eq!(key_bytes(&ks, false), Some(vec![3]));
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
    fn selection_becomes_one_rect_per_selected_row() {
        let rects = selection_rects(&[None, Some((2, 4)), Some((0, 0))], 10.0, 20.0);
        assert_eq!(rects, vec![(20.0, 20.0, 30.0), (0.0, 40.0, 10.0)]);
    }

    #[test]
    fn cursor_position_query_is_answered() {
        let parser = Mutex::new(vt100::Parser::new(5, 20, 0));
        let replies = process_with_replies(&parser, &AtomicU64::new(0), b"ab\x1b[6ncd");
        assert_eq!(replies, b"\x1b[1;3R");
        assert_eq!(lock(&parser).screen().contents(), "abcd");
    }

    fn row_text(line: &LineText) -> String {
        line.cells.concat().trim_end().to_string()
    }

    /// Print `count` numbered lines in chunks of `chunk` lines, as a PTY would deliver them.
    fn print_lines(p: &mut vt100::Parser, scrolled: &AtomicU64, count: usize, chunk: usize) {
        for start in (0..count).step_by(chunk) {
            let data: String = (start..(start + chunk).min(count))
                .map(|n| format!("row{n}\r\n"))
                .collect();
            feed(p, scrolled, data.as_bytes());
        }
    }

    #[test]
    fn deep_history_is_readable_and_counted() {
        let mut p = vt100::Parser::new(4, 12, SCROLLBACK_LINES);
        let scrolled = AtomicU64::new(0);
        print_lines(&mut p, &scrolled, 100, 7);
        // 100 lines plus the cursor row on a 4-row screen: 97 rows went into the history.
        assert_eq!(scrolled.load(Ordering::Relaxed), 97);
        assert_eq!(history_len(&mut p), 97);
        let s = scrolled.load(Ordering::Relaxed);
        // Rows far deeper than one screen, read without disturbing the view.
        p.set_scrollback(5);
        for n in [0u64, 13, 50, 96, 98] {
            let line = read_line(&mut p, s, n).unwrap();
            assert_eq!(row_text(&line), format!("row{n}"));
        }
        assert_eq!(p.screen().scrollback(), 5);
        assert!(read_line(&mut p, s, 104).is_none());
    }

    #[test]
    fn rows_stay_addressable_once_the_history_is_full() {
        let mut p = vt100::Parser::new(24, 20, SCROLLBACK_LINES);
        let scrolled = AtomicU64::new(0);
        print_lines(&mut p, &scrolled, 7000, 5);
        let s = scrolled.load(Ordering::Relaxed);
        assert_eq!(history_len(&mut p), SCROLLBACK_LINES);
        assert_eq!(s, 7000 + 1 - 24);
        for n in [6000u64, 6975, 2500 + 1] {
            let line = read_line(&mut p, s, n).unwrap();
            assert_eq!(row_text(&line), format!("row{n}"));
        }
        // Row 100 has long since been dropped from the 5000-row history.
        assert!(read_line(&mut p, s, 100).is_none());
    }

    #[test]
    fn wrapped_rows_are_flagged() {
        let mut p = vt100::Parser::new(3, 5, SCROLLBACK_LINES);
        let scrolled = AtomicU64::new(0);
        feed(&mut p, &scrolled, b"abcdefgh\r\nxy");
        let first = read_line(&mut p, 0, 0).unwrap();
        assert!(first.wrapped);
        assert_eq!(row_text(&first), "abcde");
        assert!(!read_line(&mut p, 0, 1).unwrap().wrapped);
    }

    #[test]
    fn palette_covers_cube_and_greys() {
        assert_eq!(indexed(16), rgb(0x000000));
        assert_eq!(indexed(231), rgb(0xffffff));
        assert_eq!(indexed(232), rgb(0x080808));
    }
}
