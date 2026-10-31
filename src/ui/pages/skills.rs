//! Skills page: a project's skills, managed through `spm`.

use std::collections::BTreeSet;

use gpui::{
    AnyElement, Context, Entity, EventEmitter, InteractiveElement, IntoElement, ParentElement,
    Render, StatefulInteractiveElement, Styled, Subscription, Window, div, prelude::*, px,
};

use crate::route::{GoBack, Navigate, Route};
use crate::state::Stores;
use crate::ui::components::button::small_button;
use crate::ui::components::checkbox::checkbox;
use crate::ui::components::smooth_scroll::SmoothScroll;
use crate::ui::components::text_input::{TextInput, TextInputEvent};
use crate::ui::icons::{Icon, icon};
use crate::ui::theme;

pub struct SkillsPage {
    stores: Stores,
    install_input: Entity<TextInput>,
    repo_input: Entity<TextInput>,
    init_repo_input: Entity<TextInput>,
    filter_input: Entity<TextInput>,
    installed_scroll: SmoothScroll,
    available_scroll: SmoothScroll,
    _subs: Vec<Subscription>,
}

/// Narrowest a card gets before the row wraps and the cards stack.
const CARD_MIN_W: f32 = 340.0;

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
    title: &'static str,
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
                                .child(title),
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

impl EventEmitter<Navigate> for SkillsPage {}
impl EventEmitter<GoBack> for SkillsPage {}

impl SkillsPage {
    pub fn new(stores: Stores, cx: &mut Context<Self>) -> Self {
        let install_input = cx.new(|cx| TextInput::new("skill or market:skill", cx));
        let repo_input = cx.new(|cx| TextInput::new("owner/repo", cx));
        let init_repo_input = cx.new(|cx| TextInput::new("owner/repo (optional)", cx));
        let filter_input = cx.new(|cx| TextInput::new("filter available skills", cx));
        let subs = vec![
            cx.observe(&stores.skills, |_, _, cx| cx.notify()),
            cx.observe(&stores.projects, |_, _, cx| cx.notify()),
            cx.subscribe(&install_input, |this, _, ev: &TextInputEvent, cx| {
                if matches!(ev, TextInputEvent::Submit) {
                    this.submit_install(cx);
                }
            }),
            cx.subscribe(&filter_input, |_, _, ev: &TextInputEvent, cx| {
                if matches!(ev, TextInputEvent::Changed) {
                    cx.notify();
                }
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
            install_input,
            repo_input,
            init_repo_input,
            filter_input,
            installed_scroll: SmoothScroll::new(),
            available_scroll: SmoothScroll::new(),
            _subs: subs,
        }
    }

    fn submit_install(&mut self, cx: &mut Context<Self>) {
        let names: Vec<String> = self
            .install_input
            .read(cx)
            .value()
            .split_whitespace()
            .map(str::to_string)
            .collect();
        if names.is_empty() {
            return;
        }
        self.install_input.update(cx, |i, cx| i.set_value("", cx));
        self.stores.skills.update(cx, |s, cx| s.install(names, cx));
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
}

impl Render for SkillsPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let skills = self.stores.skills.read(cx);
        let has_project = skills.project.is_some();
        let busy = skills.busy.clone();
        let manifest = skills.snapshot.manifest.clone();
        let doctor = skills.doctor.clone();

        // Declared in spm.json plus anything on disk.
        let mut names: BTreeSet<String> = skills.snapshot.installed.iter().cloned().collect();
        if let Some(m) = &manifest {
            names.extend(m.skills.keys().cloned());
        }
        let installed_names = names.clone();
        let project_id = self.stores.projects.read(cx).selected.clone();
        let rows: Vec<AnyElement> = names
            .into_iter()
            .enumerate()
            .map(|(i, name)| {
                let on_disk = skills.snapshot.installed.contains(&name);
                let source = manifest.as_ref().and_then(|m| m.source_of(&name));
                let cost = doctor.as_ref().and_then(|d| {
                    d.skills
                        .iter()
                        .find(|s| s.name == name)
                        .map(|s| s.description_tokens)
                });
                let remove_name = name.clone();
                let edit_route = project_id
                    .clone()
                    .filter(|_| on_disk)
                    .map(|id| Route::SkillEditor(id, name.clone()));
                let name_route = edit_route.clone();
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
                            .when_some(name_route, |s, route| {
                                s.cursor_pointer().on_click(cx.listener(move |_, _, _, cx| {
                                    cx.emit(Navigate(route.clone()));
                                }))
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
                                        format!("{name} (not installed)")
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
                                .child(format!("{tokens} tok")),
                        )
                    })
                    .when_some(edit_route, |s, route| {
                        s.child(icon_button(
                            gpui::SharedString::from(format!("edit-skill-{name}")),
                            Icon::Pencil,
                            cx.listener(move |_, _, _, cx| cx.emit(Navigate(route.clone()))),
                        ))
                    })
                    .child(icon_button(
                        gpui::SharedString::from(format!("rm-skill-{}", name.clone())),
                        Icon::Trash,
                        cx.listener(move |this, _, _, cx| {
                            let n = remove_name.clone();
                            this.stores.skills.update(cx, |s, cx| s.uninstall(n, cx));
                        }),
                    ))
                    .into_any_element()
            })
            .collect();
        let filter = self.filter_input.read(cx).value().trim().to_lowercase();
        let pool: Vec<_> = skills
            .available
            .iter()
            .filter(|a| !installed_names.contains(&a.name))
            .cloned()
            .collect();
        let available_error = skills.available_error.clone();
        let has_available_error = available_error.is_some();
        let nothing_new = pool.is_empty();
        let matches: Vec<_> = pool
            .into_iter()
            .filter(|a| {
                filter.is_empty()
                    || a.name.to_lowercase().contains(&filter)
                    || a.description.to_lowercase().contains(&filter)
            })
            .collect();
        let no_match = !nothing_new && matches.is_empty();
        let available_rows: Vec<AnyElement> = matches
            .into_iter()
            .enumerate()
            .map(|(i, a)| {
                let install_name = a.name.clone();
                row_base(i)
                    .child(
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
                                    .child(a.name.clone()),
                            )
                            .when(!a.description.is_empty(), |s| {
                                s.child(
                                    div()
                                        .truncate()
                                        .text_size(px(11.0))
                                        .text_color(theme::text_muted())
                                        .child(a.description.clone()),
                                )
                            }),
                    )
                    .child(icon_button(
                        gpui::SharedString::from(format!("add-skill-{}", a.name)),
                        Icon::Plus,
                        cx.listener(move |this, _, _, cx| {
                            let n = install_name.clone();
                            this.stores
                                .skills
                                .update(cx, |s, cx| s.install(vec![n], cx));
                        }),
                    ))
                    .into_any_element()
            })
            .collect();
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
                        gpui::SharedString::from(format!("rm-repo-{}", repo)),
                        Icon::Close,
                        cx.listener(move |this, _, _, cx| {
                            let r = remove_repo.clone();
                            this.stores.skills.update(cx, |s, cx| s.remove_repo(r, cx));
                        }),
                    ))
                    .into_any_element()
            })
            .collect();
        let total = doctor.as_ref().map(|d| d.description_tokens());
        let no_manifest = has_project && manifest.is_none();

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
                                    .text_size(px(20.0))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child("Skills"),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(12.0))
                                    .when_some(manifest.as_ref().map(|m| m.isolate), |s, isolate| {
                                        let skills = self.stores.skills.clone();
                                        let is_busy = busy.is_some();
                                        s.child(
                                            div()
                                                .when(is_busy, |d| d.opacity(0.5))
                                                .child(checkbox(
                                                    "skills-isolate",
                                                    "Isolate from global skills",
                                                    isolate,
                                                    move |on, _, cx| {
                                                        if !is_busy {
                                                            skills.update(cx, |s, cx| {
                                                                s.set_isolate(on, cx)
                                                            });
                                                        }
                                                    },
                                                )),
                                        )
                                    })
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
                                                this.stores.skills.update(cx, |s, cx| s.refresh(cx));
                                            }))
                                            .child(
                                                icon(Icon::Refresh, px(15.0))
                                                    .text_color(theme::text()),
                                            ),
                                    ),
                            ),
                    )
                    .when(no_manifest, |s| {
                        s.child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(8.0))
                                .child(
                                    div()
                                        .text_size(px(12.5))
                                        .text_color(theme::text_muted())
                                        .child(
                                            "This project has no spm.json. Initializing creates it and hides global skills in this project.",
                                        ),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(6.0))
                                        .max_w(px(480.0))
                                        .child(field(&self.init_repo_input))
                                        .child(small_button(
                                            "init-skills",
                                            "Initialize skills",
                                            cx.listener(|this, _, _, cx| this.submit_init(cx)),
                                        )),
                                ),
                        )
                    })
                    .when(has_project && !no_manifest, |s| {
                        let cost_actions = div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .child(small_button(
                                "run-doctor",
                                "Measure",
                                cx.listener(|this, _, _, cx| {
                                    this.stores.skills.update(cx, |s, cx| s.run_doctor(cx));
                                }),
                            ))
                            .child(small_button(
                                "update-skills",
                                "Update all",
                                cx.listener(|this, _, _, cx| {
                                    this.stores.skills.update(cx, |s, cx| s.update_all(cx));
                                }),
                            ))
                            .into_any_element();
                        let cost_body: Vec<AnyElement> = total
                            .map(|t| {
                                div()
                                    .flex()
                                    .items_baseline()
                                    .gap(px(6.0))
                                    .child(
                                        div()
                                            .text_size(px(24.0))
                                            .font_weight(gpui::FontWeight::SEMIBOLD)
                                            .child(t.to_string()),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(12.0))
                                            .text_color(theme::text_muted())
                                            .child("tokens in descriptions"),
                                    )
                                    .into_any_element()
                            })
                            .into_iter()
                            .collect();

                        let installed_count = rows.len();
                        let mut installed_body: Vec<AnyElement> = vec![
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.0))
                                .child(field(&self.install_input))
                                .child(small_button(
                                    "install-skill",
                                    "Install",
                                    cx.listener(|this, _, _, cx| this.submit_install(cx)),
                                ))
                                .into_any_element(),
                        ];
                        installed_body.push(
                            self.installed_scroll
                                .attach(
                                    div()
                                        .id("skills-installed")
                                        .flex()
                                        .flex_col()
                                        .max_h(px(360.0))
                                        .children(rows),
                                    window,
                                )
                                .into_any_element(),
                        );

                        let available_count = available_rows.len();
                        let mut available_body: Vec<AnyElement> =
                            vec![div().flex().child(field(&self.filter_input)).into_any_element()];
                        if let Some(e) = available_error {
                            available_body.push(muted_note(e).into_any_element());
                        }
                        if nothing_new && !has_available_error {
                            available_body.push(muted_note("Nothing to install".into()).into_any_element());
                        }
                        if no_match {
                            available_body.push(muted_note("No skills match".into()).into_any_element());
                        }
                        available_body.push(
                            self.available_scroll
                                .attach(
                                    div()
                                        .id("skills-available")
                                        .flex()
                                        .flex_col()
                                        .max_h(px(360.0))
                                        .children(available_rows),
                                    window,
                                )
                                .into_any_element(),
                        );

                        let repo_count = repos.len();
                        let mut repo_body: Vec<AnyElement> = vec![
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.0))
                                .child(field(&self.repo_input))
                                .child(small_button(
                                    "add-repo",
                                    "Add",
                                    cx.listener(|this, _, _, cx| this.submit_repo(cx)),
                                ))
                                .into_any_element(),
                        ];
                        repo_body.extend(repos);

                        // Summary and sources on top, the two long lists side by side below;
                        // each row wraps into a stack when the window is narrow.
                        let row = || div().flex().flex_wrap().items_start().gap(px(12.0));
                        s.child(
                            row()
                                .child(card("Context cost", None, Some(cost_actions), cost_body))
                                .child(card("Repos", Some(repo_count), None, repo_body)),
                        )
                        .child(
                            row()
                                .child(card("Installed", Some(installed_count), None, installed_body))
                                .child(card("Available", Some(available_count), None, available_body)),
                        )
                    }),
            )
    }
}
