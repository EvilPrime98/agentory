//! Agent office: the agent sessions of one project as pixel-art workers in a top-down office.
//! Status sets the pose, the tool call in progress sets the action, a meter shows context use.
//! Clicking a worker opens a side panel to steer that session.

use std::cell::Cell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AppContext as _, Context, Entity, EventEmitter, InteractiveElement, IntoElement, ParentElement,
    Render, StatefulInteractiveElement, Styled, Subscription, Window, canvas, div, px,
};

use super::office_anim::{DUR, SCENE_PAD, Tween, cols_for, ease_out, fade};
use super::office_art::{self, Action, CELL_H, CELL_W, LABEL_Y, Placed, WorkerView};
use crate::backend::gh;
use crate::backend::spm::store::parse_skill;
use crate::backend::tasks::{self, Plan, Task, TaskStatus};
use crate::backend::transcripts::{ChatItem, Tail};
use crate::i18n::{t, t_args};
use crate::model::{Folder, Project, SessionStatus, new_id};
use crate::route::{Navigate, Route};
use crate::runtime;
use crate::state::dispatch::{FOREMAN_PREFIX, foreman_prompt};
use crate::state::plan as flow;
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
const RECENT_KEPT: usize = 40;
const RECENT_SHOWN: usize = 14;
const PANEL_W: f32 = 340.0;
const BOARD_SHOWN: usize = 8;
const DIFF_LINES: usize = 400;
const SECTIONS: usize = 3;
const BOARD: usize = 0;
const DONE: usize = 1;
const ROW_H: f32 = 24.0;
const FOREMAN: &str = "foreman";
const ENTER_DROP: f32 = 36.0;

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

fn foreman_plan_files(root: &Path, docs: &str) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for dir in [
        root.to_path_buf(),
        root.join(PathBuf::from_iter(docs.split('/'))),
    ] {
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

fn seat_of(s: &sessions::Session) -> Seat {
    Seat {
        id: s.id.clone(),
        name: s.name.clone(),
        status: s.status,
        claude_id: s.claude_id.clone(),
        cwd: s.cwd.clone(),
    }
}

/// What the right-hand panel shows besides a worker's session.
#[derive(Clone, PartialEq, Eq)]
enum ForemanPanel {
    /// Choose the issue or document to plan.
    Pick,
    /// The live foreman session: the user and the foreman plan together.
    Planner,
    /// Review the plan: the draft tasks, their dependencies and the headcount.
    Review,
    /// Review the result of this task: accept it or send it back with feedback.
    Task(String),
}

pub struct OfficePage {
    foreman_bar: Scrollbar,
    foreman: Option<ForemanPanel>,
    task_diff: Option<(String, String)>,
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
    /// Open PRs of finished plans, waiting for the user's review.
    prs: Vec<gh::PrSummary>,
    scene_w: Rc<Cell<f32>>,
    rail_bar: Scrollbar,
    glides: HashMap<String, (Tween, Tween)>,
    sel_fx: HashMap<String, Tween>,
    ctx_fx: HashMap<String, Tween>,
    panel_fx: Tween,
    open: [bool; SECTIONS],
    section_fx: [Tween; SECTIONS],
    rows: HashMap<String, (String, Instant)>,
    rows_primed: bool,
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
            task_diff: None,
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
            scene_w: Rc::new(Cell::new(0.0)),
            rail_bar: Scrollbar::new(),
            glides: HashMap::new(),
            sel_fx: HashMap::new(),
            ctx_fx: HashMap::new(),
            panel_fx: Tween::at(0.0, Instant::now()),
            open: [true; SECTIONS],
            section_fx: [Tween::at(1.0, Instant::now()); SECTIONS],
            rows: HashMap::new(),
            rows_primed: false,
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
                    .filter(|p| p.state == "OPEN" && p.head_ref_name.starts_with("agentory/plan-"))
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

    fn row_opacity(&self, key: &str, now: Instant) -> f32 {
        self.rows.get(key).map_or(1.0, |(_, since)| {
            let t = now.saturating_duration_since(*since).as_secs_f32() / DUR.as_secs_f32();
            0.3 + 0.7 * ease_out(t)
        })
    }

    fn done_body(&self, now: Instant) -> gpui::AnyElement {
        let rows = self.prs.iter().map(|pr| {
            let url = pr.url.clone();
            div()
                .id(gpui::SharedString::from(format!("pr-{}", pr.number)))
                .flex()
                .items_center()
                .gap(px(8.0))
                .px(px(6.0))
                .h(px(ROW_H))
                .rounded(px(4.0))
                .text_size(px(12.0))
                .text_color(theme::text())
                .opacity(self.row_opacity(&format!("pr-{}", pr.number), now))
                .cursor_pointer()
                .hover(|d| d.bg(theme::hover_soft()))
                .on_click(move |_, _, cx| cx.open_url(&url))
                .child(
                    div()
                        .text_color(theme::text_faint())
                        .child(format!("#{}", pr.number)),
                )
                .child(div().flex_1().min_w_0().truncate().child(pr.title.clone()))
        });
        div()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .px(px(8.0))
            .pb(px(8.0))
            .children(rows)
            .into_any_element()
    }

    fn seats(&self, cx: &Context<Self>) -> Vec<Seat> {
        self.stores
            .sessions
            .read(cx)
            .agents(&self.project_id)
            .filter(|s| !s.id.starts_with(FOREMAN_PREFIX))
            .map(seat_of)
            .collect()
    }

    /// The foreman's seat, while its session lives.
    fn foreman_seat(&self, cx: &Context<Self>) -> Option<Seat> {
        self.stores
            .sessions
            .read(cx)
            .agents(&self.project_id)
            .find(|s| s.id.starts_with(FOREMAN_PREFIX))
            .map(seat_of)
    }

    fn tick(&mut self, cx: &mut Context<Self>) {
        self.frame += 1;
        if self.frame % POLL_EVERY == 0 {
            self.poll(cx);
            self.load_tasks(cx);
        }
        if self.foreman == Some(ForemanPanel::Planner) && self.foreman_seat(cx).is_none() {
            self.foreman = None;
            self.selected = None;
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
        let mut seats = self.seats(cx);
        seats.extend(self.foreman_seat(cx));
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
        let text = self.input.read(cx).value().trim().to_string();
        if let Some(ForemanPanel::Task(task)) = self.foreman.clone() {
            if !text.is_empty() {
                self.send_back_task(&task, &text, cx);
            }
            return;
        }
        let Some(id) = self.selected.clone() else {
            return;
        };
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
        if id.starts_with(FOREMAN_PREFIX) {
            self.foreman = None;
        }
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
                    let was_proposed = this.proposed();
                    this.tasks = loaded;
                    this.plan = plan;
                    this.follow_plan(was_proposed, cx);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Moves the foreman panel along with the plan: the proposal opens the review, a withdrawn
    /// proposal goes back to the chat, and a task that left review closes its panel.
    fn follow_plan(&mut self, was_proposed: bool, cx: &mut Context<Self>) {
        match self.foreman.clone() {
            Some(ForemanPanel::Planner) if !was_proposed && self.proposed() => {
                self.foreman = Some(ForemanPanel::Review);
                self.selected = None;
            }
            Some(ForemanPanel::Review) if !self.proposed() => match self.foreman_seat(cx) {
                Some(seat) => {
                    self.foreman = Some(ForemanPanel::Planner);
                    self.selected = Some(seat.id);
                }
                None if self.drafts().next().is_none() => self.foreman = None,
                None => {}
            },
            Some(ForemanPanel::Task(id)) => {
                let waiting = self
                    .tasks
                    .iter()
                    .any(|t| t.id == id && t.status == TaskStatus::Review);
                if !waiting {
                    self.foreman = None;
                }
            }
            _ => {}
        }
    }

    fn board_body(&self, seats: &[Seat], now: Instant, cx: &mut Context<Self>) -> gpui::AnyElement {
        let mut shown: Vec<&Task> = self
            .tasks
            .iter()
            .filter(|t| t.status != TaskStatus::Draft)
            .rev()
            .collect();
        // Results waiting for the user come first, so they are never pushed off the list.
        shown.sort_by_key(|t| t.status != TaskStatus::Review);
        let rows = shown.into_iter().take(BOARD_SHOWN).map(|task| {
            let (mark, color) = match task.status {
                TaskStatus::Draft => ("◌", theme::text_faint()),
                TaskStatus::Open if !task.ready(&self.tasks) => ("◌", theme::text_faint()),
                TaskStatus::Open => ("○", theme::text_muted()),
                TaskStatus::Claimed => ("●", theme::text()),
                TaskStatus::Review => ("◐", theme::warning()),
                TaskStatus::Done => ("✓", theme::text_faint()),
            };
            let review = task.status == TaskStatus::Review;
            let id = task.id.clone();
            let who = task.assignee.as_deref().map(|a| {
                seats
                    .iter()
                    .find(|s| s.id == a)
                    .map_or_else(|| a.to_string(), |s| s.name.clone())
            });
            div()
                .id(gpui::SharedString::from(format!("board-{}", task.id)))
                .flex()
                .items_center()
                .gap(px(8.0))
                .px(px(6.0))
                .h(px(ROW_H))
                .rounded(px(4.0))
                .text_size(px(12.0))
                .text_color(color)
                .opacity(self.row_opacity(&format!("task-{}", task.id), now))
                .when(review, |d| {
                    d.cursor_pointer()
                        .hover(|d| d.bg(theme::hover_soft()))
                        .on_click(cx.listener(move |this, _, _, cx| this.open_task(&id, cx)))
                })
                .child(div().flex_none().w(px(12.0)).child(mark))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child(task.title.clone()),
                )
                .children(who.map(|w| div().flex_none().text_color(theme::text_faint()).child(w)))
        });
        div()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .px(px(8.0))
            .pb(px(8.0))
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

    /// The foreman proposed a complete plan, waiting for the user.
    fn proposed(&self) -> bool {
        self.plan.headcount.is_some() && self.drafts().next().is_some()
    }

    fn click_desk(&mut self, cx: &mut Context<Self>) {
        self.selected = None;
        if self.foreman.take().is_some() {
            cx.notify();
            return;
        }
        self.foreman = Some(match self.foreman_seat(cx) {
            Some(_) if self.proposed() => ForemanPanel::Review,
            Some(seat) => {
                self.selected = Some(seat.id);
                ForemanPanel::Planner
            }
            None if self.drafts().next().is_some() => ForemanPanel::Review,
            None => {
                self.open_picker(cx);
                ForemanPanel::Pick
            }
        });
        cx.notify();
    }

    /// Back to the chat with the foreman, to change the plan.
    fn discuss(&mut self, cx: &mut Context<Self>) {
        let Some(seat) = self.foreman_seat(cx) else {
            return;
        };
        self.selected = Some(seat.id);
        self.foreman = Some(ForemanPanel::Planner);
        cx.notify();
    }

    fn drop_draft(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(root) = self.stores.projects.read(cx).path_of(&self.project_id) else {
            return;
        };
        let was_proposed = self.proposed();
        if let Err(e) = tasks::drop_draft(&root, id) {
            toast::error(cx, format!("{e:?}"));
            return;
        }
        self.tasks = tasks::load(&root);
        self.plan = tasks::load_plan(&root);
        self.follow_plan(was_proposed, cx);
        cx.notify();
    }

    fn open_task(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(project) = self.stores.projects.read(cx).get(&self.project_id).cloned() else {
            return;
        };
        let Some(task) = self.tasks.iter().find(|t| t.id == id).cloned() else {
            return;
        };
        self.selected = None;
        self.foreman = Some(ForemanPanel::Task(id.to_string()));
        self.task_diff = None;
        self.input.update(cx, |i, cx| i.set_value("", cx));
        cx.notify();
        let id = id.to_string();
        cx.spawn(async move |this, cx| {
            let text =
                runtime::run(
                    async move { flow::diff(&project, &task).await.unwrap_or_else(|e| e) },
                )
                .await;
            this.update(cx, |this, cx| {
                this.task_diff = Some((id, text));
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn task_under_review(&self, id: &str, cx: &Context<Self>) -> Option<(Project, Task)> {
        let project = self
            .stores
            .projects
            .read(cx)
            .get(&self.project_id)
            .cloned()?;
        let task = self.tasks.iter().find(|t| t.id == id).cloned()?;
        Some((project, task))
    }

    fn accept_task(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some((project, task)) = self.task_under_review(id, cx) else {
            return;
        };
        flow::accept(&self.stores, &project, task, cx);
        self.foreman = None;
        cx.notify();
    }

    fn send_back_task(&mut self, id: &str, feedback: &str, cx: &mut Context<Self>) {
        let Some((project, task)) = self.task_under_review(id, cx) else {
            return;
        };
        flow::send_back(&self.stores, &project, &task, feedback, cx);
        self.input.update(cx, |i, cx| i.set_value("", cx));
        self.foreman = None;
        cx.notify();
    }

    fn open_picker(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.stores.projects.read(cx).path_of(&self.project_id) else {
            return;
        };
        let docs = self.stores.folder(&self.project_id, Folder::Docs, cx);
        self.docs = foreman_plan_files(&root, &docs);
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
        let seat = sid.clone();
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
                self.foreman = Some(ForemanPanel::Planner);
                self.selected = Some(seat);
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
        self.selected = None;
        self.headcount = None;
        // The plan is settled either way, so the foreman's job is over.
        if let Some(seat) = self.foreman_seat(cx) {
            sessions::request_close(&seat.id, cx);
        }
        cx.notify();
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
                .pt(px(8.0))
                .pb(px(2.0))
                .px(px(6.0))
                .text_size(px(11.0))
                .text_color(theme::text_muted())
                .child(t(key))
        };
        let note = |key: &str| {
            div()
                .px(px(6.0))
                .py(px(2.0))
                .text_size(px(12.0))
                .text_color(theme::text_faint())
                .child(t(key))
        };
        let row = |id: gpui::SharedString, number: String, name: String| {
            div()
                .id(id)
                .flex()
                .items_center()
                .gap(px(8.0))
                .px(px(6.0))
                .py(px(4.0))
                .rounded(px(4.0))
                .text_size(px(12.0))
                .text_color(theme::text())
                .cursor_pointer()
                .hover(|d| d.bg(theme::hover_soft()))
                .child(
                    div()
                        .flex_none()
                        .text_color(theme::text_faint())
                        .child(number),
                )
                .child(div().flex_1().min_w_0().truncate().child(name))
        };
        let root = self.stores.projects.read(cx).path_of(&self.project_id);
        let mut list = div()
            .id("foreman-pick")
            .flex()
            .flex_col()
            .gap(px(2.0))
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
                let id = task.id.clone();
                let after = (!task.depends_on.is_empty()).then(|| {
                    div()
                        .text_size(px(11.0))
                        .text_color(theme::text_muted())
                        .child(t_args(
                            "office.foreman.after",
                            &[("ids", &task.depends_on.join(", "))],
                        ))
                });
                let criteria = (!task.acceptance.is_empty()).then(|| {
                    div()
                        .text_size(px(11.0))
                        .text_color(theme::text_faint())
                        .child(task.acceptance.clone())
                });
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .px(px(8.0))
                    .py(px(6.0))
                    .rounded(px(4.0))
                    .border_1()
                    .border_color(theme::border_subtle())
                    .text_size(px(12.0))
                    .text_color(theme::text())
                    .child(
                        div()
                            .flex()
                            .items_start()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .flex_none()
                                    .text_color(theme::text_faint())
                                    .child(task.id.clone()),
                            )
                            .child(div().flex_1().min_w_0().child(task.title.clone()))
                            .child(div().flex_none().text_color(theme::text_faint()).child(t(
                                if task.write {
                                    "office.foreman.write"
                                } else {
                                    "office.foreman.read"
                                },
                            )))
                            .child(
                                div()
                                    .id(gpui::SharedString::from(format!("foreman-drop-{id}")))
                                    .flex_none()
                                    .px(px(4.0))
                                    .rounded(px(3.0))
                                    .text_color(theme::text_faint())
                                    .cursor_pointer()
                                    .hover(|d| d.bg(theme::hover_soft()))
                                    .on_click(
                                        cx.listener(move |this, _, _, cx| this.drop_draft(&id, cx)),
                                    )
                                    .child("×"),
                            ),
                    )
                    .children(after)
                    .children(criteria)
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
            .flex_wrap()
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
            ))
            .when(self.foreman_seat(cx).is_some(), |d| {
                d.child(button(
                    "foreman-discuss",
                    t("office.foreman.discuss"),
                    ButtonVariant::Secondary,
                    cx.listener(|this, _, _, cx| this.discuss(cx)),
                ))
            });
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

    fn task_panel(&self, id: &str, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(task) = self.tasks.iter().find(|t| t.id == id) else {
            return Self::foreman_shell().into_any_element();
        };
        let label = |key: &str| {
            div()
                .text_size(px(11.0))
                .text_color(theme::text_muted())
                .child(t(key))
        };
        let text = |s: String| {
            div()
                .text_size(px(12.0))
                .text_color(theme::text_secondary())
                .child(s)
        };
        let changes: Vec<gpui::AnyElement> = match &self.task_diff {
            _ if !task.write => vec![text(t("office.review.no_changes")).into_any_element()],
            Some((tid, diff)) if tid == id && diff.trim().is_empty() => {
                vec![text(t("office.review.empty_diff")).into_any_element()]
            }
            Some((tid, diff)) if tid == id => diff
                .lines()
                .take(DIFF_LINES)
                .map(|l| {
                    let color = if l.starts_with("+++") || l.starts_with("---") {
                        theme::text_muted()
                    } else if l.starts_with('+') {
                        theme::success()
                    } else if l.starts_with('-') {
                        theme::error()
                    } else if l.starts_with("@@") {
                        theme::text_faint()
                    } else {
                        theme::text_secondary()
                    };
                    div()
                        .text_size(px(11.0))
                        .text_color(color)
                        .child(if l.is_empty() {
                            " ".to_string()
                        } else {
                            l.to_string()
                        })
                        .into_any_element()
                })
                .collect(),
            _ => vec![text(t("office.foreman.loading")).into_any_element()],
        };
        let list = div()
            .id("task-review")
            .flex()
            .flex_col()
            .gap(px(6.0))
            .p(px(10.0))
            .overflow_y_scroll()
            .child(label("office.review.summary"))
            .child(text(if task.summary.is_empty() {
                "-".to_string()
            } else {
                task.summary.clone()
            }))
            .children((!task.acceptance.is_empty()).then(|| {
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(label("office.review.criteria"))
                    .child(text(task.acceptance.clone()))
            }))
            .child(label("office.review.changes"))
            .children(changes);
        let task_id = task.id.clone();
        Self::foreman_shell()
            .child(
                div()
                    .text_size(px(14.0))
                    .text_color(theme::text())
                    .child(format!("{}  {}", task.id, task.title)),
            )
            .child(self.foreman_bar.overlay(list))
            .child(label("office.review.feedback_hint"))
            .child(
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
            .child(
                div()
                    .flex()
                    .gap(px(8.0))
                    .child(button(
                        "task-accept",
                        t("office.review.accept"),
                        ButtonVariant::Classic,
                        cx.listener(move |this, _, _, cx| this.accept_task(&task_id, cx)),
                    ))
                    .child(button(
                        "task-close",
                        t("common.close"),
                        ButtonVariant::Secondary,
                        cx.listener(|this, _, _, cx| {
                            this.foreman = None;
                            cx.notify();
                        }),
                    )),
            )
            .into_any_element()
    }

    fn skills_body(&self, cx: &Context<Self>) -> gpui::AnyElement {
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
            .flex()
            .flex_col()
            .gap(px(8.0))
            .px(px(14.0))
            .pb(px(10.0))
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

    fn toggle_section(&mut self, i: usize, cx: &mut Context<Self>) {
        self.open[i] = !self.open[i];
        let to = if self.open[i] { 1.0 } else { 0.0 };
        self.section_fx[i].retarget(to, Instant::now());
        cx.notify();
    }

    fn section_height(&self, i: usize, cx: &Context<Self>) -> f32 {
        match i {
            BOARD => {
                let shown = self
                    .tasks
                    .iter()
                    .filter(|t| t.status != TaskStatus::Draft)
                    .count()
                    .min(BOARD_SHOWN);
                shown.max(1) as f32 * (ROW_H + 2.0) + 8.0
            }
            DONE => self.prs.len() as f32 * (ROW_H + 2.0) + 8.0,
            _ => {
                let n = self.stores.skills.read(cx).snapshot.installed.len();
                44.0 + n.div_ceil(3) as f32 * 32.0
            }
        }
    }

    fn rail(&self, seats: &[Seat], now: Instant, cx: &mut Context<Self>) -> gpui::AnyElement {
        let open_tasks = self
            .tasks
            .iter()
            .filter(|t| t.status == TaskStatus::Open)
            .count();
        let total = self
            .tasks
            .iter()
            .filter(|t| t.status != TaskStatus::Draft)
            .count();
        let titles = [
            t_args(
                "office.board",
                &[
                    ("open", &open_tasks.to_string()),
                    ("total", &total.to_string()),
                ],
            ),
            t_args("office.done", &[("count", &self.prs.len().to_string())]),
            t("office.skills"),
        ];
        let mut list = div()
            .id("office-rail")
            .flex()
            .flex_col()
            .overflow_y_scroll();
        for (i, title) in titles.into_iter().enumerate() {
            let k = self.section_fx[i].value(now);
            let chevron = if self.open[i] { "▾" } else { "▸" };
            list = list.child(
                div()
                    .id(gpui::SharedString::from(format!("rail-head-{i}")))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .px(px(14.0))
                    .h(px(32.0))
                    .border_t_1()
                    .border_color(theme::border_subtle())
                    .text_size(px(11.0))
                    .text_color(theme::text_muted())
                    .cursor_pointer()
                    .hover(|d| d.bg(theme::hover_soft()))
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_section(i, cx)))
                    .child(div().flex_none().w(px(10.0)).child(chevron))
                    .child(div().flex_1().min_w_0().truncate().child(title)),
            );
            if k <= 0.001 {
                continue;
            }
            let body = match i {
                BOARD if total == 0 => div()
                    .px(px(14.0))
                    .pb(px(8.0))
                    .text_size(px(12.0))
                    .text_color(theme::text_faint())
                    .child(t("office.board_empty"))
                    .into_any_element(),
                BOARD => self.board_body(seats, now, cx),
                DONE => self.done_body(now),
                _ => self.skills_body(cx),
            };
            let mut wrap = div().flex_none().overflow_hidden().child(body);
            if k < 0.999 {
                wrap = wrap.max_h(px(self.section_height(i, cx) * k));
            }
            list = list.child(wrap);
        }
        div()
            .flex()
            .flex_col()
            .size_full()
            .border_l_1()
            .border_color(theme::border_subtle())
            .child(self.rail_bar.overlay(list))
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

    fn desk_pos(&self, key: &str, now: Instant) -> (f32, f32) {
        self.glides
            .get(key)
            .map_or((0.0, 0.0), |(x, y)| (x.value(now), y.value(now)))
    }

    fn sync_fx(
        &mut self,
        seats: &[Seat],
        cols: usize,
        panel_open: bool,
        now: Instant,
        cx: &Context<Self>,
    ) -> bool {
        let keys: Vec<&str> = std::iter::once(FOREMAN)
            .chain(seats.iter().map(|s| s.id.as_str()))
            .collect();
        for (slot, key) in keys.iter().enumerate() {
            let tx = (slot % cols) as f32 * CELL_W;
            let ty = (slot / cols) as f32 * CELL_H;
            match self.glides.get_mut(*key) {
                Some((x, y)) => {
                    x.retarget(tx, now);
                    y.retarget(ty, now);
                }
                None => {
                    let drop = if *key == FOREMAN { 0.0 } else { ENTER_DROP };
                    self.glides.insert(
                        key.to_string(),
                        (Tween::at(tx, now), Tween::between(ty + drop, ty, now)),
                    );
                }
            }
            let selected = if *key == FOREMAN {
                !matches!(self.foreman, None | Some(ForemanPanel::Task(_)))
            } else {
                self.selected.as_deref() == Some(*key)
            };
            let to = if selected { 1.0 } else { 0.0 };
            self.sel_fx
                .entry(key.to_string())
                .or_insert_with(|| Tween::at(0.0, now))
                .retarget(to, now);
        }
        let usage = self.stores.usage.read(cx);
        for seat in seats {
            let pct = usage
                .sessions
                .get(&seat.id)
                .map_or(0.0, |u| u.context_percent());
            self.ctx_fx
                .entry(seat.id.clone())
                .or_insert_with(|| Tween::at(pct, now))
                .retarget(pct, now);
        }
        self.glides.retain(|k, _| keys.contains(&k.as_str()));
        self.sel_fx.retain(|k, _| keys.contains(&k.as_str()));
        self.ctx_fx.retain(|k, _| keys.contains(&k.as_str()));

        self.panel_fx
            .retarget(if panel_open { 1.0 } else { 0.0 }, now);

        let seen = if self.rows_primed {
            now
        } else {
            now.checked_sub(DUR * 2).unwrap_or(now)
        };
        self.rows_primed = true;
        let mut live: Vec<(String, String)> = self
            .tasks
            .iter()
            .filter(|t| t.status != TaskStatus::Draft)
            .map(|t| (format!("task-{}", t.id), format!("{:?}", t.status)))
            .collect();
        live.extend(
            self.prs
                .iter()
                .map(|p| (format!("pr-{}", p.number), String::new())),
        );
        self.rows.retain(|k, _| live.iter().any(|(l, _)| l == k));
        for (key, marker) in live {
            match self.rows.get_mut(&key) {
                Some(row) if row.0 == marker => {}
                Some(row) => *row = (marker, now),
                None => {
                    self.rows.insert(key, (marker, seen));
                }
            }
        }

        let tween_running = self
            .glides
            .values()
            .any(|(x, y)| x.active(now) || y.active(now))
            || self.sel_fx.values().any(|t| t.active(now))
            || self.ctx_fx.values().any(|t| t.active(now))
            || self.panel_fx.active(now)
            || self.section_fx.iter().any(|t| t.active(now));
        tween_running
            || self
                .rows
                .values()
                .any(|(_, since)| now.saturating_duration_since(*since) < DUR)
    }

    fn scene(
        &self,
        seats: &[Seat],
        cols: usize,
        now: Instant,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let rows = (seats.len() + 1).div_ceil(cols).max(1);
        let (width, height) = (cols as f32 * CELL_W, rows as f32 * CELL_H);
        let frame = self.frame;
        let fx = |key: &str| self.sel_fx.get(key).map_or(0.0, |t| t.value(now));

        let fstatus = self.foreman_status(cx);
        let busy = fstatus.is_some();
        let drafts = self.drafts().count();
        let badge = if busy && !self.proposed() {
            t("office.foreman.drafting")
        } else if drafts > 0 {
            t_args(
                "office.foreman.badge_review",
                &[("count", &drafts.to_string())],
            )
        } else {
            t("office.status.idle")
        };
        let (fx_, fy) = self.desk_pos(FOREMAN, now);
        let mut desks = vec![Placed {
            x: fx_,
            y: fy,
            slot: 0,
            view: WorkerView {
                action: Action::of(fstatus.unwrap_or(SessionStatus::Idle), None),
                context_pct: 0.0,
            },
        }];
        let mut cells = vec![
            div()
                .id("foreman-desk")
                .absolute()
                .left(px(fx_))
                .top(px(fy))
                .w(px(CELL_W))
                .h(px(CELL_H))
                .border_1()
                .border_color(fade(theme::accent(), fx(FOREMAN)))
                .cursor_pointer()
                .on_click(cx.listener(|this, _, _, cx| this.click_desk(cx)))
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
                                .self_start()
                                .px(px(6.0))
                                .rounded(px(4.0))
                                .border_1()
                                .border_color(theme::accent())
                                .text_size(px(11.0))
                                .text_color(theme::accent())
                                .child(t("office.foreman.title")),
                        )
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(theme::text_muted())
                                .truncate()
                                .child(badge),
                        ),
                ),
        ];

        for (i, seat) in seats.iter().enumerate() {
            let id = seat.id.clone();
            let drop_id = seat.id.clone();
            let tool = self
                .tool_of(seat)
                .unwrap_or_else(|| Self::status_label(seat.status));
            let (x, y) = self.desk_pos(&seat.id, now);
            let pct = self.ctx_fx.get(&seat.id).map_or(0.0, |t| t.value(now));
            desks.push(Placed {
                x,
                y,
                slot: i,
                view: WorkerView {
                    action: Action::of(seat.status, self.tool_of(seat).as_deref()),
                    context_pct: pct,
                },
            });
            cells.push(
                div()
                    .id(gpui::SharedString::from(format!("worker-{}", seat.id)))
                    .absolute()
                    .left(px(x))
                    .top(px(y))
                    .w(px(CELL_W))
                    .h(px(CELL_H))
                    .cursor_pointer()
                    .border_1()
                    .border_color(fade(theme::accent(), fx(&seat.id)))
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
            .w(px(width))
            .h(px(height))
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, _| {
                        office_art::paint_scene(
                            window,
                            bounds.origin,
                            width,
                            height,
                            &desks,
                            frame,
                        );
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
            .gap(px(8.0))
            .p(px(10.0))
            .overflow_y_scroll()
            .when(lines.is_empty(), |d| {
                d.child(
                    div()
                        .text_size(px(12.0))
                        .text_color(theme::text_faint())
                        .child(t("office.activity_empty")),
                )
            })
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
                            .flex_1()
                            .min_w_0()
                            .text_size(px(14.0))
                            .text_color(theme::text())
                            .truncate()
                            .child(seat.name.clone()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(11.5))
                            .text_color(theme::text_muted())
                            .child(Self::status_label(seat.status)),
                    ),
            )
            .when(seat.status == SessionStatus::NeedsInput, |d| {
                d.child(
                    div()
                        .px(px(10.0))
                        .py(px(8.0))
                        .rounded(px(4.0))
                        .border_1()
                        .border_color(theme::warning())
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = Instant::now();
        let seats = self.seats(cx);
        let selected = self
            .selected
            .as_ref()
            .and_then(|id| seats.iter().find(|s| &s.id == id));
        let panel = match (self.foreman.clone(), selected) {
            (Some(ForemanPanel::Pick), _) => Some(self.foreman_pick(cx)),
            (Some(ForemanPanel::Planner), _) => {
                self.foreman_seat(cx).map(|seat| self.panel(&seat, cx))
            }
            (Some(ForemanPanel::Review), _) => Some(self.foreman_review(cx)),
            (Some(ForemanPanel::Task(id)), _) => Some(self.task_panel(&id, cx)),
            (None, Some(s)) => Some(self.panel(s, cx)),
            (None, None) => None,
        };

        let measured = self.scene_w.get();
        let width = if measured > 0.0 {
            measured
        } else {
            f32::from(window.viewport_size().width) - PANEL_W - 240.0
        };
        let cols = cols_for(width);
        if self.sync_fx(&seats, cols, panel.is_some(), now, cx) {
            window.request_animation_frame();
        }
        let scene = self.scene(&seats, cols, now, cx);

        let measure = {
            let cell = self.scene_w.clone();
            canvas(
                move |bounds, window, _| {
                    let w = f32::from(bounds.size.width);
                    let old = cell.replace(w);
                    if cols_for(w) != cols_for(old) {
                        window.request_animation_frame();
                    }
                },
                |_, _, _, _| (),
            )
            .absolute()
            .size_full()
        };
        let body = div()
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .child(measure)
            .child(
                self.scrollbar.overlay(
                    div().id("office-scroll").overflow_y_scroll().child(
                        div()
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap(px(16.0))
                            .p(px(SCENE_PAD))
                            .child(scene)
                            .when(seats.is_empty(), |d| {
                                d.child(
                                    div()
                                        .text_size(px(13.0))
                                        .text_color(theme::text_muted())
                                        .child(t("office.empty")),
                                )
                            }),
                    ),
                ),
            );

        let slide = self.panel_fx.value(now);
        let rail = div()
            .absolute()
            .top_0()
            .bottom_0()
            .left(px(-slide * 40.0))
            .w(px(PANEL_W))
            .opacity(1.0 - slide)
            .child(self.rail(&seats, now, cx));
        let column = div()
            .relative()
            .flex_none()
            .w(px(PANEL_W))
            .h_full()
            .overflow_hidden()
            .child(rail)
            .children(panel.map(|panel| {
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(px((1.0 - slide) * PANEL_W))
                    .w(px(PANEL_W))
                    .bg(theme::bg_app())
                    .child(panel)
            }));

        div()
            .flex()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .child(body)
            .child(column)
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
