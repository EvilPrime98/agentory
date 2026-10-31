//! Settings page.

use gpui::{
    AnyElement, Context, Entity, EventEmitter, InteractiveElement, IntoElement, ParentElement,
    Render, Styled, Subscription, Window, div, prelude::*, px,
};

use crate::i18n::t;
use crate::platform::logging;
use crate::route::{GoBack, Navigate};
use crate::state::Stores;
use crate::state::prefs::{MAX_FONT, MIN_FONT};
use crate::ui::components::button::{ButtonVariant, button, small_button};
use crate::ui::components::checkbox::checkbox;
use crate::ui::components::text_input::{TextInput, TextInputEvent};
use crate::ui::theme;

pub struct SettingsPage {
    stores: Stores,
    claude_input: Entity<TextInput>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<Navigate> for SettingsPage {}
impl EventEmitter<GoBack> for SettingsPage {}

fn section(title: &str, body: impl IntoElement) -> AnyElement {
    div()
        .flex()
        .flex_col()
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

impl SettingsPage {
    pub fn new(stores: Stores, cx: &mut Context<Self>) -> Self {
        let initial = stores.prefs.read(cx).prefs.claude_command.clone();
        let claude_input = cx.new(|cx| TextInput::new("claude", cx).with_value(initial));
        let subs = vec![
            cx.observe(&stores.prefs, |_, _, cx| cx.notify()),
            cx.observe(&stores.projects, |_, _, cx| cx.notify()),
            cx.subscribe(&claude_input, |this, input, ev: &TextInputEvent, cx| {
                if matches!(ev, TextInputEvent::Changed) {
                    let value = input.read(cx).value().to_string();
                    this.stores
                        .prefs
                        .update(cx, |p, cx| p.update(cx, |p| p.claude_command = value));
                }
            }),
        ];
        Self {
            stores,
            claude_input,
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
                    .gap(px(14.0))
                    .w_full()
                    .max_w(px(720.0))
                    .p(px(24.0))
                    .child(
                        div()
                            .text_size(px(20.0))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(t("settings.title")),
                    )
                    .child(section(
                        &t("settings.language.title"),
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(10.0))
                            .child(
                                div()
                                    .text_size(px(12.5))
                                    .text_color(theme::text_muted())
                                    .child(t("settings.language.hint")),
                            )
                            .child(small_button(
                                "pref-language",
                                t(language.label_key()),
                                move |_, _, cx| {
                                    language_stores.prefs.update(cx, |p, cx| {
                                        p.update(cx, |p| p.language = p.language.next())
                                    });
                                },
                            )),
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
                    ))
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
                                .children(git_projects.into_iter().map(|(id, name, choice)| {
                                    let store = projects_store.clone();
                                    let (label, next) = match choice {
                                        None => (t("settings.projects.ask"), Some(true)),
                                        Some(true) => (t("settings.projects.worktree"), Some(false)),
                                        Some(false) => (t("settings.projects.folder"), None),
                                    };
                                    div()
                                        .flex()
                                        .items_center()
                                        .justify_between()
                                        .gap(px(10.0))
                                        .child(div().truncate().text_size(px(13.0)).child(name))
                                        .child(small_button(
                                            gpui::SharedString::from(format!("wt-{id}")),
                                            label,
                                            move |_, _, cx| {
                                                store.update(cx, |p, cx| {
                                                    p.set_use_worktree(&id, next, cx)
                                                });
                                            },
                                        ))
                                })),
                        ))
                    })
                    .child(section(
                        &t("settings.terminal.title"),
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.0))
                            .child(div().text_size(px(13.0)).child(t("settings.terminal.font_size")))
                            .child(small_button("font-down", "−", move |_, _, cx| {
                                font_stores.prefs.update(cx, |p, cx| {
                                    p.update(cx, |p| {
                                        p.terminal_font_size = (p.terminal_font_size - 1.0).max(MIN_FONT)
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
                                        p.terminal_font_size = (p.terminal_font_size + 1.0).min(MAX_FONT)
                                    })
                                });
                            })),
                    ))
                    .child(section(
                        &t("settings.data.title"),
                        div()
                            .flex()
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
    }
}
