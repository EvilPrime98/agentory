//! Agent office: the agent sessions of one project as pixel-art workers in a top-down office.
//! Status sets the pose, the tool call in progress sets the action, a meter shows context use.
//! Clicking a worker opens a side panel to steer that session.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AppContext as _, Context, Entity, EventEmitter, InteractiveElement, IntoElement, ParentElement,
    Render, StatefulInteractiveElement, Styled, Subscription, Window, canvas, div, px,
};

use super::office_art::{self, Action, CELL_H, CELL_W, LABEL_Y, WorkerView};
use crate::backend::transcripts::{ChatItem, Tail};
use crate::i18n::t;
use crate::model::SessionStatus;
use crate::route::{Navigate, Route};
use crate::runtime;
use crate::state::{Stores, sessions};
use crate::ui::components::button::{ButtonVariant, button};
use crate::ui::components::scrollbar::Scrollbar;
use crate::ui::components::text_input::{TextInput, TextInputEvent};
use crate::ui::shell::sidebar::status_color;
use crate::ui::theme;

/// Fixed animation timestep (8 frames per second: it is pixel art).
const FRAME: Duration = Duration::from_millis(125);
/// Transcripts are tailed every this many frames.
const POLL_EVERY: u64 = 4;
const COLS: usize = 3;
const RECENT_KEPT: usize = 40;
const RECENT_SHOWN: usize = 14;
const PANEL_W: f32 = 340.0;

enum Line {
    User(String),
    Assistant(String),
    Tool { name: String, summary: String },
}

/// A tool call that has no result yet.
struct OpenTool {
    id: String,
    name: String,
}

/// Read-side state of one worker's transcript.
#[derive(Default)]
struct Feed {
    open: Vec<OpenTool>,
    recent: VecDeque<Line>,
}

impl Feed {
    fn apply(&mut self, item: ChatItem) {
        let line = match item {
            ChatItem::User(text) => {
                self.open.clear();
                Line::User(text)
            }
            ChatItem::Assistant(text) => Line::Assistant(text),
            ChatItem::ToolCall {
                id, name, summary, ..
            } => {
                self.open.push(OpenTool {
                    id,
                    name: name.clone(),
                });
                Line::Tool { name, summary }
            }
            ChatItem::ToolResult { id, .. } => {
                self.open.retain(|t| t.id != id);
                return;
            }
        };
        self.recent.push_back(line);
        if self.recent.len() > RECENT_KEPT {
            self.recent.pop_front();
        }
    }

    /// The latest tool call without a matching result.
    fn current_tool(&self) -> Option<&str> {
        self.open.last().map(|t| t.name.as_str())
    }

    fn reset(&mut self) {
        self.open.clear();
        self.recent.clear();
    }
}

struct Worker {
    tail: Arc<Mutex<Tail>>,
    feed: Feed,
}

/// What one render needs from a session.
struct Seat {
    id: String,
    name: String,
    status: SessionStatus,
    claude_id: Option<String>,
    cwd: PathBuf,
}

pub struct OfficePage {
    scrollbar: Scrollbar,
    activity_bar: Scrollbar,
    stores: Stores,
    project_id: String,
    frame: u64,
    workers: HashMap<String, Worker>,
    selected: Option<String>,
    input: Entity<TextInput>,
    polling: bool,
    _subs: Vec<Subscription>,
}

impl EventEmitter<Navigate> for OfficePage {}

impl OfficePage {
    pub fn new(stores: Stores, project_id: String, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| TextInput::new(t("office.prompt_placeholder"), cx));
        let subs = vec![
            cx.subscribe(&input, |this, _, ev: &TextInputEvent, cx| {
                if matches!(ev, TextInputEvent::Submit) {
                    this.send(cx);
                }
            }),
            cx.observe(&stores.sessions, |_, _, cx| cx.notify()),
            cx.observe(&stores.usage, |_, _, cx| cx.notify()),
        ];
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(FRAME).await;
                if this.update(cx, |this, cx| this.tick(cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        let mut this = Self {
            scrollbar: Scrollbar::new(),
            activity_bar: Scrollbar::new(),
            stores,
            project_id,
            frame: 0,
            workers: HashMap::new(),
            selected: None,
            input,
            polling: false,
            _subs: subs,
        };
        this.poll(cx);
        this
    }

    fn seats(&self, cx: &Context<Self>) -> Vec<Seat> {
        self.stores
            .sessions
            .read(cx)
            .agents(&self.project_id)
            .map(|s| Seat {
                id: s.id.clone(),
                name: s.name.clone(),
                status: s.status,
                claude_id: s.claude_id.clone(),
                cwd: s.cwd.clone(),
            })
            .collect()
    }

    fn tick(&mut self, cx: &mut Context<Self>) {
        self.frame += 1;
        if self.frame % POLL_EVERY == 0 {
            self.poll(cx);
        }
        cx.notify();
    }

    /// Tails the transcripts of the project's agents on the background runtime.
    fn poll(&mut self, cx: &mut Context<Self>) {
        if self.polling {
            return;
        }
        let seats = self.seats(cx);
        let alive: HashSet<&str> = seats.iter().map(|s| s.id.as_str()).collect();
        self.workers.retain(|id, _| alive.contains(id.as_str()));
        if self
            .selected
            .as_ref()
            .is_some_and(|id| !alive.contains(id.as_str()))
        {
            self.selected = None;
        }
        let mut jobs = Vec::new();
        for seat in &seats {
            let Some(claude_id) = &seat.claude_id else {
                continue;
            };
            let worker = self
                .workers
                .entry(seat.id.clone())
                .or_insert_with(|| Worker {
                    tail: Arc::new(Mutex::new(Tail::new(claude_id.clone()))),
                    feed: Feed::default(),
                });
            jobs.push((seat.id.clone(), worker.tail.clone()));
        }
        if jobs.is_empty() {
            return;
        }
        self.polling = true;
        cx.spawn(async move |this, cx| {
            let polled = runtime::run(async move {
                jobs.into_iter()
                    .filter_map(|(id, tail)| {
                        let mut tail = tail.lock().ok()?;
                        let items = tail.poll();
                        let reset = tail.take_reset();
                        Some((id, reset, items))
                    })
                    .collect::<Vec<_>>()
            })
            .await;
            this.update(cx, |this, cx| {
                this.polling = false;
                for (id, reset, items) in polled {
                    if let Some(w) = this.workers.get_mut(&id) {
                        if reset {
                            w.feed.reset();
                        }
                        for item in items {
                            w.feed.apply(item);
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn select(&mut self, id: &str, cx: &mut Context<Self>) {
        self.selected = (self.selected.as_deref() != Some(id)).then(|| id.to_string());
        self.input.update(cx, |i, cx| i.set_value("", cx));
        cx.notify();
    }

    fn send(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.selected.clone() else {
            return;
        };
        let text = self.input.read(cx).value().trim().to_string();
        if text.is_empty() {
            return;
        }
        self.stores
            .sessions
            .update(cx, |s, cx| s.submit(&id, &text, cx));
        self.input.update(cx, |i, cx| i.set_value("", cx));
    }

    /// Esc to the PTY, the same as pressing Esc in the terminal.
    fn interrupt(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.selected.clone() else {
            return;
        };
        let terminal = self
            .stores
            .sessions
            .read(cx)
            .get(&id)
            .map(|s| s.terminal.clone());
        if let Some(terminal) = terminal {
            terminal.update(cx, |t, _| t.send(b"\x1b"));
        }
    }

    fn resume(&mut self, seat: &Seat, cx: &mut Context<Self>) {
        let Some(claude_id) = &seat.claude_id else {
            return;
        };
        sessions::resume(claude_id, seat.cwd.clone(), &self.project_id, cx);
    }

    fn close(&mut self, id: &str, cx: &mut Context<Self>) {
        sessions::request_close(id, cx);
        self.selected = None;
        cx.notify();
    }

    fn open_terminal(&mut self, id: &str, cx: &mut Context<Self>) {
        self.stores.sessions.update(cx, |s, cx| s.select(id, cx));
        cx.emit(Navigate(Route::Sessions));
    }

    fn status_label(status: SessionStatus) -> String {
        t(match status {
            SessionStatus::Starting => "office.status.starting",
            SessionStatus::Working => "office.status.working",
            SessionStatus::NeedsInput => "office.status.needs_input",
            SessionStatus::Idle => "office.status.idle",
            SessionStatus::Exited => "office.status.exited",
        })
    }

    fn tool_of(&self, seat: &Seat) -> Option<String> {
        let tool = self.workers.get(&seat.id)?.feed.current_tool()?;
        (seat.status == SessionStatus::Working).then(|| tool.to_string())
    }

    fn scene(&self, seats: &[Seat], cx: &mut Context<Self>) -> gpui::AnyElement {
        let usage = self.stores.usage.read(cx);
        let views: Vec<WorkerView> = seats
            .iter()
            .map(|s| WorkerView {
                action: Action::of(s.status, self.tool_of(s).as_deref()),
                context_pct: usage
                    .sessions
                    .get(&s.id)
                    .map_or(0.0, |u| u.context_percent()),
            })
            .collect();
        let rows = seats.len().div_ceil(COLS).max(1);
        let frame = self.frame;

        let mut cells = Vec::new();
        for (i, seat) in seats.iter().enumerate() {
            let id = seat.id.clone();
            let selected = self.selected.as_deref() == Some(seat.id.as_str());
            let tool = self
                .tool_of(seat)
                .unwrap_or_else(|| Self::status_label(seat.status));
            cells.push(
                div()
                    .id(gpui::SharedString::from(format!("worker-{}", seat.id)))
                    .absolute()
                    .left(px((i % COLS) as f32 * CELL_W))
                    .top(px((i / COLS) as f32 * CELL_H))
                    .w(px(CELL_W))
                    .h(px(CELL_H))
                    .cursor_pointer()
                    .border_1()
                    .border_color(if selected {
                        theme::accent()
                    } else {
                        gpui::rgba(0x00000000)
                    })
                    .on_click(cx.listener(move |this, _, _, cx| this.select(&id, cx)))
                    .child(
                        div()
                            .absolute()
                            .top(px(LABEL_Y))
                            .left(px(16.0))
                            .w(px(CELL_W - 32.0))
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.0))
                                    .child(
                                        div()
                                            .flex_none()
                                            .size(px(8.0))
                                            .rounded_full()
                                            .bg(status_color(seat.status)),
                                    )
                                    .child(
                                        div()
                                            .min_w_0()
                                            .text_size(px(12.5))
                                            .text_color(theme::text())
                                            .truncate()
                                            .child(seat.name.clone()),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(theme::text_muted())
                                    .truncate()
                                    .child(tool),
                            ),
                    ),
            );
        }

        div()
            .relative()
            .w(px(COLS as f32 * CELL_W))
            .h(px(rows as f32 * CELL_H))
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, _| {
                        office_art::paint_scene(window, bounds.origin, COLS, &views, frame);
                    },
                )
                .absolute()
                .size_full(),
            )
            .children(cells)
            .into_any_element()
    }

    fn panel(&self, seat: &Seat, cx: &mut Context<Self>) -> gpui::AnyElement {
        let id = seat.id.clone();
        let feed = self.workers.get(&seat.id).map(|w| &w.feed);
        let lines: Vec<_> = feed
            .map(|f| f.recent.iter().rev().take(RECENT_SHOWN).collect::<Vec<_>>())
            .unwrap_or_default();
        let activity = div()
            .id("office-activity")
            .flex()
            .flex_col()
            .gap(px(6.0))
            .p(px(10.0))
            .overflow_y_scroll()
            .children(lines.into_iter().map(|line| {
                let (tag, text) = match line {
                    Line::User(s) => (t("office.line.you"), s.clone()),
                    Line::Assistant(s) => (t("office.line.agent"), s.clone()),
                    Line::Tool { name, summary } => (name.clone(), summary.clone()),
                };
                div()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .text_size(px(10.5))
                            .text_color(theme::text_faint())
                            .child(tag),
                    )
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(theme::text_secondary())
                            .max_h(px(54.0))
                            .overflow_hidden()
                            .child(text),
                    )
            }));

        let exited = seat.status == SessionStatus::Exited;
        let actions = div()
            .flex()
            .flex_wrap()
            .gap(px(8.0))
            .child(button(
                "office-open",
                t("office.open_terminal"),
                ButtonVariant::Secondary,
                {
                    let id = id.clone();
                    cx.listener(move |this, _, _, cx| this.open_terminal(&id, cx))
                },
            ))
            .when(!exited, |d| {
                d.child(button(
                    "office-interrupt",
                    t("office.interrupt"),
                    ButtonVariant::Secondary,
                    cx.listener(|this, _, _, cx| this.interrupt(cx)),
                ))
            })
            .when(exited, |d| {
                let seat = Seat {
                    id: seat.id.clone(),
                    name: seat.name.clone(),
                    status: seat.status,
                    claude_id: seat.claude_id.clone(),
                    cwd: seat.cwd.clone(),
                };
                d.child(button(
                    "office-resume",
                    t("office.resume"),
                    ButtonVariant::Classic,
                    cx.listener(move |this, _, _, cx| this.resume(&seat, cx)),
                ))
            })
            .child(button(
                "office-close",
                t("common.close"),
                ButtonVariant::Secondary,
                cx.listener(move |this, _, _, cx| this.close(&id, cx)),
            ));

        div()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(PANEL_W))
            .h_full()
            .gap(px(10.0))
            .p(px(14.0))
            .border_l_1()
            .border_color(theme::border_subtle())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .flex_none()
                            .size(px(8.0))
                            .rounded_full()
                            .bg(status_color(seat.status)),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .text_size(px(14.0))
                            .text_color(theme::text())
                            .truncate()
                            .child(seat.name.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(11.5))
                            .text_color(theme::text_muted())
                            .child(Self::status_label(seat.status)),
                    ),
            )
            .when(seat.status == SessionStatus::NeedsInput, |d| {
                d.child(
                    div()
                        .text_size(px(12.0))
                        .text_color(theme::warning())
                        .child(t("office.needs_input_hint")),
                )
            })
            .child(self.activity_bar.overlay(activity))
            .when(!exited, |d| {
                d.child(
                    div()
                        .flex_none()
                        .h(px(34.0))
                        .px(px(10.0))
                        .flex()
                        .items_center()
                        .rounded(px(4.0))
                        .border_1()
                        .border_color(theme::border_strong())
                        .child(self.input.clone()),
                )
            })
            .child(actions)
            .into_any_element()
    }
}

impl Render for OfficePage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let seats = self.seats(cx);
        let selected = self
            .selected
            .as_ref()
            .and_then(|id| seats.iter().find(|s| &s.id == id));
        let panel = match selected {
            Some(s) => Some(self.panel(s, cx)),
            None => None,
        };

        let body = if seats.is_empty() {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(13.0))
                .text_color(theme::text_muted())
                .child(t("office.empty"))
                .into_any_element()
        } else {
            let scene = self.scene(&seats, cx);
            self.scrollbar
                .overlay(
                    div()
                        .id("office-scroll")
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .min_h_0()
                        .overflow_y_scroll()
                        .items_center()
                        .p(px(24.0))
                        .child(scene),
                )
                .into_any_element()
        };

        div()
            .flex()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .child(body)
            .children(panel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(id: &str, name: &str) -> ChatItem {
        ChatItem::ToolCall {
            id: id.into(),
            name: name.into(),
            summary: String::new(),
            input: String::new(),
        }
    }

    fn result(id: &str) -> ChatItem {
        ChatItem::ToolResult {
            id: id.into(),
            text: String::new(),
            is_error: false,
        }
    }

    #[test]
    fn current_tool_is_the_latest_call_without_a_result() {
        let mut f = Feed::default();
        assert_eq!(f.current_tool(), None);
        f.apply(call("1", "Read"));
        f.apply(call("2", "Edit"));
        assert_eq!(f.current_tool(), Some("Edit"));
        f.apply(result("2"));
        assert_eq!(f.current_tool(), Some("Read"));
        f.apply(result("1"));
        assert_eq!(f.current_tool(), None);
    }

    #[test]
    fn a_new_prompt_drops_calls_that_never_returned() {
        let mut f = Feed::default();
        f.apply(call("1", "Bash"));
        f.apply(ChatItem::User("again".into()));
        assert_eq!(f.current_tool(), None);
    }

    #[test]
    fn recent_lines_are_capped() {
        let mut f = Feed::default();
        for i in 0..RECENT_KEPT + 10 {
            f.apply(ChatItem::Assistant(i.to_string()));
        }
        assert_eq!(f.recent.len(), RECENT_KEPT);
    }
}
