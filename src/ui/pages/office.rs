//! Agent office: the agent sessions of one project as pixel-art workers in a top-down office.
//! Status sets the pose, the tool call in progress sets the action, a meter shows context use.
//! Clicking a worker opens a side panel to steer that session.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AppContext as _, Context, Entity, EventEmitter, InteractiveElement, IntoElement, ParentElement,
    Render, StatefulInteractiveElement, Styled, Subscription, Window, canvas, div, px,
};

use super::office_art::{self, Action, CELL_H, CELL_W, LABEL_Y, WorkerView};
use crate::backend::gh;
use crate::backend::spm::store::parse_skill;
use crate::backend::tasks::{self, Plan, Task, TaskStatus};
use crate::backend::transcripts::{ChatItem, Tail};
use crate::i18n::{t, t_args};
use crate::model::{SessionStatus, new_id};
use crate::route::{Navigate, Route};
use crate::runtime;
use crate::state::dispatch::{FOREMAN_PREFIX, foreman_prompt};
use crate::state::sessions::SessionKind;
use crate::state::{Stores, sessions};
use crate::ui::components::button::{ButtonVariant, button};
use crate::ui::components::scrollbar::Scrollbar;
use crate::ui::components::text_input::{TextInput, TextInputEvent};
use crate::ui::shell::sidebar::status_color;
use crate::ui::{theme, toast};

/// Fixed animation timestep (8 frames per second: it is pixel art).
const FRAME: Duration = Duration::from_millis(125);
/// Transcripts are tailed every this many frames.
const POLL_EVERY: u64 = 4;
const PR_POLL: Duration = Duration::from_secs(30);
const COLS: usize = 3;
const RECENT_KEPT: usize = 40;
const RECENT_SHOWN: usize = 14;
const PANEL_W: f32 = 340.0;
const BOARD_SHOWN: usize = 6;

/// A skill chip being dragged from the tray onto a worker.
#[derive(Clone)]
struct DraggedSkill(String);

struct SkillGhost(String);

impl Render for SkillGhost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        skill_chip(self.0.clone()).opacity(0.9)
    }
}

fn skill_chip(name: String) -> gpui::Div {
    div()
        .px(px(10.0))
        .h(px(26.0))
        .flex()
        .items_center()
        .rounded(px(13.0))
        .border_1()
        .border_color(theme::border_strong())
        .bg(theme::bg_modal())
        .text_size(px(12.0))
        .text_color(theme::text())
        .child(name)
}

/// The prompt a worker receives when a skill is dropped on it. The judgment is the session's.
fn skill_prompt(name: &str, description: &str, hint: &str) -> String {
    let hint = if hint.is_empty() {
        String::new()
    } else {
        format!(" Arguments: {hint}.")
    };
    format!(
        "The user dropped the skill /{name} on you. Description: {description}.{hint}          Judge whether it fits what you are doing and this project. If it fits, run /{name}.          If it does not, say why and do not run it. If it fits but needs a flag or argument          you were not given (for example --apply), ask permission first."
    )
}

/// The markdown files a foreman can plan from: those in the project root and in `docs/`.
fn markdown_files(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for dir in [root.to_path_buf(), root.join("docs")] {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for path in entries.flatten().map(|e| e.path()) {
            let md = path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("md"));
            if md && path.is_file() {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

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
    claude_id: String,
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

/// What the right-hand panel shows for the foreman.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ForemanPanel {
    /// Choose the issue or document to plan.
    Pick,
    /// Review the draft tasks, then approve or reject them.
    Review,
}

pub struct OfficePage {
    foreman_bar: Scrollbar,
    foreman: Option<ForemanPanel>,
    plan: Plan,
    /// The user's headcount; the proposal is used until it is changed.
    headcount: Option<u32>,
    /// Open issues of the project; `None` while loading.
    issues: Option<Vec<gh::IssueSummary>>,
    docs: Vec<PathBuf>,
    scrollbar: Scrollbar,
    activity_bar: Scrollbar,
    stores: Stores,
    project_id: String,
    frame: u64,
    workers: HashMap<String, Worker>,
    selected: Option<String>,
    input: Entity<TextInput>,
    polling: bool,
    tasks: Vec<Task>,
    /// Open PRs opened by finished workers, waiting for the user's review.
    prs: Vec<gh::PrSummary>,
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
            foreman_bar: Scrollbar::new(),
            foreman: None,
            plan: Plan::default(),
            headcount: None,
            issues: None,
            docs: Vec::new(),
            scrollbar: Scrollbar::new(),
            activity_bar: Scrollbar::new(),
            stores,
            project_id,
            frame: 0,
            workers: HashMap::new(),
            selected: None,
            input,
            polling: false,
            tasks: Vec::new(),
            prs: Vec::new(),
            _subs: subs,
        };
        this.poll(cx);
        this.load_tasks(cx);
        this.load_prs(cx);
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(PR_POLL).await;
                if this.update(cx, |this, cx| this.load_prs(cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        this
    }

    /// Lists the worker PRs still open, on the background runtime. `gh` is a network call, so
    /// this runs far less often than the board read.
    fn load_prs(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.stores.projects.read(cx).path_of(&self.project_id) else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let loaded = runtime::run(async move {
                if gh::setup(&root).await != gh::Setup::Ready {
                    return Vec::new();
                }
                gh::pr_list(&root)
                    .await
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|p| p.state == "OPEN" && p.head_ref_name.starts_with("agentory/w"))
                    .collect::<Vec<_>>()
            })
            .await;
            this.update(cx, |this, cx| {
                if this.prs != loaded {
                    this.prs = loaded;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn done_tray(&self) -> gpui::AnyElement {
        let rows = self.prs.iter().map(|pr| {
            let url = pr.url.clone();
            div()
                .id(gpui::SharedString::from(format!("pr-{}", pr.number)))
                .flex()
                .gap(px(8.0))
                .text_size(px(12.0))
                .text_color(theme::text())
                .cursor_pointer()
                .on_click(move |_, _, cx| cx.open_url(&url))
                .child(
                    div()
                        .text_color(theme::text_faint())
                        .child(format!("#{}", pr.number)),
                )
                .child(div().flex_1().min_w_0().truncate().child(pr.title.clone()))
        });
        div()
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .px(px(24.0))
            .py(px(10.0))
            .border_t_1()
            .border_color(theme::border_subtle())
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(theme::text_faint())
                    .child(crate::i18n::t_args(
                        "office.done",
                        &[("count", &self.prs.len().to_string())],
                    )),
            )
            .children(rows)
            .into_any_element()
    }

    fn seats(&self, cx: &Context<Self>) -> Vec<Seat> {
        self.stores
            .sessions
            .read(cx)
            .agents(&self.project_id)
            .filter(|s| !s.id.starts_with(FOREMAN_PREFIX))
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
            self.load_tasks(cx);
        }
        let animating = self
            .stores
            .sessions
            .read(cx)
            .agents(&self.project_id)
            .any(|s| s.status != SessionStatus::Exited);
        if animating {
            cx.notify();
        }
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
                    claude_id: claude_id.clone(),
                    tail: Arc::new(Mutex::new(Tail::new(claude_id.clone()))),
                    feed: Feed::default(),
                });
            if worker.claude_id != *claude_id {
                *worker = Worker {
                    claude_id: claude_id.clone(),
                    tail: Arc::new(Mutex::new(Tail::new(claude_id.clone()))),
                    feed: Feed::default(),
                };
            }
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
        self.foreman = None;
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
        if sessions::resume(claude_id, seat.cwd.clone(), &self.project_id, cx) {
            cx.emit(Navigate(Route::Sessions));
        }
    }

    fn close(&mut self, id: &str, cx: &mut Context<Self>) {
        sessions::request_close(id, cx);
        self.selected = None;
        cx.notify();
    }

    fn drop_skill(&mut self, seat_id: &str, name: &str, cx: &mut Context<Self>) {
        let skills = self.stores.skills.read(cx);
        let mut dirs: Vec<PathBuf> = Vec::new();
        if let Some(p) = &skills.project {
            dirs.push(p.join(".claude").join("skills"));
        }
        if let Some(h) = dirs::home_dir() {
            dirs.push(h.join(".claude").join("skills"));
        }
        let parsed = dirs
            .iter()
            .find_map(|d| std::fs::read_to_string(d.join(name).join("SKILL.md")).ok())
            .map(|text| parse_skill(&text))
            .unwrap_or_default();
        let text = skill_prompt(name, &parsed.description, &parsed.argument_hint);
        self.stores
            .sessions
            .update(cx, |s, cx| s.submit(seat_id, &text, cx));
    }

    /// Reads the project's task board on the background runtime.
    fn load_tasks(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.stores.projects.read(cx).path_of(&self.project_id) else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let (loaded, plan) =
                runtime::run(async move { (tasks::load(&root), tasks::load_plan(&root)) }).await;
            this.update(cx, |this, cx| {
                if this.tasks != loaded || this.plan != plan {
                    this.tasks = loaded;
                    this.plan = plan;
                    if this.foreman == Some(ForemanPanel::Review) && this.drafts().next().is_none()
                    {
                        this.foreman = None;
                    }
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn board(&self, seats: &[Seat]) -> gpui::AnyElement {
        let shown: Vec<&Task> = self
            .tasks
            .iter()
            .filter(|t| t.status != TaskStatus::Draft)
            .collect();
        let open = shown
            .iter()
            .filter(|t| t.status == TaskStatus::Open)
            .count();
        let rows = shown.iter().rev().take(BOARD_SHOWN).map(|task| {
            let (mark, color) = match task.status {
                TaskStatus::Draft => ("◌", theme::text_faint()),
                TaskStatus::Open => ("○", theme::text_muted()),
                TaskStatus::Claimed => ("●", theme::text()),
                TaskStatus::Done => ("✓", theme::text_faint()),
            };
            let who = task.assignee.as_deref().map(|a| {
                seats
                    .iter()
                    .find(|s| s.id == a)
                    .map_or_else(|| a.to_string(), |s| s.name.clone())
            });
            div()
                .flex()
                .gap(px(8.0))
                .text_size(px(12.0))
                .text_color(color)
                .child(mark)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child(task.title.clone()),
                )
                .children(who.map(|w| div().text_color(theme::text_faint()).child(w)))
        });
        div()
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .px(px(24.0))
            .py(px(10.0))
            .border_t_1()
            .border_color(theme::border_subtle())
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(theme::text_faint())
                    .child(crate::i18n::t_args(
                        "office.board",
                        &[
                            ("open", &open.to_string()),
                            ("total", &shown.len().to_string()),
                        ],
                    )),
            )
            .children(rows)
            .into_any_element()
    }

    fn drafts(&self) -> impl Iterator<Item = &Task> {
        self.tasks.iter().filter(|t| t.status == TaskStatus::Draft)
    }

    /// The status of the foreman's session, while one is running.
    fn foreman_status(&self, cx: &Context<Self>) -> Option<SessionStatus> {
        self.stores
            .sessions
            .read(cx)
            .agents(&self.project_id)
            .find(|s| s.id.starts_with(FOREMAN_PREFIX))
            .map(|s| s.status)
    }

    fn worker_cap(&self, cx: &Context<Self>) -> u32 {
        self.stores.prefs.read(cx).prefs.max_workers.max(1)
    }

    fn headcount(&self, cap: u32) -> u32 {
        let proposed = tasks::proposed_headcount(cap, &self.plan, &self.tasks);
        self.headcount.unwrap_or(proposed).clamp(1, cap)
    }

    fn step_headcount(&mut self, delta: i32, cx: &mut Context<Self>) {
        let cap = self.worker_cap(cx);
        let next = (self.headcount(cap) as i32 + delta).clamp(1, cap as i32);
        self.headcount = Some(next as u32);
        cx.notify();
    }

    fn click_desk(&mut self, cx: &mut Context<Self>) {
        if self.foreman_status(cx).is_some() {
            return;
        }
        self.selected = None;
        if self.foreman.take().is_some() {
            cx.notify();
            return;
        }
        if self.drafts().next().is_some() {
            self.foreman = Some(ForemanPanel::Review);
        } else {
            self.foreman = Some(ForemanPanel::Pick);
            self.open_picker(cx);
        }
        cx.notify();
    }

    fn open_picker(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.stores.projects.read(cx).path_of(&self.project_id) else {
            return;
        };
        self.docs = markdown_files(&root);
        self.issues = None;
        cx.spawn(async move |this, cx| {
            let issues = runtime::run(async move {
                if gh::setup(&root).await != gh::Setup::Ready {
                    return Vec::new();
                }
                gh::issue_list(&root)
                    .await
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|i| i.state == "OPEN")
                    .collect::<Vec<_>>()
            })
            .await;
            this.update(cx, |this, cx| {
                this.issues = Some(issues);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Starts a foreman that plans the work in the markdown file at `input`.
    fn start_foreman(&mut self, input: &Path, cx: &mut Context<Self>) {
        let Some(project) = self.stores.projects.read(cx).get(&self.project_id).cloned() else {
            return;
        };
        if let Err(e) = tasks::clear_plan(&project.path) {
            toast::error(
                cx,
                t_args("office.foreman.failed", &[("error", &format!("{e:?}"))]),
            );
            return;
        }
        let prefs = self.stores.prefs.read(cx).prefs.clone();
        let prompt = foreman_prompt(&input.to_string_lossy());
        let sid = new_id(FOREMAN_PREFIX);
        let started = self.stores.sessions.update(cx, |s, cx| {
            s.start(
                &project,
                sid,
                None,
                SessionKind::Agent,
                &prefs,
                Some(prompt),
                cx,
            )
        });
        match started {
            Ok(()) => {
                self.foreman = None;
                self.headcount = None;
            }
            Err(e) => toast::error(cx, t_args("office.foreman.failed", &[("error", &e)])),
        }
        cx.notify();
    }

    fn plan_issue(&mut self, number: u64, cx: &mut Context<Self>) {
        let Some(root) = self.stores.projects.read(cx).path_of(&self.project_id) else {
            return;
        };
        let project_id = self.project_id.clone();
        cx.spawn(async move |this, cx| {
            let written = runtime::run(async move {
                let issue = gh::issue_view(&root, number).await?;
                let dir = std::env::temp_dir().join("agentory").join("foreman");
                std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
                let path = dir.join(format!("{project_id}-{number}.md"));
                let text = format!("# {} (#{})\n\n{}\n", issue.title, issue.number, issue.body);
                std::fs::write(&path, text).map_err(|e| e.to_string())?;
                Ok::<_, String>(path)
            })
            .await;
            this.update(cx, |this, cx| match written {
                Ok(path) => this.start_foreman(&path, cx),
                Err(e) => toast::error(cx, t_args("office.foreman.failed", &[("error", &e)])),
            })
            .ok();
        })
        .detach();
    }

    /// Ends the review: the drafts are opened (`approve`) or deleted.
    fn resolve_plan(&mut self, approve: bool, cx: &mut Context<Self>) {
        let Some(root) = self.stores.projects.read(cx).path_of(&self.project_id) else {
            return;
        };
        let done = if approve {
            let n = self.headcount(self.worker_cap(cx));
            tasks::approve(&root, n).map(|_| ())
        } else {
            tasks::discard(&root).map(|_| ())
        };
        if let Err(e) = done {
            toast::error(cx, format!("{e:?}"));
            return;
        }
        self.tasks = tasks::load(&root);
        self.plan = tasks::load_plan(&root);
        self.foreman = None;
        self.headcount = None;
        cx.notify();
    }

    fn foreman_desk(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let status = self.foreman_status(cx);
        let busy = status.is_some();
        let drafts = self.drafts().count();
        let label = if busy {
            t("office.foreman.drafting")
        } else if drafts > 0 {
            t_args("office.foreman.review", &[("count", &drafts.to_string())])
        } else {
            t("office.foreman.idle")
        };
        let view = WorkerView {
            action: Action::of(status.unwrap_or(SessionStatus::Idle), None),
            context_pct: 0.0,
        };
        let frame = self.frame;
        div()
            .id("foreman-desk")
            .flex_none()
            .flex()
            .items_center()
            .gap(px(16.0))
            .px(px(24.0))
            .py(px(10.0))
            .border_b_1()
            .border_color(theme::border_subtle())
            .when(busy, |d| d.opacity(0.7))
            .when(!busy, |d| {
                d.cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| this.click_desk(cx)))
            })
            .child(
                div()
                    .relative()
                    .flex_none()
                    .w(px(CELL_W))
                    .h(px(CELL_H))
                    .child(
                        canvas(
                            |_, _, _| (),
                            move |bounds, _, window, _| {
                                office_art::paint_scene(window, bounds.origin, 1, &[view], frame);
                            },
                        )
                        .absolute()
                        .size_full(),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .min_w_0()
                    .child(
                        div()
                            .px(px(8.0))
                            .py(px(2.0))
                            .rounded(px(4.0))
                            .border_1()
                            .border_color(theme::accent())
                            .text_size(px(11.0))
                            .text_color(theme::accent())
                            .child(t("office.foreman.title")),
                    )
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(theme::text_muted())
                            .child(label),
                    ),
            )
            .into_any_element()
    }

    fn foreman_shell() -> gpui::Div {
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
    }

    fn foreman_pick(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let heading = |key: &str| {
            div()
                .text_size(px(11.0))
                .text_color(theme::text_faint())
                .child(t(key))
        };
        let note = |key: &str| {
            div()
                .text_size(px(12.0))
                .text_color(theme::text_muted())
                .child(t(key))
        };
        let row = |id: gpui::SharedString, number: String, name: String| {
            div()
                .id(id)
                .flex()
                .gap(px(8.0))
                .text_size(px(12.0))
                .text_color(theme::text())
                .cursor_pointer()
                .child(div().text_color(theme::text_faint()).child(number))
                .child(div().flex_1().min_w_0().truncate().child(name))
        };
        let root = self.stores.projects.read(cx).path_of(&self.project_id);
        let mut list = div()
            .id("foreman-pick")
            .flex()
            .flex_col()
            .gap(px(6.0))
            .p(px(10.0))
            .overflow_y_scroll()
            .child(heading("office.foreman.pick_issue"));
        match &self.issues {
            None => list = list.child(note("office.foreman.loading")),
            Some(issues) if issues.is_empty() => {
                list = list.child(note("office.foreman.no_issues"));
            }
            Some(issues) => {
                for issue in issues {
                    let number = issue.number;
                    list = list.child(
                        row(
                            format!("foreman-issue-{number}").into(),
                            format!("#{number}"),
                            issue.title.clone(),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| this.plan_issue(number, cx))),
                    );
                }
            }
        }
        list = list.child(heading("office.foreman.pick_doc"));
        if self.docs.is_empty() {
            list = list.child(note("office.foreman.no_docs"));
        }
        for (i, doc) in self.docs.iter().enumerate() {
            let shown = root
                .as_deref()
                .and_then(|r| doc.strip_prefix(r).ok())
                .unwrap_or(doc)
                .to_string_lossy()
                .into_owned();
            let doc = doc.clone();
            list = list.child(
                row(format!("foreman-doc-{i}").into(), "▤".into(), shown)
                    .on_click(cx.listener(move |this, _, _, cx| this.start_foreman(&doc, cx))),
            );
        }
        Self::foreman_shell()
            .child(
                div()
                    .text_size(px(14.0))
                    .text_color(theme::text())
                    .child(t("office.foreman.title")),
            )
            .child(self.foreman_bar.overlay(list))
            .into_any_element()
    }

    fn foreman_review(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let cap = self.worker_cap(cx);
        let count = self.headcount(cap);
        let list = div()
            .id("foreman-drafts")
            .flex()
            .flex_col()
            .gap(px(8.0))
            .p(px(10.0))
            .overflow_y_scroll()
            .children(self.drafts().map(|task| {
                div()
                    .flex()
                    .gap(px(8.0))
                    .text_size(px(12.0))
                    .text_color(theme::text())
                    .child(div().flex_1().min_w_0().child(task.title.clone()))
                    .child(div().flex_none().text_color(theme::text_faint()).child(t(
                        if task.write {
                            "office.foreman.write"
                        } else {
                            "office.foreman.read"
                        },
                    )))
            }));
        let stepper = div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(
                div()
                    .flex_1()
                    .text_size(px(12.0))
                    .text_color(theme::text_muted())
                    .child(t("office.foreman.workers")),
            )
            .child(button(
                "foreman-fewer",
                "−",
                ButtonVariant::Secondary,
                cx.listener(|this, _, _, cx| this.step_headcount(-1, cx)),
            ))
            .child(
                div()
                    .w(px(20.0))
                    .flex()
                    .justify_center()
                    .text_size(px(13.0))
                    .text_color(theme::text())
                    .child(count.to_string()),
            )
            .child(button(
                "foreman-more",
                "+",
                ButtonVariant::Secondary,
                cx.listener(|this, _, _, cx| this.step_headcount(1, cx)),
            ));
        let actions = div()
            .flex()
            .gap(px(8.0))
            .child(button(
                "foreman-approve",
                t("office.foreman.approve"),
                ButtonVariant::Classic,
                cx.listener(|this, _, _, cx| this.resolve_plan(true, cx)),
            ))
            .child(button(
                "foreman-reject",
                t("office.foreman.reject"),
                ButtonVariant::Secondary,
                cx.listener(|this, _, _, cx| this.resolve_plan(false, cx)),
            ));
        Self::foreman_shell()
            .child(
                div()
                    .text_size(px(14.0))
                    .text_color(theme::text())
                    .child(t("office.foreman.plan")),
            )
            .child(self.foreman_bar.overlay(list))
            .child(stepper)
            .child(actions)
            .into_any_element()
    }

    fn skill_tray(&self, cx: &Context<Self>) -> gpui::AnyElement {
        let names: Vec<String> = self
            .stores
            .skills
            .read(cx)
            .snapshot
            .installed
            .iter()
            .cloned()
            .collect();
        div()
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .px(px(24.0))
            .py(px(10.0))
            .border_t_1()
            .border_color(theme::border_subtle())
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(theme::text_faint())
                    .child(t("office.skills_hint")),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(6.0))
                    .children(names.into_iter().map(|name| {
                        let drag = DraggedSkill(name.clone());
                        skill_chip(name.clone())
                            .id(gpui::SharedString::from(format!("skill-{name}")))
                            .cursor_grab()
                            .on_drag(drag, |d, _, _, cx| cx.new(|_| SkillGhost(d.0.clone())))
                    })),
            )
            .into_any_element()
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
            let drop_id = seat.id.clone();
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
                    .drag_over::<DraggedSkill>(|s, _, _, _| s.border_color(theme::accent()))
                    .on_drop(cx.listener(move |this, d: &DraggedSkill, _, cx| {
                        this.drop_skill(&drop_id, &d.0, cx)
                    }))
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
                                            .text_size(px(13.0))
                                            .font_weight(gpui::FontWeight::SEMIBOLD)
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
        let panel = match (self.foreman, selected) {
            (Some(ForemanPanel::Pick), _) => Some(self.foreman_pick(cx)),
            (Some(ForemanPanel::Review), _) => Some(self.foreman_review(cx)),
            (None, Some(s)) => Some(self.panel(s, cx)),
            (None, None) => None,
        };
        let desk = self.foreman_desk(cx);

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
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .child(desk)
                    .child(body)
                    .when(
                        self.tasks.iter().any(|t| t.status != TaskStatus::Draft),
                        |d| d.child(self.board(&seats)),
                    )
                    .when(!self.prs.is_empty(), |d| d.child(self.done_tray()))
                    .when(!seats.is_empty(), |d| d.child(self.skill_tray(cx))),
            )
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
    fn skill_prompt_names_the_skill_and_asks_for_judgment() {
        let p = skill_prompt("review", "Review a diff", "[--apply]");
        assert!(p.contains("/review") && p.contains("Review a diff") && p.contains("[--apply]"));
        assert!(!p.contains('\n'));
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
