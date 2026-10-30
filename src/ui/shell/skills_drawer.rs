//! Right-hand drawer: the selected project's skills, managed through `spm`.

use std::collections::BTreeSet;

use gpui::{
    AnyElement, Context, Entity, InteractiveElement, IntoElement, ParentElement, Render,
    StatefulInteractiveElement, Styled, Subscription, Window, div, prelude::*, px,
};

use crate::state::Stores;
use crate::ui::components::button::small_button;
use crate::ui::components::text_input::{TextInput, TextInputEvent};
use crate::ui::icons::{Icon, icon};
use crate::ui::theme;

pub struct SkillsDrawer {
    stores: Stores,
    install_input: Entity<TextInput>,
    repo_input: Entity<TextInput>,
    _subs: Vec<Subscription>,
}

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

fn section_title(text: &'static str) -> impl IntoElement {
    div()
        .mt(px(14.0))
        .mb(px(6.0))
        .text_size(px(11.0))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(theme::text_muted())
        .child(text)
}

impl SkillsDrawer {
    pub fn new(stores: Stores, cx: &mut Context<Self>) -> Self {
        let install_input = cx.new(|cx| TextInput::new("skill or market:skill", cx));
        let repo_input = cx.new(|cx| TextInput::new("owner/repo", cx));
        let subs = vec![
            cx.observe(&stores.skills, |_, _, cx| cx.notify()),
            cx.observe(&stores.projects, |_, _, cx| cx.notify()),
            cx.subscribe(&install_input, |this, _, ev: &TextInputEvent, cx| {
                if matches!(ev, TextInputEvent::Submit) {
                    this.submit_install(cx);
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

    fn submit_repo(&mut self, cx: &mut Context<Self>) {
        let repo = self.repo_input.read(cx).value().trim().to_string();
        if repo.is_empty() {
            return;
        }
        self.repo_input.update(cx, |i, cx| i.set_value("", cx));
        self.stores.skills.update(cx, |s, cx| s.add_repo(repo, cx));
    }
}

impl Render for SkillsDrawer {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
        let rows: Vec<AnyElement> = names
            .into_iter()
            .map(|name| {
                let on_disk = skills.snapshot.installed.contains(&name);
                let source = manifest.as_ref().and_then(|m| m.source_of(&name));
                let cost = doctor.as_ref().and_then(|d| {
                    d.skills
                        .iter()
                        .find(|s| s.name == name)
                        .map(|s| s.description_tokens)
                });
                let remove_name = name.clone();
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .py(px(4.0))
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
                                .text_size(px(11.0))
                                .text_color(theme::text_muted())
                                .child(format!("{tokens} tok")),
                        )
                    })
                    .child(small_button(
                        gpui::SharedString::from(format!("rm-skill-{}", name.clone())),
                        "Remove",
                        cx.listener(move |this, _, _, cx| {
                            let n = remove_name.clone();
                            this.stores.skills.update(cx, |s, cx| s.uninstall(n, cx));
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
            .map(|repo| {
                let remove_repo = repo.clone();
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .py(px(2.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(12.0))
                            .text_color(theme::text_muted())
                            .child(repo.clone()),
                    )
                    .child(small_button(
                        gpui::SharedString::from(format!("rm-repo-{}", repo)),
                        "×",
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
            .flex()
            .flex_col()
            .flex_none()
            .w(theme::drawer_width())
            .h_full()
            .bg(theme::bg_panel())
            .border_l_1()
            .border_color(theme::border_subtle())
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_between()
                    .h(px(40.0))
                    .px(px(14.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(icon(Icon::Puzzle, px(16.0)).text_color(theme::accent()))
                            .child(
                                div()
                                    .text_size(px(13.0))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child("Skills"),
                            ),
                    )
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
                            .child(icon(Icon::Refresh, px(15.0)).text_color(theme::text())),
                    ),
            )
            .child(
                div()
                    .id("skills-scroll")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .px(px(14.0))
                    .pb(px(14.0))
                    .overflow_y_scroll()
                    .when(!has_project, |s| {
                        s.child(
                            div()
                                .text_size(px(12.5))
                                .text_color(theme::text_muted())
                                .child("Select a project to manage its skills."),
                        )
                    })
                    .when(no_manifest, |s| {
                        s.child(
                            div()
                                .text_size(px(12.5))
                                .text_color(theme::text_muted())
                                .child(
                                    "This project has no spm.json. Run `spm init` in the project folder.",
                                ),
                        )
                    })
                    .when(has_project && !no_manifest, |s| {
                        s.child(section_title("INSTALLED"))
                            .children(rows)
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.0))
                                    .mt(px(6.0))
                                    .child(field(&self.install_input))
                                    .child(small_button(
                                        "install-skill",
                                        "Install",
                                        cx.listener(|this, _, _, cx| this.submit_install(cx)),
                                    )),
                            )
                            .child(section_title("REPOS"))
                            .children(repos)
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.0))
                                    .mt(px(6.0))
                                    .child(field(&self.repo_input))
                                    .child(small_button(
                                        "add-repo",
                                        "Add",
                                        cx.listener(|this, _, _, cx| this.submit_repo(cx)),
                                    )),
                            )
                            .child(section_title("CONTEXT COST"))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.0))
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
                                    .when_some(total, |s, t| {
                                        s.child(
                                            div()
                                                .text_size(px(12.0))
                                                .text_color(theme::text_muted())
                                                .child(format!("{t} tokens in descriptions")),
                                        )
                                    }),
                            )
                    }),
            )
            .when_some(busy, |s, label| {
                s.child(
                    div()
                        .flex_none()
                        .px(px(14.0))
                        .py(px(8.0))
                        .border_t_1()
                        .border_color(theme::border_subtle())
                        .text_size(px(12.0))
                        .text_color(theme::accent())
                        .child(format!("{label}…")),
                )
            })
    }
}
