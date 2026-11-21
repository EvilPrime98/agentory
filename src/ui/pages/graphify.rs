use gpui::{
    AnyElement, Context, EventEmitter, InteractiveElement, IntoElement, ParentElement, Render,
    StatefulInteractiveElement, Styled, Window, div, px,
};

use super::skills::{card, icon_button, muted_note, row_base};
use crate::backend::graphify::{self, Status};
use crate::backend::transcripts;
use crate::i18n::{t, t_args, t_n};
use crate::route::{Navigate, Route};
use crate::runtime;
use crate::state::{Stores, sessions};
use crate::ui::components::button::{ButtonVariant, button};
use crate::ui::components::scrollbar::Scrollbar;
use crate::ui::icons::Icon;
use crate::ui::{theme, toast};

pub struct GraphifyPage {
    scrollbar: Scrollbar,
    stores: Stores,
    project_id: String,
    status: Option<Status>,
    updating: bool,
}

impl EventEmitter<Navigate> for GraphifyPage {}

fn fact(label: String, value: String) -> AnyElement {
    row_base(1)
        .justify_between()
        .child(
            div()
                .text_size(px(12.5))
                .text_color(theme::text_muted())
                .child(label),
        )
        .child(div().text_size(px(12.5)).child(value))
        .into_any_element()
}

fn built_text(secs: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64);
    match transcripts::age(secs as i64 * 1000, now) {
        ("now", _) => t("history.age.now"),
        (unit, n) => t_n(&format!("history.age.{unit}"), n as usize),
    }
}

impl GraphifyPage {
    pub fn new(stores: Stores, project_id: String, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            scrollbar: Scrollbar::new(),
            stores,
            project_id,
            status: None,
            updating: false,
        };
        this.refresh(cx);
        this
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(dir) = self.stores.projects.read(cx).path_of(&self.project_id) else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let status = runtime::run(async move { graphify::status(&dir) }).await;
            this.update(cx, |this, cx| {
                this.status = Some(status);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn update_graph(&mut self, cx: &mut Context<Self>) {
        let Some(dir) = self.stores.projects.read(cx).path_of(&self.project_id) else {
            return;
        };
        if self.updating {
            return;
        }
        self.updating = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = runtime::run(async move { graphify::update(&dir) }).await;
            this.update(cx, |this, cx| {
                this.updating = false;
                match result {
                    Ok(_) => toast::success(cx, t("graphify.updated")),
                    Err(e) => toast::error(cx, t_args("graphify.update_failed", &[("error", &e)])),
                }
                this.refresh(cx);
            })
            .ok();
        })
        .detach();
    }

    fn full_build(&self, cx: &mut Context<Self>) {
        sessions::begin_new_input(&self.project_id, Some("/graphify".to_string()), cx);
        cx.emit(Navigate(Route::Sessions));
    }

    fn open_report(&self, cx: &mut Context<Self>) {
        let rel = "../graphify-out/GRAPH_REPORT.md".to_string();
        cx.emit(Navigate(Route::DocPreview(self.project_id.clone(), rel)));
    }

    fn open_graph_html(&self, path: &std::path::Path, cx: &mut Context<Self>) {
        if let Err(e) = open::that(path) {
            toast::error(
                cx,
                t_args("graphify.open_html_failed", &[("error", &e.to_string())]),
            );
        }
    }

    fn status_card(&self, cx: &mut Context<Self>) -> AnyElement {
        let this = cx.entity();
        let refresh = icon_button("graphify-refresh", Icon::Refresh, move |_, _, cx| {
            this.update(cx, |this, cx| this.refresh(cx))
        })
        .into_any_element();
        let body = match &self.status {
            None => Vec::new(),
            Some(s) if !s.installed => vec![
                fact(t("graphify.installed"), t("graphify.not_installed")),
                muted_note(t("graphify.install_hint").into()).into_any_element(),
            ],
            Some(s) => {
                let mut rows = vec![fact(
                    t("graphify.version"),
                    s.version.clone().unwrap_or_default(),
                )];
                if s.graph {
                    rows.push(fact(t("graphify.graph"), t("graphify.present")));
                    if let Some(n) = s.nodes {
                        rows.push(fact(t("graphify.nodes"), n.to_string()));
                    }
                    if let Some(b) = s.built_at {
                        rows.push(fact(t("graphify.built"), built_text(b)));
                    }
                    let key = if s.stale {
                        "graphify.stale"
                    } else {
                        "graphify.fresh"
                    };
                    rows.push(muted_note(t(key).into()).into_any_element());
                } else {
                    rows.push(muted_note(t("graphify.no_graph").into()).into_any_element());
                }
                rows
            }
        };
        card(&t("graphify.title"), None, Some(refresh), body).into_any_element()
    }

    fn actions_card(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let s = self.status.as_ref().filter(|s| s.installed)?;
        let mut body: Vec<AnyElement> = Vec::new();
        if s.graph {
            let label = t(if self.updating {
                "graphify.updating"
            } else {
                "graphify.update"
            });
            body.push(
                div()
                    .flex()
                    .flex_col()
                    .items_start()
                    .gap(px(4.0))
                    .child(button(
                        "graphify-update",
                        label,
                        ButtonVariant::Classic,
                        cx.listener(|this, _, _, cx| this.update_graph(cx)),
                    ))
                    .child(muted_note(t("graphify.update_hint").into()))
                    .into_any_element(),
            );
        }
        body.push(
            div()
                .flex()
                .flex_col()
                .items_start()
                .gap(px(4.0))
                .child(button(
                    "graphify-build",
                    t("graphify.full_build"),
                    ButtonVariant::Secondary,
                    cx.listener(|this, _, _, cx| this.full_build(cx)),
                ))
                .child(muted_note(t("graphify.full_build_hint").into()))
                .into_any_element(),
        );
        if s.report.is_some() || s.graph_html.is_some() {
            let mut row = div().flex().items_center().gap(px(8.0));
            if s.report.is_some() {
                row = row.child(button(
                    "graphify-report",
                    t("graphify.report"),
                    ButtonVariant::Secondary,
                    cx.listener(|this, _, _, cx| this.open_report(cx)),
                ));
            }
            if let Some(path) = s.graph_html.clone() {
                row = row.child(icon_button(
                    "graphify-open-html",
                    Icon::Globe,
                    cx.listener(move |this, _, _, cx| this.open_graph_html(&path, cx)),
                ));
            }
            body.push(row.into_any_element());
        }
        Some(card("", None, None, body).into_any_element())
    }
}

impl Render for GraphifyPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let status = self.status_card(cx);
        let actions = self.actions_card(cx);
        self.scrollbar.overlay(
            div()
                .id("graphify-scroll")
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
                        .max_w(px(720.0))
                        .p(px(24.0))
                        .gap(px(12.0))
                        .child(
                            div()
                                .text_size(px(20.0))
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .child(t("graphify.title")),
                        )
                        .child(status)
                        .children(actions),
                ),
        )
    }
}
