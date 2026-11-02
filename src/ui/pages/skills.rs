//! Skills page: a project's skills, managed by the built-in `spm`.

use std::collections::BTreeSet;

use gpui::{
    AnyElement, Context, Entity, EventEmitter, InteractiveElement, IntoElement, ParentElement,
    Render, StatefulInteractiveElement, Styled, Subscription, Window, div, prelude::*, px,
};

use crate::i18n::{t, t_args};
use crate::route::{GoBack, Navigate, Route};
use crate::state::Stores;
use crate::ui::components::button::small_button;
use crate::ui::components::checkbox::checkbox;
use crate::ui::components::smooth_scroll::SmoothScroll;
use crate::ui::components::text_input::{TextInput, TextInputEvent};
use crate::ui::icons::{Icon, icon};
use crate::ui::theme;

#[derive(Clone, Copy, PartialEq, Eq)]
enum SkillsTab {
    Installed,
    Discover,
    Sources,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DiscoverTab {
    Repos,
    Market,
}

pub struct SkillsPage {
    stores: Stores,
    tab: SkillsTab,
    discover: DiscoverTab,
    repo_input: Entity<TextInput>,
    init_repo_input: Entity<TextInput>,
    search_input: Entity<TextInput>,
    install_global: bool,
    confirm_remove: Option<String>,
    installed_scroll: SmoothScroll,
    market_scroll: SmoothScroll,
    pool_scroll: SmoothScroll,
    _subs: Vec<Subscription>,
}

/// Narrowest a card gets before the row wraps and the cards stack.
const CARD_MIN_W: f32 = 340.0;

const LIST_MAX_H: f32 = 560.0;

const INSTALLED_MAX_H: f32 = 560.0;

fn field(input: &Entity<TextInput>) -> impl IntoElement {
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
        .child(input.clone())
}

/// Bordered section: header (title, optional count badge, optional right-side action) then body.
pub(super) fn card(
    title: &str,
    count: Option<usize>,
    action: Option<AnyElement>,
    body: Vec<AnyElement>,
) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .flex_1()
        .min_w(px(CARD_MIN_W))
        .p(px(10.0))
        .gap(px(6.0))
        .rounded(px(6.0))
        .border_1()
        .border_color(theme::border_subtle())
        .bg(theme::bg_app())
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(8.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .child(
                            div()
                                .text_size(px(12.0))
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .child(title.to_string()),
                        )
                        .when_some(count, |s, n| {
                            s.child(
                                div()
                                    .px(px(6.0))
                                    .rounded(px(8.0))
                                    .bg(theme::hover())
                                    .text_size(px(11.0))
                                    .text_color(theme::text_muted())
                                    .child(n.to_string()),
                            )
                        }),
                )
                .when_some(action, |s, a| s.child(a)),
        )
        .children(body)
}

pub(super) fn icon_button(
    id: impl Into<gpui::ElementId>,
    glyph: Icon,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(24.0))
        .rounded(px(4.0))
        .cursor_pointer()
        .hover(|s| s.bg(theme::hover()))
        .on_click(on_click)
        .child(icon(glyph, px(14.0)).text_color(theme::text_muted()))
}

/// Hairline between list rows: every row but the first gets a top border.
pub(super) fn row_base(index: usize) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .gap(px(8.0))
        .py(px(5.0))
        .when(index > 0, |s| {
            s.border_t_1().border_color(theme::border_subtle())
        })
}

pub(super) fn muted_note(text: gpui::SharedString) -> impl IntoElement {
    div()
        .mt(px(6.0))
        .text_size(px(12.0))
        .text_color(theme::text_muted())
        .child(text)
}

fn segment(
    id: impl Into<gpui::ElementId>,
    label: impl Into<gpui::SharedString>,
    active: bool,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .flex()
        .items_center()
        .h(px(26.0))
        .px(px(12.0))
        .rounded(px(4.0))
        .text_size(px(12.5))
        .cursor_pointer()
        .when(active, |s| {
            s.bg(theme::hover())
                .text_color(theme::text())
                .font_weight(gpui::FontWeight::SEMIBOLD)
        })
        .when(!active, |s| {
            s.text_color(theme::text_muted())
                .hover(|s| s.text_color(theme::text()))
        })
        .on_click(on_click)
        .child(label.into())
}

fn group_heading(title: String, count: usize) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap(px(6.0))
        .mt(px(6.0))
        .child(
            div()
                .text_size(px(12.0))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .child(title),
        )
        .child(
            div()
                .px(px(6.0))
                .rounded(px(8.0))
                .bg(theme::hover())
                .text_size(px(11.0))
                .text_color(theme::text_muted())
                .child(count.to_string()),
        )
}

fn result_text(name: String, lines: Vec<String>) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .flex_1()
        .min_w_0()
        .child(
            div()
                .truncate()
                .text_size(px(13.0))
                .text_color(theme::text())
                .child(name),
        )
        .children(lines.into_iter().filter(|l| !l.is_empty()).map(|l| {
            div()
                .truncate()
                .text_size(px(11.0))
                .text_color(theme::text_muted())
                .child(l)
        }))
}

impl EventEmitter<Navigate> for SkillsPage {}
impl EventEmitter<GoBack> for SkillsPage {}

impl SkillsPage {
    pub fn new(stores: Stores, cx: &mut Context<Self>) -> Self {
        let repo_input = cx.new(|cx| TextInput::new("owner/repo", cx));
        let init_repo_input = cx.new(|cx| TextInput::new(t("skills.init_placeholder"), cx));
        let search_input = cx.new(|cx| TextInput::new(t("skills.search_placeholder"), cx));
        let subs = vec![
            cx.observe(&stores.skills, |_, _, cx| cx.notify()),
            cx.observe(&stores.projects, |_, _, cx| cx.notify()),
            cx.subscribe(&search_input, |this, _, ev: &TextInputEvent, cx| match ev {
                TextInputEvent::Changed => cx.notify(),
                TextInputEvent::Submit => this.submit_search(cx),
                _ => {}
            }),
            cx.subscribe(&init_repo_input, |this, _, ev: &TextInputEvent, cx| {
                if matches!(ev, TextInputEvent::Submit) {
                    this.submit_init(cx);
                }
            }),
            cx.subscribe(&repo_input, |this, _, ev: &TextInputEvent, cx| {
                if matches!(ev, TextInputEvent::Submit) {
                    this.submit_repo(cx);
                }
            }),
        ];
        Self {
            stores,
            tab: SkillsTab::Installed,
            discover: DiscoverTab::Repos,
            repo_input,
            init_repo_input,
            search_input,
            install_global: false,
            confirm_remove: None,
            installed_scroll: SmoothScroll::new(),
            market_scroll: SmoothScroll::new(),
            pool_scroll: SmoothScroll::new(),
            _subs: subs,
        }
    }

    fn submit_search(&mut self, cx: &mut Context<Self>) {
        let term = self.search_input.read(cx).value().trim().to_string();
        if term.is_empty() {
            return;
        }
        self.discover = DiscoverTab::Market;
        self.stores
            .skills
            .update(cx, |s, cx| s.search_market(term, cx));
    }

    fn submit_init(&mut self, cx: &mut Context<Self>) {
        let repo = self.init_repo_input.read(cx).value().trim().to_string();
        self.init_repo_input.update(cx, |i, cx| i.set_value("", cx));
        let repo = (!repo.is_empty()).then_some(repo);
        self.stores.skills.update(cx, |s, cx| s.init(repo, cx));
    }

    fn submit_repo(&mut self, cx: &mut Context<Self>) {
        let repo = self.repo_input.read(cx).value().trim().to_string();
        if repo.is_empty() {
            return;
        }
        self.repo_input.update(cx, |i, cx| i.set_value("", cx));
        self.stores.skills.update(cx, |s, cx| s.add_repo(repo, cx));
    }

    fn tab_button(
        &self,
        id: &'static str,
        label: String,
        tab: SkillsTab,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        segment(
            id,
            label,
            self.tab == tab,
            cx.listener(move |this, _, _, cx| {
                this.tab = tab;
                this.confirm_remove = None;
                cx.notify();
            }),
        )
    }

    fn installed_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let skills = self.stores.skills.read(cx);
        let manifest = skills.snapshot.manifest.clone();
        let doctor = skills.doctor.clone();
        let on_disk_set: BTreeSet<String> = skills.snapshot.installed.iter().cloned().collect();
        let latest_set = skills.latest.clone();
        // Declared in spm.json plus anything on disk.
        let mut names = on_disk_set.clone();
        if let Some(m) = &manifest {
            names.extend(m.skills.keys().cloned());
        }
        let project_id = self.stores.projects.read(cx).selected.clone();
        let confirming = self.confirm_remove.clone();

        let rows: Vec<AnyElement> = names
            .into_iter()
            .enumerate()
            .map(|(i, name)| {
                let on_disk = on_disk_set.contains(&name);
                let source = manifest.as_ref().and_then(|m| m.source_of(&name));
                let cost = doctor.as_ref().and_then(|d| {
                    d.skills
                        .iter()
                        .find(|s| s.name == name)
                        .map(|s| s.description_tokens)
                });
                let is_market = manifest
                    .as_ref()
                    .and_then(|m| m.skills.get(&name))
                    .is_some_and(serde_json::Value::is_object);
                let route = project_id
                    .clone()
                    .filter(|_| on_disk)
                    .map(|id| Route::SkillEditor(id, name.clone()));
                let is_latest = latest_set.contains(&name);
                let latest_name = name.clone();
                let is_confirming = confirming.as_deref() == Some(name.as_str());

                let actions: AnyElement = if is_confirming {
                    let remove_name = name.clone();
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(px(6.0))
                        .child(small_button(
                            gpui::SharedString::from(format!("rm-confirm-{name}")),
                            t("common.remove"),
                            cx.listener(move |this, _, _, cx| {
                                this.confirm_remove = None;
                                let n = remove_name.clone();
                                this.stores.skills.update(cx, |s, cx| s.uninstall(n, cx));
                            }),
                        ))
                        .child(small_button(
                            gpui::SharedString::from(format!("rm-cancel-{name}")),
                            t("common.cancel"),
                            cx.listener(|this, _, _, cx| {
                                this.confirm_remove = None;
                                cx.notify();
                            }),
                        ))
                        .into_any_element()
                } else {
                    let ask_name = name.clone();
                    icon_button(
                        gpui::SharedString::from(format!("rm-skill-{name}")),
                        Icon::Trash,
                        cx.listener(move |this, _, _, cx| {
                            this.confirm_remove = Some(ask_name.clone());
                            cx.notify();
                        }),
                    )
                    .into_any_element()
                };

                row_base(i)
                    .when(!on_disk, |s| {
                        s.child(
                            div()
                                .flex_none()
                                .size(px(6.0))
                                .rounded_full()
                                .bg(theme::warning()),
                        )
                    })
                    .child(
                        div()
                            .id(gpui::SharedString::from(format!("open-skill-{name}")))
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .rounded(px(4.0))
                            .when_some(route, |s, route| {
                                s.cursor_pointer().hover(|s| s.bg(theme::hover())).on_click(
                                    cx.listener(move |_, _, _, cx| {
                                        cx.emit(Navigate(route.clone()));
                                    }),
                                )
                            })
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(13.0))
                                    .text_color(if on_disk {
                                        theme::text()
                                    } else {
                                        theme::warning()
                                    })
                                    .child(if on_disk {
                                        name.clone()
                                    } else {
                                        t_args("skills.not_installed", &[("name", &name)])
                                    }),
                            )
                            .when_some(source, |s, src| {
                                s.child(
                                    div()
                                        .truncate()
                                        .text_size(px(11.0))
                                        .text_color(theme::text_muted())
                                        .child(src),
                                )
                            }),
                    )
                    .when_some(cost, |s, tokens| {
                        s.child(
                            div()
                                .flex_none()
                                .px(px(6.0))
                                .rounded(px(4.0))
                                .bg(theme::hover())
                                .text_size(px(11.0))
                                .text_color(theme::text_muted())
                                .child(t_args(
                                    "skills.tokens_short",
                                    &[("n", &tokens.to_string())],
                                )),
                        )
                    })
                    .when(is_market && !is_latest, |s| {
                        s.child(small_button(
                            gpui::SharedString::from(format!("latest-skill-{name}")),
                            t("skills.update_latest"),
                            cx.listener(move |this, _, _, cx| {
                                let n = latest_name.clone();
                                this.stores
                                    .skills
                                    .update(cx, |s, cx| s.update_latest(n, cx));
                            }),
                        ))
                    })
                    .child(actions)
                    .into_any_element()
            })
            .collect();

        let count = rows.len();
        div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(group_heading(t("skills.tab.installed"), count))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .child(small_button(
                                "run-doctor",
                                t("skills.measure"),
                                cx.listener(|this, _, _, cx| {
                                    this.stores.skills.update(cx, |s, cx| s.run_doctor(cx));
                                }),
                            ))
                            .child(small_button(
                                "update-skills",
                                t("skills.update_all"),
                                cx.listener(|this, _, _, cx| {
                                    this.stores.skills.update(cx, |s, cx| s.update_all(cx));
                                }),
                            )),
                    ),
            )
            .when(count == 0, |s| {
                s.child(muted_note(t("skills.none_installed").into()))
            })
            .child(
                self.installed_scroll.framed(
                    div()
                        .id("skills-installed")
                        .flex()
                        .flex_col()
                        .max_h(px(INSTALLED_MAX_H))
                        .children(rows),
                    window,
                ),
            )
            .into_any_element()
    }

    fn discover_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let skills = self.stores.skills.read(cx);
        let mut installed: BTreeSet<String> = skills.snapshot.installed.iter().cloned().collect();
        if let Some(m) = &skills.snapshot.manifest {
            installed.extend(m.skills.keys().cloned());
        }
        let filter = self.search_input.read(cx).value().trim().to_lowercase();
        let pool: Vec<_> = skills
            .available
            .iter()
            .filter(|a| !installed.contains(&a.name))
            .filter(|a| {
                filter.is_empty()
                    || a.name.to_lowercase().contains(&filter)
                    || a.description.to_lowercase().contains(&filter)
            })
            .cloned()
            .collect();
        let available_error = skills.available_error.clone();
        let hits = skills.hits.clone();
        let search_error = skills.search_error.clone();
        let searching = skills.searching;
        let install_global = self.install_global;

        let pool_rows: Vec<AnyElement> = pool
            .into_iter()
            .enumerate()
            .map(|(i, a)| {
                let install_name = a.name.clone();
                row_base(i)
                    .child(result_text(a.name.clone(), vec![a.description.clone()]))
                    .child(icon_button(
                        gpui::SharedString::from(format!("add-skill-{}", a.name)),
                        Icon::Plus,
                        cx.listener(move |this, _, _, cx| {
                            let n = install_name.clone();
                            let global = this.install_global;
                            this.stores
                                .skills
                                .update(cx, |s, cx| s.install(vec![n], global, cx));
                        }),
                    ))
                    .into_any_element()
            })
            .collect();
        let pool_count = pool_rows.len();

        let hit_count = hits.len();
        let hit_rows: Vec<AnyElement> = hits
            .into_iter()
            .enumerate()
            .map(|(i, h)| {
                let id = h.id();
                let install_id = id.clone();
                let meta = if h.stars > 0 {
                    format!("{}  ★ {}", id, h.stars)
                } else {
                    id.clone()
                };
                row_base(i)
                    .child(result_text(
                        h.name.clone(),
                        vec![meta, h.description.clone()],
                    ))
                    .child(icon_button(
                        gpui::SharedString::from(format!("add-hit-{id}")),
                        Icon::Plus,
                        cx.listener(move |this, _, _, cx| {
                            let id = install_id.clone();
                            let global = this.install_global;
                            this.stores.skills.update(cx, |s, cx| {
                                s.review_install("skillsmp".into(), id, global, cx)
                            });
                        }),
                    ))
                    .into_any_element()
            })
            .collect();
        let show_market = hit_count > 0 || searching || search_error.is_some();
        let discover = self.discover;
        let has_filter = !filter.is_empty();

        div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .child(field(&self.search_input))
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .p(px(2.0))
                            .rounded(px(6.0))
                            .border_1()
                            .border_color(theme::border_subtle())
                            .child(segment(
                                "target-project",
                                t("skills.target.project"),
                                !install_global,
                                cx.listener(|this, _, _, cx| {
                                    this.install_global = false;
                                    cx.notify();
                                }),
                            ))
                            .child(segment(
                                "target-global",
                                t("skills.target.global"),
                                install_global,
                                cx.listener(|this, _, _, cx| {
                                    this.install_global = true;
                                    cx.notify();
                                }),
                            )),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .child(segment(
                        "discover-repos",
                        t("skills.group.repos"),
                        discover == DiscoverTab::Repos,
                        cx.listener(|this, _, _, cx| {
                            this.discover = DiscoverTab::Repos;
                            cx.notify();
                        }),
                    ))
                    .child(segment(
                        "discover-market",
                        t("skills.group.market"),
                        discover == DiscoverTab::Market,
                        cx.listener(|this, _, _, cx| {
                            this.discover = DiscoverTab::Market;
                            cx.notify();
                        }),
                    )),
            )
            .when(discover == DiscoverTab::Repos, |s| {
                s.child(group_heading(t("skills.group.repos"), pool_count))
                    .when_some(available_error, |s, e| s.child(muted_note(e)))
                    .when(pool_count == 0 && !has_filter, |s| {
                        s.child(muted_note(t("skills.nothing_to_install").into()))
                    })
                    .when(pool_count == 0 && has_filter, |s| {
                        s.child(muted_note(t("skills.no_match").into()))
                    })
                    .child(
                        self.pool_scroll.framed(
                            div()
                                .id("skills-pool")
                                .flex()
                                .flex_col()
                                .max_h(px(LIST_MAX_H))
                                .children(pool_rows),
                            window,
                        ),
                    )
            })
            .when(discover == DiscoverTab::Market, |s| {
                s.when(!show_market, |s| {
                    s.child(muted_note(t("skills.search_hint").into()))
                })
                .when(show_market, |s| {
                    s.child(group_heading(t("skills.group.market"), hit_count))
                        .when(searching, |s| {
                            s.child(muted_note(t("skills.market.searching").into()))
                        })
                        .when_some(search_error, |s, e| s.child(muted_note(e)))
                        .when(!searching && hit_count > 0, |s| {
                            s.child(
                                self.market_scroll.framed(
                                    div()
                                        .id("skills-market")
                                        .flex()
                                        .flex_col()
                                        .max_h(px(LIST_MAX_H))
                                        .children(hit_rows),
                                    window,
                                ),
                            )
                        })
                })
            })
            .into_any_element()
    }

    fn sources_tab(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let skills = self.stores.skills.read(cx);
        let manifest = skills.snapshot.manifest.clone();
        let is_busy = skills.busy.is_some();
        let repos: Vec<AnyElement> = manifest
            .as_ref()
            .map(|m| m.repos.clone())
            .unwrap_or_default()
            .into_iter()
            .enumerate()
            .map(|(i, repo)| {
                let remove_repo = repo.clone();
                row_base(i)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(12.0))
                            .text_color(theme::text_muted())
                            .child(repo.clone()),
                    )
                    .child(icon_button(
                        gpui::SharedString::from(format!("rm-repo-{repo}")),
                        Icon::Close,
                        cx.listener(move |this, _, _, cx| {
                            let r = remove_repo.clone();
                            this.stores.skills.update(cx, |s, cx| s.remove_repo(r, cx));
                        }),
                    ))
                    .into_any_element()
            })
            .collect();
        let repo_count = repos.len();
        let skills_store = self.stores.skills.clone();

        div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(group_heading(t("skills.group.sources"), repo_count))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(field(&self.repo_input))
                    .child(small_button(
                        "add-repo",
                        t("skills.add_repo"),
                        cx.listener(|this, _, _, cx| this.submit_repo(cx)),
                    )),
            )
            .children(repos)
            .when_some(manifest.as_ref().map(|m| m.isolate), |s, isolate| {
                s.child(
                    div()
                        .mt(px(10.0))
                        .when(is_busy, |d| d.opacity(0.5))
                        .child(checkbox(
                            "skills-isolate",
                            t("skills.isolate"),
                            isolate,
                            move |on, _, cx| {
                                if !is_busy {
                                    skills_store.update(cx, |s, cx| s.set_isolate(on, cx));
                                }
                            },
                        )),
                )
            })
            .into_any_element()
    }
}

impl Render for SkillsPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        for (input, key) in [
            (&self.init_repo_input, "skills.init_placeholder"),
            (&self.search_input, "skills.search_placeholder"),
        ] {
            input.update(cx, |i, _| i.set_placeholder(t(key)));
        }
        let skills = self.stores.skills.read(cx);
        let has_project = skills.project.is_some();
        let busy = skills.busy.clone();
        let has_manifest = skills.snapshot.manifest.is_some();
        let total = skills.doctor.as_ref().map(|d| d.description_tokens());
        let no_manifest = has_project && !has_manifest;
        let ready = has_project && has_manifest;

        let tabs = ready.then(|| {
            div()
                .flex()
                .items_center()
                .gap(px(2.0))
                .child(self.tab_button(
                    "tab-installed",
                    t("skills.tab.installed"),
                    SkillsTab::Installed,
                    cx,
                ))
                .child(self.tab_button(
                    "tab-discover",
                    t("skills.tab.discover"),
                    SkillsTab::Discover,
                    cx,
                ))
                .child(self.tab_button(
                    "tab-sources",
                    t("skills.tab.sources"),
                    SkillsTab::Sources,
                    cx,
                ))
        });
        let content = ready.then(|| match self.tab {
            SkillsTab::Installed => self.installed_tab(window, cx),
            SkillsTab::Discover => self.discover_tab(window, cx),
            SkillsTab::Sources => self.sources_tab(cx),
        });

        div()
            .id("skills-scroll")
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
                                    .flex()
                                    .items_center()
                                    .gap(px(10.0))
                                    .child(
                                        div()
                                            .text_size(px(20.0))
                                            .font_weight(gpui::FontWeight::SEMIBOLD)
                                            .child(t("skills.title")),
                                    )
                                    .when_some(total, |s, tokens| {
                                        s.child(
                                            div()
                                                .px(px(8.0))
                                                .py(px(2.0))
                                                .rounded(px(10.0))
                                                .bg(theme::hover())
                                                .text_size(px(11.5))
                                                .text_color(theme::text_muted())
                                                .child(format!(
                                                    "{tokens} {}",
                                                    t("skills.tokens_in_descriptions")
                                                )),
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(12.0))
                                    .when_some(busy, |s, label| {
                                        s.child(
                                            div()
                                                .text_size(px(12.0))
                                                .text_color(theme::accent())
                                                .child(format!("{label}…")),
                                        )
                                    })
                                    .child(
                                        div()
                                            .id("skills-refresh")
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .size(px(26.0))
                                            .rounded(px(6.0))
                                            .cursor_pointer()
                                            .hover(|s| s.bg(theme::hover()))
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.stores
                                                    .skills
                                                    .update(cx, |s, cx| s.refresh(cx));
                                            }))
                                            .child(
                                                icon(Icon::Refresh, px(15.0))
                                                    .text_color(theme::text()),
                                            ),
                                    ),
                            ),
                    )
                    .when(!has_project, |s| {
                        s.child(muted_note(t("skills.no_project").into()))
                    })
                    .when(no_manifest, |s| {
                        s.child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(8.0))
                                .max_w(px(520.0))
                                .child(
                                    div()
                                        .text_size(px(14.0))
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .child(t("skills.setup_title")),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.5))
                                        .text_color(theme::text_muted())
                                        .child(t("skills.no_manifest")),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(6.0))
                                        .child(field(&self.init_repo_input))
                                        .child(small_button(
                                            "init-skills",
                                            t("skills.initialize"),
                                            cx.listener(|this, _, _, cx| this.submit_init(cx)),
                                        )),
                                ),
                        )
                    })
                    .children(tabs)
                    .children(content),
            )
    }
}
