//! Settings page.

use gpui::{
    AnyElement, Context, Entity, EventEmitter, InteractiveElement, IntoElement, ParentElement,
    Render, Styled, Subscription, Window, div, prelude::*, px,
};

use std::path::Path;

use crate::i18n::t;
use crate::platform::logging;
use crate::route::{GoBack, Navigate};
use crate::runtime;
use crate::state::Stores;
use crate::state::prefs::{MAX_FONT, MIN_FONT};
use crate::ui::components::button::{ButtonVariant, button, small_button};
use crate::ui::components::checkbox::checkbox;
use crate::ui::components::text_input::{TextInput, TextInputEvent};
use crate::ui::icons::Icon;
use crate::ui::pages::selfbuild_section;
use crate::ui::pages::skills::icon_button;
use crate::ui::theme;
use crate::ui::toast;

pub struct SettingsPage {
    stores: Stores,
    claude_input: Entity<TextInput>,
    picker_open: bool,
    _subs: Vec<Subscription>,
}

impl EventEmitter<Navigate> for SettingsPage {}
impl EventEmitter<GoBack> for SettingsPage {}

/// Narrowest a card gets before the row wraps and the cards stack.
const CARD_MIN_W: f32 = 340.0;

const CYCLE_BTN_W: f32 = 96.0;

/// A wrapping row of cards that share the width evenly.
fn row() -> gpui::Div {
    div().flex().flex_wrap().items_start().gap(px(12.0))
}

fn section(title: &str, body: impl IntoElement) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .flex_1()
        .min_w(px(CARD_MIN_W))
        .gap(px(10.0))
        .p(px(16.0))
        .rounded(px(10.0))
        .bg(theme::bg_panel())
        .border_1()
        .border_color(theme::border_subtle())
        .child(
            div()
                .text_size(px(13.0))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .child(title.to_string()),
        )
        .child(body)
        .into_any_element()
}

fn relative_folder(root: &Path, picked: &Path) -> Option<String> {
    let root = root.canonicalize().ok()?;
    let picked = picked.canonicalize().ok()?;
    let rel = picked.strip_prefix(&root).ok()?;
    let parts: Vec<_> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    (!parts.is_empty()).then(|| parts.join("/"))
}

impl SettingsPage {
    fn add_docs_folder(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self
            .stores
            .projects
            .read(cx)
            .selected_project()
            .map(|p| p.path.clone())
        else {
            return;
        };
        if self.picker_open {
            return;
        }
        self.picker_open = true;
        let start = root.clone();
        cx.spawn(async move |this, cx| {
            let picked = runtime::run(async move {
                rfd::AsyncFileDialog::new()
                    .set_title(t("settings.docs.pick"))
                    .set_directory(start)
                    .pick_folder()
                    .await
                    .map(|h| h.path().to_path_buf())
            })
            .await;
            this.update(cx, |this, cx| {
                this.picker_open = false;
                if let Some(picked) = picked {
                    match relative_folder(&root, &picked) {
                        Some(rel) => this.stores.prefs.update(cx, |p, cx| {
                            p.update(cx, |p| {
                                if !p.docs_folders.contains(&rel) {
                                    p.docs_folders.push(rel);
                                }
                            })
                        }),
                        None => toast::error(cx, t("settings.docs.outside")),
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn new(stores: Stores, cx: &mut Context<Self>) -> Self {
        let initial = stores.prefs.read(cx).prefs.claude_command.clone();
        let claude_input = cx.new(|cx| TextInput::new("claude", cx).with_value(initial));
        let subs = vec![
            cx.observe(&stores.prefs, |_, _, cx| cx.notify()),
            cx.observe(&stores.projects, |_, _, cx| cx.notify()),
            cx.observe(&stores.selfbuild, |_, _, cx| cx.notify()),
            cx.subscribe(&claude_input, |this, input, ev: &TextInputEvent, cx| {
                if matches!(ev, TextInputEvent::Changed) {
                    let value = input.read(cx).value().to_string();
                    this.stores
                        .prefs
                        .update(cx, |p, cx| p.update(cx, |p| p.claude_command = value));
                }
            }),
        ];
        selfbuild_section::recheck(&stores, cx);
        Self {
            stores,
            claude_input,
            picker_open: false,
            _subs: subs,
        }
    }
}

impl Render for SettingsPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let prefs = self.stores.prefs.read(cx).prefs.clone();
        let stores = self.stores.clone();
        let set = move |change: fn(&mut crate::state::prefs::Prefs, bool)| {
            let stores = stores.clone();
            move |value: bool, _: &mut Window, cx: &mut gpui::App| {
                stores
                    .prefs
                    .update(cx, |p, cx| p.update(cx, |p| change(p, value)));
            }
        };
        let git_projects: Vec<_> = self
            .stores
            .projects
            .read(cx)
            .items
            .iter()
            .filter(|p| p.is_git_repo())
            .map(|p| (p.id.clone(), p.name.clone(), p.use_worktree))
            .collect();
        let projects_store = self.stores.projects.clone();
        let font = prefs.terminal_font_size;
        let font_stores = self.stores.clone();
        let font_stores_up = self.stores.clone();
        let language = prefs.language;
        let language_stores = self.stores.clone();
        let has_project = self.stores.projects.read(cx).selected_project().is_some();
        let add_page = cx.entity();
        let docs_folders = prefs.docs_folders.clone();
        let theme_pref = prefs.theme;
        let theme_stores = self.stores.clone();

        div()
            .id("settings-scroll")
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
                    .gap(px(12.0))
                    .w_full()
                    .max_w(px(1100.0))
                    .p(px(24.0))
                    .child(
                        div()
                            .text_size(px(20.0))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(t("settings.title")),
                    )
                    .child(
                        row()
                            .child(section(
                                &t("settings.theme.title"),
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap(px(10.0))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .text_size(px(12.5))
                                            .text_color(theme::text_muted())
                                            .child(t("settings.theme.hint")),
                                    )
                                    .child(
                                        small_button(
                                            "pref-theme",
                                            t(theme_pref.label_key()),
                                            move |_, _, cx| {
                                                theme_stores.prefs.update(cx, |p, cx| {
                                                    p.update(cx, |p| p.theme = p.theme.next())
                                                });
                                            },
                                        )
                                        .flex_none()
                                        .w(px(CYCLE_BTN_W)),
                                    ),
                            ))
                            .child(section(
                                &t("settings.language.title"),
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap(px(10.0))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .text_size(px(12.5))
                                            .text_color(theme::text_muted())
                                            .child(t("settings.language.hint")),
                                    )
                                    .child(
                                        small_button(
                                            "pref-language",
                                            t(language.label_key()),
                                            move |_, _, cx| {
                                                language_stores.prefs.update(cx, |p, cx| {
                                                    p.update(cx, |p| p.language = p.language.next())
                                                });
                                            },
                                        )
                                        .flex_none()
                                        .w(px(CYCLE_BTN_W)),
                                    ),
                            ))
                            .child(section(
                                &t("settings.claude.title"),
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(8.0))
                                    .child(
                                        div()
                                            .text_size(px(12.5))
                                            .text_color(theme::text_muted())
                                            .child(t("settings.claude.hint")),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .h(px(32.0))
                                            .px(px(10.0))
                                            .rounded(px(6.0))
                                            .border_1()
                                            .border_color(theme::border_subtle())
                                            .bg(theme::bg_app())
                                            .child(self.claude_input.clone()),
                                    ),
                            )),
                    )
                    .child(
                        row()
                            .child(section(
                                &t("settings.sessions.title"),
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(10.0))
                                    .child(checkbox(
                                        "pref-worktree",
                                        t("settings.sessions.worktree"),
                                        prefs.ask_worktree,
                                        set(|p, v| p.ask_worktree = v),
                                    ))
                                    .child(checkbox(
                                        "pref-send-doc",
                                        t("settings.sessions.send_doc"),
                                        prefs.send_doc_prompt,
                                        set(|p, v| p.send_doc_prompt = v),
                                    ))
                                    .child(checkbox(
                                        "pref-notify",
                                        t("settings.sessions.notify"),
                                        prefs.notifications,
                                        set(|p, v| p.notifications = v),
                                    ))
                                    .child(checkbox(
                                        "pref-fullscreen",
                                        t("settings.sessions.fullscreen"),
                                        prefs.fullscreen_agents,
                                        set(|p, v| p.fullscreen_agents = v),
                                    )),
                            ))
                            .when(!git_projects.is_empty(), |s| {
                                s.child(section(
                                    &t("settings.projects.title"),
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap(px(8.0))
                                        .child(
                                            div()
                                                .text_size(px(12.5))
                                                .text_color(theme::text_muted())
                                                .child(t("settings.projects.hint")),
                                        )
                                        .children(git_projects.into_iter().map(
                                            |(id, name, choice)| {
                                                let store = projects_store.clone();
                                                let (label, next) = match choice {
                                                    None => {
                                                        (t("settings.projects.ask"), Some(true))
                                                    }
                                                    Some(true) => (
                                                        t("settings.projects.worktree"),
                                                        Some(false),
                                                    ),
                                                    Some(false) => {
                                                        (t("settings.projects.folder"), None)
                                                    }
                                                };
                                                div()
                                                    .flex()
                                                    .items_center()
                                                    .justify_between()
                                                    .gap(px(10.0))
                                                    .child(
                                                        div()
                                                            .flex_1()
                                                            .min_w_0()
                                                            .truncate()
                                                            .text_size(px(13.0))
                                                            .child(name),
                                                    )
                                                    .child(small_button(
                                                        gpui::SharedString::from(format!(
                                                            "wt-{id}"
                                                        )),
                                                        label,
                                                        move |_, _, cx| {
                                                            store.update(cx, |p, cx| {
                                                                p.set_use_worktree(&id, next, cx)
                                                            });
                                                        },
                                                    ))
                                            },
                                        )),
                                ))
                            }),
                    )
                    .child(
                        row().child(section(
                            &t("settings.docs.title"),
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(8.0))
                                .child(
                                    div()
                                        .text_size(px(12.5))
                                        .text_color(theme::text_muted())
                                        .child(t("settings.docs.hint")),
                                )
                                .children(docs_folders.into_iter().map(|folder| {
                                    let stores = self.stores.clone();
                                    let target = folder.clone();
                                    div()
                                        .flex()
                                        .items_center()
                                        .justify_between()
                                        .gap(px(10.0))
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .truncate()
                                                .text_size(px(13.0))
                                                .child(folder.clone()),
                                        )
                                        .child(icon_button(
                                            gpui::SharedString::from(format!("rm-docs-{folder}")),
                                            Icon::Trash,
                                            move |_, _, cx| {
                                                stores.prefs.update(cx, |p, cx| {
                                                    p.update(cx, |p| {
                                                        p.docs_folders.retain(|f| *f != target)
                                                    })
                                                });
                                            },
                                        ))
                                }))
                                .child(div().flex().items_center().gap(px(10.0)).child(
                                    if has_project {
                                        small_button(
                                            "add-docs-folder",
                                            t("settings.docs.add"),
                                            move |_, _, cx| {
                                                add_page
                                                    .update(cx, |this, cx| this.add_docs_folder(cx))
                                            },
                                        )
                                        .into_any_element()
                                    } else {
                                        div()
                                            .text_size(px(12.5))
                                            .text_color(theme::text_muted())
                                            .child(t("settings.docs.no_project"))
                                            .into_any_element()
                                    },
                                )),
                        )),
                    )
                    .child(
                        row()
                            .child(section(
                                &t("settings.terminal.title"),
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .items_center()
                                    .gap(px(10.0))
                                    .child(
                                        div()
                                            .text_size(px(13.0))
                                            .child(t("settings.terminal.font_size")),
                                    )
                                    .child(small_button("font-down", "−", move |_, _, cx| {
                                        font_stores.prefs.update(cx, |p, cx| {
                                            p.update(cx, |p| {
                                                p.terminal_font_size =
                                                    (p.terminal_font_size - 1.0).max(MIN_FONT)
                                            })
                                        });
                                    }))
                                    .child(
                                        div()
                                            .w(px(36.0))
                                            .flex()
                                            .justify_center()
                                            .text_size(px(13.0))
                                            .child(format!("{font:.0}")),
                                    )
                                    .child(small_button("font-up", "+", move |_, _, cx| {
                                        font_stores_up.prefs.update(cx, |p, cx| {
                                            p.update(cx, |p| {
                                                p.terminal_font_size =
                                                    (p.terminal_font_size + 1.0).min(MAX_FONT)
                                            })
                                        });
                                    })),
                            ))
                            .child(section(
                                &t("settings.data.title"),
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .gap(px(8.0))
                                    .child(button(
                                        "open-logs",
                                        t("settings.data.open_logs"),
                                        ButtonVariant::Secondary,
                                        |_, _, _| {
                                            let _ = open::that(logging::log_dir());
                                        },
                                    ))
                                    .child(button(
                                        "open-config",
                                        t("settings.data.open_config"),
                                        ButtonVariant::Secondary,
                                        |_, _, _| {
                                            if let Some(dir) = dirs::config_dir() {
                                                let _ = open::that(dir.join("Agentory"));
                                            }
                                        },
                                    )),
                            )),
                    )
                    .child(row().child(section(
                        &t("selfbuild.title"),
                        selfbuild_section::body(&self.stores, cx),
                    ))),
            )
    }
}
