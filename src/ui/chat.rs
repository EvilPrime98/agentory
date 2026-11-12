
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    AnyElement, ClipboardItem, Context, CursorStyle, Entity, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, canvas, FocusHandle, Focusable, InteractiveElement, IntoElement,
    ParentElement, Render, StatefulInteractiveElement, Styled, Subscription, Task, Window, div,
    point, prelude::*, px,
};

use crate::backend::completions::{self, Candidate, Index, Trigger};
use crate::backend::transcripts::{ChatItem, Tail};
use crate::i18n::t;
use crate::model::SessionStatus;
use crate::state::Stores;
use crate::state::prefs::SessionView;
use crate::ui::components::button::{ButtonVariant, button};
use crate::ui::components::markdown::markdown_selectable;
use crate::ui::components::scrollbar::Scrollbar;
use crate::ui::components::selectable::{Pos, Registry, Sink, ordered};
use crate::ui::components::text_area::{TextArea, TextAreaEvent};
use crate::ui::icons::{Icon, icon};
use crate::ui::terminal::Terminal;
use crate::ui::theme;

const POLL: Duration = Duration::from_millis(400);
const MAX_RENDERED: usize = 150;
const COMPOSER_H: f32 = 84.0;
const GRAB_SLOP: f32 = 28.0;
const ESC: &[u8] = b"\x1b";

enum Entry {
    User(String),
    Assistant(String),
    Tool {
        id: String,
        name: String,
        summary: String,
        input: String,
        result: Option<(String, bool)>,
    },
}

#[derive(Debug, Clone, PartialEq)]
enum Prompt {
    Permission { question: String, detail: String },
    Other,
}

fn detect_prompt(screen: &str) -> Option<Prompt> {
    let lines: Vec<&str> = screen.lines().map(str::trim).collect();
    let strip = |l: &str| {
        l.trim_start_matches(['❯', '>', '│', '╭', '╰', ' '])
            .trim()
            .to_string()
    };
    let option_at = lines
        .iter()
        .position(|l| strip(l).starts_with("1. Yes"))?
        .min(lines.len());
    let question_at = lines[..option_at]
        .iter()
        .rposition(|l| l.contains("Do you want") || l.ends_with('?'));
    let Some(q) = question_at else {
        return Some(Prompt::Other);
    };
    if !lines[q].contains("Do you want") {
        return Some(Prompt::Other);
    }
    let detail = lines[..q]
        .iter()
        .rev()
        .map(|l| strip(l.trim_matches(|c: char| matches!(c, '─' | '│' | '╭' | '╮' | '╰' | '╯'))))
        .filter(|l| !l.is_empty() && !l.chars().all(|c| matches!(c, '─' | '-' | '═')))
        .take(6)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("\n");
    Some(Prompt::Permission {
        question: strip(lines[q]),
        detail,
    })
}

pub struct ChatView {
    stores: Stores,
    session_id: String,
    terminal: Entity<Terminal>,
    entries: Vec<Entry>,
    expanded: HashSet<String>,
    composer: Entity<TextArea>,
    scrollbar: Scrollbar,
    stick: u8,
    prompt: Option<Prompt>,
    index: Option<Arc<Index>>,
    menu_sel: usize,
    dismissed: Option<Trigger>,
    history: Option<usize>,
    blocks: Registry,
    selection: Option<(Pos, Pos)>,
    dragging: bool,
    pointer: gpui::Point<gpui::Pixels>,
    _task: Task<()>,
    _index_task: Task<()>,
    _subs: [Subscription; 2],
}

impl ChatView {
    pub fn new(
        stores: Stores,
        session_id: String,
        claude_id: String,
        terminal: Entity<Terminal>,
        cx: &mut Context<Self>,
    ) -> Self {
        let composer = cx.new(|cx| TextArea::new(cx).submit_on_enter());
        let sub = cx.subscribe(&composer, |this, _, ev: &TextAreaEvent, cx| match ev {
            TextAreaEvent::Submit => this.send(cx),
            TextAreaEvent::Changed => {
                this.menu_sel = 0;
                this.dismissed = None;
                this.history = None;
            }
            TextAreaEvent::Save => {}
        });
        let observe = cx.observe(&composer, |_, _, cx| cx.notify());
        let cwd = stores
            .sessions
            .read(cx)
            .get(&session_id)
            .map(|s| s.cwd.clone())
            .unwrap_or_default();
        let index_task = cx.spawn(async move |this, cx| {
            let index = cx
                .background_executor()
                .spawn(async move { Index::build(&cwd) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.index = Some(Arc::new(index));
                cx.notify();
            });
        });
        let task = cx.spawn(async move |this, cx| {
            let mut tail = Tail::new(claude_id);
            loop {
                let (back, items) = cx
                    .background_executor()
                    .spawn(async move {
                        let items = tail.poll();
                        (tail, items)
                    })
                    .await;
                tail = back;
                if this.update(cx, |this, cx| this.refresh(items, cx)).is_err() {
                    break;
                }
                cx.background_executor().timer(POLL).await;
            }
        });
        Self {
            stores,
            session_id,
            terminal,
            entries: Vec::new(),
            expanded: HashSet::new(),
            composer,
            scrollbar: Scrollbar::new(),
            stick: 3,
            prompt: None,
            index: None,
            menu_sel: 0,
            dismissed: None,
            history: None,
            blocks: Registry::default(),
            selection: None,
            dragging: false,
            pointer: point(px(0.0), px(0.0)),
            _index_task: index_task,
            _task: task,
            _subs: [sub, observe],
        }
    }

    fn status(&self, cx: &gpui::App) -> SessionStatus {
        self.stores
            .sessions
            .read(cx)
            .get(&self.session_id)
            .map_or(SessionStatus::Exited, |s| s.status)
    }

    fn refresh(&mut self, items: Vec<ChatItem>, cx: &mut Context<Self>) {
        let mut changed = !items.is_empty();
        for item in items {
            match item {
                ChatItem::User(text) => self.entries.push(Entry::User(text)),
                ChatItem::Assistant(text) => self.entries.push(Entry::Assistant(text)),
                ChatItem::ToolCall {
                    id,
                    name,
                    summary,
                    input,
                } => self.entries.push(Entry::Tool {
                    id,
                    name,
                    summary,
                    input,
                    result: None,
                }),
                ChatItem::ToolResult { id, text, is_error } => {
                    let call = self.entries.iter_mut().rev().find_map(|e| match e {
                        Entry::Tool { id: i, result, .. } if *i == id => Some(result),
                        _ => None,
                    });
                    if let Some(result) = call {
                        *result = Some((text, is_error));
                    }
                }
            }
        }
        let prompt = if self.status(cx) == SessionStatus::NeedsInput {
            detect_prompt(&self.terminal.read(cx).screen_text())
        } else {
            None
        };
        if prompt != self.prompt {
            self.prompt = prompt;
            changed = true;
        }
        if changed {
            self.stick = 3;
            cx.notify();
        }
    }

    fn menu(&self, cx: &gpui::App) -> Option<(Trigger, Vec<Candidate>)> {
        let trigger = completions::trigger(&self.composer.read(cx).text_before_caret())?;
        if self.dismissed.as_ref() == Some(&trigger) {
            return None;
        }
        let index = self.index.as_ref()?;
        let items = match &trigger {
            Trigger::File(q) => index.files(q),
            Trigger::Command(q) => index.commands(q),
        };
        (!items.is_empty()).then_some((trigger, items))
    }

    fn accept(&mut self, cx: &mut Context<Self>) {
        let Some((trigger, items)) = self.menu(cx) else {
            return;
        };
        let Some(item) = items.get(self.menu_sel.min(items.len() - 1)) else {
            return;
        };
        let end = if item.dir { "" } else { " " };
        let text = format!("{}{}{end}", trigger.sigil(), item.value);
        self.composer.update(cx, |c, cx| {
            c.replace_before_caret(trigger.token_len(), &text, cx)
        });
    }

    fn recall(&mut self, older: bool, cx: &mut Context<Self>) {
        let sent: Vec<&str> = self
            .entries
            .iter()
            .rev()
            .filter_map(|e| match e {
                Entry::User(t) => Some(t.as_str()),
                _ => None,
            })
            .collect();
        let text = match (self.history, older) {
            (None, true) => sent.first().map(|t| (0, *t)),
            (Some(i), true) => sent.get(i + 1).map(|t| (i + 1, *t)),
            (Some(0), false) | (None, false) => None,
            (Some(i), false) => sent.get(i - 1).map(|t| (i - 1, *t)),
        };
        let (pos, text) = match text {
            Some((i, t)) => (Some(i), t.to_string()),
            None if !older => (None, String::new()),
            None => return,
        };
        self.composer.update(cx, |c, cx| c.set_text(&text, cx));
        self.history = pos;
    }

    fn on_composer_key(&mut self, key: &str, shift: bool, cx: &mut Context<Self>) -> bool {
        if let Some((trigger, items)) = self.menu(cx) {
            let n = items.len();
            match key {
                "down" => self.menu_sel = (self.menu_sel + 1) % n,
                "up" => self.menu_sel = (self.menu_sel + n - 1) % n,
                "tab" | "enter" if !shift => self.accept(cx),
                "escape" => self.dismissed = Some(trigger),
                _ => return false,
            }
            cx.notify();
            return true;
        }
        let (empty, first, last) = {
            let c = self.composer.read(cx);
            (
                c.is_empty(),
                c.caret_on_first_line(),
                c.caret_on_last_line(),
            )
        };
        match key {
            "tab" if shift => {
                self.terminal.update(cx, |t, _| t.send(b"\x1b[Z"));
            }
            "up" if first && (empty || self.history.is_some()) => self.recall(true, cx),
            "down" if last && self.history.is_some() => self.recall(false, cx),
            _ => return false,
        }
        true
    }

    fn send(&mut self, cx: &mut Context<Self>) {
        let text = self.composer.read(cx).value();
        let mut text = text.trim().to_string();
        if text
            .rsplit(char::is_whitespace)
            .next()
            .is_some_and(|w| w.starts_with('@'))
        {
            text.push(' ');
        }
        let text = text.as_str();
        if text.trim().is_empty() || self.status(cx) == SessionStatus::Exited {
            return;
        }
        let id = self.session_id.clone();
        self.stores
            .sessions
            .update(cx, |s, cx| s.submit(&id, text, cx));
        self.composer.update(cx, |c, cx| c.set_value("", cx));
        self.history = None;
        self.stick = 3;
        cx.notify();
    }

    fn type_into_terminal(&mut self, bytes: &'static [u8], cx: &mut Context<Self>) {
        self.terminal.update(cx, |t, _| t.send(bytes));
        self.prompt = None;
        cx.notify();
    }

    fn interrupt(&mut self, cx: &mut Context<Self>) {
        if self.status(cx) == SessionStatus::Working {
            self.type_into_terminal(ESC, cx);
        }
    }

    fn autoscroll(&mut self, cx: &mut Context<Self>) {
        let view = self.scrollbar.handle().bounds();
        let y = self.pointer.y;
        let past = if y < view.top() {
            f32::from(view.top() - y)
        } else if y > view.bottom() {
            -f32::from(y - view.bottom())
        } else {
            return;
        };
        let step = (past.abs() * 10.0).clamp(80.0, 4000.0) / 60.0;
        let handle = self.scrollbar.handle();
        let max = f32::from(handle.max_offset().y);
        let mut offset = handle.offset();
        let now = f32::from(offset.y);
        let next = (now + past.signum() * step).clamp(-max, 0.0);
        if next != now {
            offset.y = px(next);
            handle.set_offset(offset);
        }
        if let Some(head) = self.blocks.hit(self.pointer, None)
            && let Some((anchor, _)) = self.selection
        {
            self.selection = Some((anchor, head));
        }
        cx.notify();
    }

    fn open_terminal(&mut self, cx: &mut Context<Self>) {
        let id = self.session_id.clone();
        self.stores
            .sessions
            .update(cx, |s, cx| s.set_view(&id, SessionView::Terminal, cx));
    }

    fn bubble(&self, entry: &Entry, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let sink = Sink::new(&self.blocks, ix, self.selection.map(|(a, b)| ordered(a, b)));
        match entry {
            Entry::User(text) => div()
                .flex()
                .justify_end()
                .child(
                    div()
                        .max_w(px(560.0))
                        .px(px(12.0))
                        .py(px(8.0))
                        .rounded(px(10.0))
                        .bg(theme::accent_soft())
                        .cursor(CursorStyle::IBeam)
                        .text_size(px(13.0))
                        .child(sink.text(text.clone(), Vec::new())),
                )
                .into_any_element(),
            Entry::Assistant(text) => div()
                .flex()
                .child(
                    div()
                        .w_full()
                        .min_w_0()
                        .px(px(12.0))
                        .py(px(8.0))
                        .rounded(px(10.0))
                        .bg(theme::bg_panel())
                        .cursor(CursorStyle::IBeam)
                        .border_1()
                        .border_color(theme::border_subtle())
                        .child(markdown_selectable(text, &sink)),
                )
                .into_any_element(),
            Entry::Tool {
                id,
                name,
                summary,
                input,
                result,
            } => {
                let open = self.expanded.contains(id);
                let toggle_id = id.clone();
                let status = match result {
                    None => t("chat.tool.running"),
                    Some((_, true)) => t("chat.tool.error"),
                    Some(_) => String::new(),
                };
                let status_color = match result {
                    Some((_, true)) => theme::error(),
                    _ => theme::text_muted(),
                };
                let mut card = div()
                    .flex()
                    .flex_col()
                    .max_w(px(680.0))
                    .rounded(px(8.0))
                    .border_1()
                    .border_color(theme::border_subtle())
                    .child(
                        div()
                            .id(("tool", ix))
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .px(px(10.0))
                            .py(px(6.0))
                            .cursor_pointer()
                            .hover(|s| s.bg(theme::hover_soft()))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !this.expanded.remove(&toggle_id) {
                                    this.expanded.insert(toggle_id.clone());
                                }
                                cx.notify();
                            }))
                            .child(
                                icon(
                                    if open {
                                        Icon::ChevronDown
                                    } else {
                                        Icon::ChevronRight
                                    },
                                    px(12.0),
                                )
                                .text_color(theme::text_muted()),
                            )
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child(name.clone()),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_size(px(12.0))
                                    .text_color(theme::text_secondary())
                                    .child(summary.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(status_color)
                                    .child(status),
                            ),
                    );
                if open {
                    card = card.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(8.0))
                            .p(px(10.0))
                            .border_t_1()
                            .border_color(theme::border_subtle())
                            .text_size(px(12.0))
                            .child(
                                div()
                                    .text_color(theme::text_secondary())
                                    .child(input.clone()),
                            )
                            .children(result.as_ref().map(|(text, is_error)| {
                                div()
                                    .text_color(if *is_error {
                                        theme::error()
                                    } else {
                                        theme::text()
                                    })
                                    .child(text.clone())
                            })),
                    );
                }
                div().flex().justify_start().child(card).into_any_element()
            }
        }
    }

    fn prompt_card(&self, prompt: &Prompt, cx: &mut Context<Self>) -> AnyElement {
        let card = div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .max_w(px(680.0))
            .p(px(12.0))
            .rounded(px(10.0))
            .border_1()
            .border_color(theme::warning());
        match prompt {
            Prompt::Permission { question, detail } => card
                .child(
                    div()
                        .text_size(px(13.0))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(t("chat.approval.title")),
                )
                .when(!detail.is_empty(), |c| {
                    c.child(
                        div()
                            .text_size(px(12.0))
                            .text_color(theme::text_secondary())
                            .child(detail.clone()),
                    )
                })
                .child(div().text_size(px(13.0)).child(question.clone()))
                .child(
                    div()
                        .flex()
                        .gap(px(8.0))
                        .child(button(
                            "chat-allow",
                            t("chat.approval.allow"),
                            ButtonVariant::Classic,
                            cx.listener(|this, _, _, cx| this.type_into_terminal(b"1", cx)),
                        ))
                        .child(button(
                            "chat-deny",
                            t("chat.approval.deny"),
                            ButtonVariant::Secondary,
                            cx.listener(|this, _, _, cx| this.type_into_terminal(ESC, cx)),
                        )),
                )
                .into_any_element(),
            Prompt::Other => card
                .child(
                    div()
                        .text_size(px(13.0))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(t("chat.dialog.title")),
                )
                .child(div().flex().child(button(
                    "chat-open-terminal",
                    t("chat.dialog.open_terminal"),
                    ButtonVariant::Classic,
                    cx.listener(|this, _, _, cx| this.open_terminal(cx)),
                )))
                .into_any_element(),
        }
    }
}

fn drag_tracker(
    dragging: bool,
    on_move: impl Fn(&MouseMoveEvent, &mut Window, &mut gpui::App) + 'static,
    on_up: impl Fn(&MouseUpEvent, &mut Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |_, _, window, _| {
            if dragging {
                window.set_window_cursor_style(CursorStyle::IBeam);
            }
            window.on_mouse_event(move |ev: &MouseMoveEvent, phase, window, cx| {
                if phase.bubble() {
                    on_move(ev, window, cx);
                }
            });
            window.on_mouse_event(move |ev: &MouseUpEvent, phase, window, cx| {
                if phase.bubble() {
                    on_up(ev, window, cx);
                }
            });
        },
    )
    .absolute()
    .size_full()
}

impl Focusable for ChatView {
    fn focus_handle(&self, cx: &gpui::App) -> FocusHandle {
        self.composer.read(cx).focus_handle(cx)
    }
}

impl Render for ChatView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.stick > 0 {
            self.stick -= 1;
            let handle = self.scrollbar.handle();
            let max = f32::from(handle.max_offset().y);
            handle.set_offset(point(px(0.0), px(-max)));
            cx.notify();
        }
        if self.dragging {
            self.autoscroll(cx);
        }
        self.blocks.clear();
        let status = self.status(cx);
        let working = status == SessionStatus::Working;
        let skip = self.entries.len().saturating_sub(MAX_RENDERED);
        let rows: Vec<AnyElement> = self
            .entries
            .iter()
            .enumerate()
            .skip(skip)
            .map(|(ix, e)| self.bubble(e, ix, cx))
            .collect();
        let prompt = self.prompt.clone().map(|p| self.prompt_card(&p, cx));

        let list = div()
            .id("chat-list")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .gap(px(10.0))
            .overflow_y_scroll()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                    this.pointer = ev.position;
                    this.selection = this.blocks.hit(ev.position, Some(GRAB_SLOP)).map(|p| (p, p));
                    this.dragging = this.selection.is_some();
                    cx.notify();
                }),
            )
            .p(px(16.0))
            .when(rows.is_empty(), |d| {
                d.child(
                    div()
                        .text_size(px(13.0))
                        .text_color(theme::text_muted())
                        .child(t("chat.empty")),
                )
            })
            .children(rows)
            .children(prompt)
            .when(working && self.prompt.is_none(), |d| {
                d.child(
                    div()
                        .text_size(px(12.0))
                        .text_color(theme::text_muted())
                        .child(t("chat.working")),
                )
            })
            .when(status == SessionStatus::Exited, |d| {
                d.child(
                    div()
                        .text_size(px(12.0))
                        .text_color(theme::text_muted())
                        .child(t("chat.exited")),
                )
            });

        let action = div()
            .id(if working { "chat-stop" } else { "chat-send" })
            .absolute()
            .right(px(8.0))
            .bottom(px(8.0))
            .size(px(28.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .cursor_pointer()
            .bg(if working {
                theme::hover_soft()
            } else {
                theme::accent()
            })
            .hover(|s| s.opacity(0.85))
            .on_click(cx.listener(move |this, _, _, cx| {
                if working {
                    this.interrupt(cx);
                } else {
                    this.send(cx);
                }
            }))
            .child(
                icon(if working { Icon::Close } else { Icon::ArrowUp }, px(16.0)).text_color(
                    if working {
                        theme::text()
                    } else {
                        theme::on_accent()
                    },
                ),
            );
        let empty = self.composer.read(cx).is_empty();
        let placeholder = empty.then(|| {
            div()
                .absolute()
                .top(px(10.0))
                .left(px(10.0))
                .text_size(px(13.0))
                .text_color(theme::text_faint())
                .child(t("chat.placeholder"))
        });

        let menu = self.menu(cx).map(|(trigger, items)| {
            let sel = self.menu_sel.min(items.len() - 1);
            let sigil = trigger.sigil();
            div()
                .absolute()
                .left(px(12.0))
                .bottom(px(COMPOSER_H + 24.0))
                .w(px(520.0))
                .max_w_full()
                .flex()
                .flex_col()
                .p(px(4.0))
                .rounded(px(8.0))
                .border_1()
                .border_color(theme::border_strong())
                .bg(theme::bg_modal())
                .occlude()
                .children(items.into_iter().enumerate().map(|(i, item)| {
                    let row = div()
                        .id(("candidate", i))
                        .flex()
                        .items_center()
                        .gap(px(10.0))
                        .px(px(8.0))
                        .py(px(5.0))
                        .rounded(px(4.0))
                        .text_size(px(12.0))
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.menu_sel = i;
                            this.accept(cx);
                            let handle = this.composer.read(cx).focus_handle(cx);
                            window.focus(&handle, cx);
                        }))
                        .child(
                            div()
                                .flex_none()
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .child(format!("{sigil}{}", item.value)),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_color(theme::text_muted())
                                .child(item.detail),
                        );
                    if i == sel {
                        row.bg(theme::accent_soft())
                    } else {
                        row.hover(|s| s.bg(theme::hover_soft()))
                    }
                }))
        });

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .bg(theme::bg_app())
            .child(self.scrollbar.overlay(list).child(drag_tracker(
                self.dragging,
                cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                    if !this.dragging {
                        return;
                    }
                    if ev.pressed_button != Some(MouseButton::Left) {
                        this.dragging = false;
                        cx.notify();
                        return;
                    }
                    this.pointer = ev.position;
                    if let Some(head) = this.blocks.hit(ev.position, None)
                        && let Some((anchor, _)) = this.selection
                    {
                        this.selection = Some((anchor, head));
                    }
                    cx.notify();
                }),
                cx.listener(|this, _: &MouseUpEvent, _, cx| {
                    if this.dragging {
                        this.dragging = false;
                        if this.selection.is_some_and(|(a, b)| a == b) {
                            this.selection = None;
                        }
                        cx.notify();
                    }
                }),
            )))
            .child(
                div()
                    .relative()
                    .flex()
                    .items_end()
                    .gap(px(8.0))
                    .p(px(12.0))
                    .border_t_1()
                    .border_color(theme::border_subtle())
                    .capture_key_down(cx.listener(|this, e: &gpui::KeyDownEvent, _, cx| {
                        let ks = &e.keystroke;
                        let chord = ks.modifiers.control || ks.modifiers.platform;
                        if chord
                            && ks.key == "c"
                            && !this.composer.read(cx).has_selection()
                            && let Some((a, b)) = this.selection.filter(|(a, b)| a != b)
                        {
                            cx.write_to_clipboard(ClipboardItem::new_string(this.blocks.copy(a, b)));
                            cx.stop_propagation();
                            return;
                        }
                        let plain = !ks.modifiers.control && !ks.modifiers.alt;
                        if plain && this.on_composer_key(&ks.key, ks.modifiers.shift, cx) {
                            cx.stop_propagation();
                        }
                    }))
                    .on_key_down(cx.listener(|this, e: &gpui::KeyDownEvent, _, cx| {
                        if e.keystroke.key == "escape" {
                            this.interrupt(cx);
                        }
                    }))
                    .child(
                        div()
                            .relative()
                            .flex_1()
                            .min_w_0()
                            .h(px(COMPOSER_H))
                            .rounded(px(8.0))
                            .border_1()
                            .border_color(theme::border_strong())
                            .bg(theme::bg_panel())
                            .overflow_hidden()
                            .child(div().size_full().pr(px(44.0)).child(self.composer.clone()))
                            .children(placeholder)
                            .child(action),
                    )
                    .children(menu),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_a_permission_prompt() {
        let screen = "╭──────────╮\n│ Bash command │\n│   cargo build │\n╰──────╯\n Do you want to proceed?\n ❯ 1. Yes\n   2. Yes, and don't ask again\n   3. No, and tell Claude what to do differently (esc)";
        match detect_prompt(screen) {
            Some(Prompt::Permission { question, detail }) => {
                assert_eq!(question, "Do you want to proceed?");
                assert!(detail.contains("cargo build"));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn other_dialogs_and_plain_screens() {
        assert_eq!(
            detect_prompt("Select a model?\n ❯ 1. Yes please\n 2. No"),
            Some(Prompt::Other)
        );
        assert_eq!(detect_prompt("> hello\n\nClaude is idle"), None);
    }
}
