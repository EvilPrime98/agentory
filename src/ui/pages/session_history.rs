//! Session history page: a grid of previous Claude Code sessions read from Claude's own
//! transcripts. Clicking a card resumes that session in a new Agentory session.

use std::collections::HashMap;

use gpui::{
    Context, Entity, EventEmitter, InteractiveElement, IntoElement, ParentElement, Render,
    SharedString, StatefulInteractiveElement, Styled, Subscription, Window, div, prelude::*, px,
};

use crate::backend::transcripts::{self, SessionMeta};
use crate::i18n::{t, t_args, t_n};
use crate::route::{GoBack, Navigate, Route};
use crate::runtime;
use crate::state::Stores;
use crate::state::sessions;
use crate::ui::components::button::small_button;
use crate::ui::components::text_input::{TextInput, TextInputEvent};
use crate::ui::icons::{Icon, icon};
use crate::ui::theme;

/// Transcripts read per step, so the grid fills progressively.
const CHUNK: usize = 24;
/// Rows per page.
const PAGE_SIZE: usize = 15;
const AGE_W: f32 = 130.0;
const COUNT_W: f32 = 100.0;

pub struct SessionHistoryPage {
    stores: Stores,
    project_id: String,
    sessions: Vec<SessionMeta>,
    /// Titles given in Agentory (session id → title).
    titles: HashMap<String, String>,
    loading: bool,
    /// Zero-based page of the grid.
    page: usize,
    /// Bumped on every reload so a superseded scan stops adding cards.
    generation: u64,
    search: Entity<TextInput>,
    /// The card being renamed, with its input.
    rename: Option<(String, Entity<TextInput>)>,
    _subs: Vec<Subscription>,
    _rename_sub: Option<Subscription>,
}

impl EventEmitter<Navigate> for SessionHistoryPage {}
impl EventEmitter<GoBack> for SessionHistoryPage {}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// A fixed-width table cell of muted text.
fn cell(text: String, width: f32) -> impl IntoElement {
    div()
        .flex_none()
        .w(px(width))
        .truncate()
        .text_size(px(12.0))
        .text_color(theme::text_muted())
        .child(text)
}

fn age_label(modified_ms: i64) -> String {
    match transcripts::age(modified_ms, now_ms()) {
        ("now", _) => t("history.age.now"),
        (unit, n) => t_n(&format!("history.age.{unit}"), n as usize),
    }
}

impl SessionHistoryPage {
    pub fn new(stores: Stores, project_id: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| TextInput::new(t("history.search_placeholder"), cx));
        let subs = vec![cx.subscribe_in(
            &search,
            window,
            |this, _, ev: &TextInputEvent, _, cx| {
                if matches!(ev, TextInputEvent::Changed) {
                    this.page = 0;
                    cx.notify();
                }
            },
        )];
        let mut this = Self {
            stores,
            project_id,
            sessions: Vec::new(),
            titles: HashMap::new(),
            loading: false,
            page: 0,
            generation: 0,
            search,
            rename: None,
            _subs: subs,
            _rename_sub: None,
        };
        this.reload(cx);
        this
    }

    /// Scan the transcripts on the Tokio runtime, adding cards as each chunk is read.
    fn reload(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        self.sessions.clear();
        self.page = 0;
        self.titles = transcripts::read_titles();
        self.loading = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let entries = runtime::run(async { transcripts::list() }).await;
            for chunk in entries.chunks(CHUNK) {
                let chunk = chunk.to_vec();
                let metas = runtime::run(async move { transcripts::load(&chunk) }).await;
                let current = this
                    .update(cx, |this, cx| {
                        if this.generation != generation {
                            return false;
                        }
                        this.sessions.extend(metas);
                        cx.notify();
                        true
                    })
                    .unwrap_or(false);
                if !current {
                    return;
                }
            }
            this.update(cx, |this, cx| {
                if this.generation == generation {
                    this.loading = false;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn resume(&mut self, meta: &SessionMeta, cx: &mut Context<Self>) {
        let Some(cwd) = meta.cwd.clone().filter(|c| c.is_dir()) else {
            return;
        };
        if sessions::resume(&meta.id, cwd, &self.project_id, cx) {
            cx.emit(Navigate(Route::Sessions));
        }
    }

    fn begin_rename(&mut self, id: String, current: String, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| TextInput::new(t("history.rename_placeholder"), cx).with_value(current));
        let rename_id = id.clone();
        self._rename_sub = Some(cx.subscribe_in(
            &input,
            window,
            move |this, input, ev: &TextInputEvent, _, cx| match ev {
                TextInputEvent::Submit => {
                    let title = input.read(cx).value().to_string();
                    if let Err(e) = transcripts::write_title(&rename_id, &title) {
                        tracing::warn!("could not save the session title: {e}");
                    }
                    this.titles = transcripts::read_titles();
                    this.rename = None;
                    this._rename_sub = None;
                    cx.notify();
                }
                TextInputEvent::Cancel => {
                    this.rename = None;
                    this._rename_sub = None;
                    cx.notify();
                }
                TextInputEvent::Changed => {}
            },
        ));
        self.rename = Some((id, input));
        cx.notify();
    }

    /// Sessions of this project matching the search, newest first.
    fn visible(&self, cx: &Context<Self>) -> Vec<&SessionMeta> {
        let query = self.search.read(cx).value().trim().to_lowercase();
        let path = self.stores.projects.read(cx).path_of(&self.project_id);
        self.sessions
            .iter()
            .filter(|m| {
                path.as_deref()
                    .is_some_and(|p| m.cwd.as_deref().is_some_and(|c| transcripts::same_dir(c, p)))
            })
            .filter(|m| {
                query.is_empty()
                    || self
                        .titles
                        .get(&m.id)
                        .unwrap_or(&m.title)
                        .to_lowercase()
                        .contains(&query)
                    || m.title.to_lowercase().contains(&query)
                    || m.first_prompt.to_lowercase().contains(&query)
            })
            .collect()
    }
}

impl Render for SessionHistoryPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.search
            .update(cx, |s, _| s.set_placeholder(t("history.search_placeholder")));
        let all: Vec<SessionMeta> = self.visible(cx).into_iter().cloned().collect();
        let empty = all.is_empty();
        let pages = all.len().div_ceil(PAGE_SIZE).max(1);
        self.page = self.page.min(pages - 1);
        let page = self.page;
        let metas: Vec<SessionMeta> = all.into_iter().skip(page * PAGE_SIZE).take(PAGE_SIZE).collect();

        let rows = metas.into_iter().enumerate().map(|(index, meta)| {
            let title = self
                .titles
                .get(&meta.id)
                .cloned()
                .filter(|t| !t.is_empty())
                .or_else(|| (!meta.title.is_empty()).then(|| meta.title.clone()))
                .unwrap_or_else(|| t("history.untitled"));
            let resumable = meta.cwd.as_deref().is_some_and(|c| c.is_dir());
            let renaming = self
                .rename
                .as_ref()
                .filter(|(id, _)| *id == meta.id)
                .map(|(_, input)| input.clone());
            let (rename_id, rename_title) = (meta.id.clone(), title.clone());
            let age = age_label(meta.modified_ms);
            let count = t_n("history.messages", meta.messages);
            let click_meta = meta.clone();

            div()
                .id(SharedString::from(format!("history-{}", meta.id)))
                .flex()
                .items_center()
                .gap(px(12.0))
                .px(px(12.0))
                .py(px(8.0))
                .when(index > 0, |s| {
                    s.border_t_1().border_color(theme::border_subtle())
                })
                .when(resumable, |s| {
                    s.cursor_pointer().hover(|s| s.bg(theme::hover()))
                })
                .when(!resumable, |s| s.opacity(0.5))
                .on_click(cx.listener(move |this, _, _, cx| this.resume(&click_meta, cx)))
                .child(match renaming {
                    Some(input) => div()
                        .id("history-rename-field")
                        .flex_1()
                        .min_w_0()
                        .h(px(24.0))
                        .px(px(6.0))
                        .rounded(px(4.0))
                        .border_1()
                        .border_color(theme::border_subtle())
                        // Typing in the field must not resume the session.
                        .on_click(|_, _, cx| cx.stop_propagation())
                        .child(input)
                        .into_any_element(),
                    None => div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .truncate()
                                .text_size(px(13.0))
                                .text_color(theme::text())
                                .child(title),
                        )
                        .when(!resumable, |s| {
                            s.child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(theme::text_muted())
                                    .child(t("history.cwd_missing")),
                            )
                        })
                        .into_any_element(),
                })
                .child(cell(age, AGE_W))
                .child(cell(count, COUNT_W))
                .child(
                    div()
                        .id(SharedString::from(format!("rename-{}", meta.id)))
                        .flex()
                        .flex_none()
                        .items_center()
                        .justify_center()
                        .size(px(22.0))
                        .rounded(px(4.0))
                        .cursor_pointer()
                        .hover(|s| s.bg(theme::hover()))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.begin_rename(rename_id.clone(), rename_title.clone(), window, cx);
                        }))
                        .child(icon(Icon::Pencil, px(13.0)).text_color(theme::text_muted())),
                )
        });
        let table = div()
            .flex()
            .flex_col()
            .rounded(px(6.0))
            .border_1()
            .border_color(theme::border_subtle())
            .bg(theme::bg_app())
            .overflow_hidden()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .px(px(12.0))
                    .py(px(6.0))
                    .text_size(px(11.0))
                    .text_color(theme::text_muted())
                    .child(div().flex_1().min_w_0().child(t("history.col.title")))
                    .child(cell(t("history.col.updated"), AGE_W))
                    .child(cell(t("history.col.messages"), COUNT_W))
                    // Room for the rename button below.
                    .child(div().flex_none().w(px(22.0))),
            )
            .children(rows);

        div()
            .id("history-scroll")
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .overflow_y_scroll()
            .items_center()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .w_full()
                    .max_w(px(1100.0))
                    .p(px(24.0))
                    .gap(px(12.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_size(px(20.0))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child(t("history.title")),
                            )
                            .child(
                                div()
                                    .id("history-refresh")
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .size(px(26.0))
                                    .rounded(px(6.0))
                                    .cursor_pointer()
                                    .hover(|s| s.bg(theme::hover()))
                                    .on_click(cx.listener(|this, _, _, cx| this.reload(cx)))
                                    .child(icon(Icon::Refresh, px(15.0)).text_color(theme::text())),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .flex()
                                    .flex_1()
                                    .min_w_0()
                                    .h(px(26.0))
                                    .px(px(8.0))
                                    .rounded(px(4.0))
                                    .border_1()
                                    .border_color(theme::border_subtle())
                                    .bg(theme::bg_app())
                                    .child(self.search.clone()),
                            )
                    )
                    .child(table)
                    .when(pages > 1, |s| {
                        s.child(
                            div()
                                .flex()
                                .items_center()
                                .justify_center()
                                .gap(px(10.0))
                                .child(small_button(
                                    "history-prev",
                                    t("history.prev"),
                                    cx.listener(|this, _, _, cx| {
                                        this.page = this.page.saturating_sub(1);
                                        cx.notify();
                                    }),
                                ))
                                .child(
                                    div()
                                        .text_size(px(12.0))
                                        .text_color(theme::text_muted())
                                        .child(t_args(
                                            "history.page",
                                            &[
                                                ("page", &(page + 1).to_string()),
                                                ("total", &pages.to_string()),
                                            ],
                                        )),
                                )
                                .child(small_button(
                                    "history-next",
                                    t("history.next"),
                                    cx.listener(move |this, _, _, cx| {
                                        this.page = (this.page + 1).min(pages - 1);
                                        cx.notify();
                                    }),
                                )),
                        )
                    })
                    .when(empty, |s| {
                        s.child(
                            div()
                                .text_size(px(12.0))
                                .text_color(theme::text_muted())
                                .child(if self.loading {
                                    t("history.loading")
                                } else {
                                    t("history.empty")
                                }),
                        )
                    }),
            )
    }
}
