//! Settings page.

use gpui::{
    AnyElement, Context, Entity, EventEmitter, InteractiveElement, IntoElement, ParentElement,
    Render, SharedString, StatefulInteractiveElement, Styled, Subscription, Window, div,
    prelude::*, px,
};

use crate::i18n::{LanguagePref, t};
use crate::platform::logging;
use crate::route::{GoBack, Navigate};
use crate::state::Stores;
use crate::state::prefs::{DEFAULT_DOC_PROMPT, MAX_FONT, MIN_FONT};
use crate::ui::components::button::{ButtonVariant, button, small_button};
use crate::ui::components::checkbox::checkbox;
use crate::ui::components::scrollbar::Scrollbar;
use crate::ui::components::segmented::segmented;
use crate::ui::components::text_input::{TextInput, TextInputEvent};
use crate::ui::pages::selfbuild_section;
use crate::ui::theme::{self, ThemePref};

#[derive(Clone, Copy, PartialEq, Eq)]
enum SettingsTab {
    General,
    Sessions,
    Terminal,
    Data,
}

impl SettingsTab {
    const ALL: [Self; 4] = [Self::General, Self::Sessions, Self::Terminal, Self::Data];

    fn label_key(self) -> &'static str {
        match self {
            Self::General => "settings.tab.general",
            Self::Sessions => "settings.tab.sessions",
            Self::Terminal => "settings.tab.terminal",
            Self::Data => "settings.tab.data",
        }
    }
}

pub struct SettingsPage {
    scrollbar: Scrollbar,
    stores: Stores,
    claude_input: Entity<TextInput>,
    doc_prompt_input: Entity<TextInput>,
    tab: SettingsTab,
    _subs: Vec<Subscription>,
}

impl EventEmitter<Navigate> for SettingsPage {}
impl EventEmitter<GoBack> for SettingsPage {}

const CONTROL_W: f32 = 280.0;

fn list(rows: Vec<AnyElement>) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .rounded(px(10.0))
        .bg(theme::bg_panel())
        .border_1()
        .border_color(theme::border_subtle())
        .children(rows.into_iter().enumerate().map(|(i, row)| {
            div()
                .px(px(16.0))
                .py(px(12.0))
                .when(i > 0, |d| {
                    d.border_t_1().border_color(theme::border_subtle())
                })
                .child(row)
        }))
}

fn label_block(label: impl Into<SharedString>, hint: Option<SharedString>) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap(px(2.0))
        .flex_1()
        .min_w_0()
        .child(div().text_size(px(13.0)).child(label.into()))
        .when_some(hint, |d, hint| {
            d.child(
                div()
                    .text_size(px(12.0))
                    .text_color(theme::text_muted())
                    .child(hint),
            )
        })
}

fn setting_row(
    label: impl Into<SharedString>,
    hint: Option<SharedString>,
    control: impl IntoElement,
) -> AnyElement {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(16.0))
        .child(label_block(label, hint))
        .child(
            div()
                .flex()
                .flex_none()
                .justify_end()
                .w(px(CONTROL_W))
                .child(control),
        )
        .into_any_element()
}

fn wide_row(
    label: impl Into<SharedString>,
    hint: Option<SharedString>,
    input: Entity<TextInput>,
) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(label_block(label, hint))
        .child(
            div()
                .flex()
                .h(px(32.0))
                .px(px(10.0))
                .rounded(px(6.0))
                .border_1()
                .border_color(theme::border_subtle())
                .bg(theme::bg_app())
                .child(input),
        )
        .into_any_element()
}

fn group_title(title: impl Into<SharedString>) -> gpui::Div {
    div()
        .text_size(px(13.0))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .child(title.into())
}

impl SettingsPage {
    pub fn new(stores: Stores, cx: &mut Context<Self>) -> Self {
        let initial = stores.prefs.read(cx).prefs.claude_command.clone();
        let claude_input = cx.new(|cx| TextInput::new("claude", cx).with_value(initial));
        let initial = stores.prefs.read(cx).prefs.doc_prompt.clone();
        let doc_prompt_input =
            cx.new(|cx| TextInput::new(DEFAULT_DOC_PROMPT, cx).with_value(initial));
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
            cx.subscribe(&doc_prompt_input, |this, input, ev: &TextInputEvent, cx| {
                if matches!(ev, TextInputEvent::Changed) {
                    let value = input.read(cx).value().to_string();
                    this.stores
                        .prefs
                        .update(cx, |p, cx| p.update(cx, |p| p.doc_prompt = value));
                }
            }),
        ];
        selfbuild_section::recheck(&stores, cx);
        Self {
            scrollbar: Scrollbar::new(),
            stores,
            claude_input,
            doc_prompt_input,
            tab: SettingsTab::General,
            _subs: subs,
        }
    }

    fn general_tab(&self, prefs: &crate::state::prefs::Prefs) -> AnyElement {
        let theme_stores = self.stores.clone();
        let language_stores = self.stores.clone();
        let theme_pref = prefs.theme;
        let language = prefs.language;
        list(vec![
            setting_row(
                t("settings.theme.title"),
                Some(t("settings.theme.hint").into()),
                segmented(
                    "pref-theme",
                    ThemePref::ALL
                        .iter()
                        .map(|v| t(v.label_key()).into())
                        .collect(),
                    ThemePref::ALL
                        .iter()
                        .position(|v| *v == theme_pref)
                        .unwrap_or(0),
                    move |i, _, cx| {
                        theme_stores
                            .prefs
                            .update(cx, |p, cx| p.update(cx, |p| p.theme = ThemePref::ALL[i]));
                    },
                )
                .w_full(),
            ),
            setting_row(
                t("settings.language.title"),
                Some(t("settings.language.hint").into()),
                segmented(
                    "pref-language",
                    LanguagePref::ALL
                        .iter()
                        .map(|v| t(v.label_key()).into())
                        .collect(),
                    LanguagePref::ALL
                        .iter()
                        .position(|v| *v == language)
                        .unwrap_or(0),
                    move |i, _, cx| {
                        language_stores.prefs.update(cx, |p, cx| {
                            p.update(cx, |p| p.language = LanguagePref::ALL[i])
                        });
                    },
                )
                .w_full(),
            ),
            wide_row(
                t("settings.claude.title"),
                Some(t("settings.claude.hint").into()),
                self.claude_input.clone(),
            ),
        ])
        .into_any_element()
    }

    fn sessions_tab(&self, prefs: &crate::state::prefs::Prefs, cx: &Context<Self>) -> AnyElement {
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

        let mut col = div().flex().flex_col().gap(px(16.0)).child(list(vec![
            checkbox(
                "pref-worktree",
                t("settings.sessions.worktree"),
                prefs.ask_worktree,
                set(|p, v| p.ask_worktree = v),
            )
            .into_any_element(),
            checkbox(
                "pref-send-doc",
                t("settings.sessions.send_doc"),
                prefs.send_doc_prompt,
                set(|p, v| p.send_doc_prompt = v),
            )
            .into_any_element(),
            wide_row(
                t("settings.sessions.doc_prompt_title"),
                Some(t("settings.sessions.doc_prompt").into()),
                self.doc_prompt_input.clone(),
            ),
            checkbox(
                "pref-notify",
                t("settings.sessions.notify"),
                prefs.notifications,
                set(|p, v| p.notifications = v),
            )
            .into_any_element(),
            checkbox(
                "pref-fullscreen",
                t("settings.sessions.fullscreen"),
                prefs.fullscreen_agents,
                set(|p, v| p.fullscreen_agents = v),
            )
            .into_any_element(),
        ]));

        if !git_projects.is_empty() {
            let project_rows = git_projects
                .into_iter()
                .map(|(id, name, choice)| {
                    let store = projects_store.clone();
                    let selected = match choice {
                        None => 0,
                        Some(true) => 1,
                        Some(false) => 2,
                    };
                    let seg_id = SharedString::from(format!("wt-{id}"));
                    setting_row(
                        name,
                        None,
                        segmented(
                            seg_id,
                            vec![
                                t("settings.projects.ask").into(),
                                t("settings.projects.worktree").into(),
                                t("settings.projects.folder").into(),
                            ],
                            selected,
                            move |i, _, cx| {
                                let value = match i {
                                    0 => None,
                                    1 => Some(true),
                                    _ => Some(false),
                                };
                                store.update(cx, |p, cx| p.set_use_worktree(&id, value, cx));
                            },
                        )
                        .w_full(),
                    )
                })
                .collect();
            col = col
                .child(group_title(t("settings.projects.title")))
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(theme::text_muted())
                        .child(t("settings.projects.hint")),
                )
                .child(list(project_rows));
        }
        col.into_any_element()
    }

    fn terminal_tab(&self, prefs: &crate::state::prefs::Prefs) -> AnyElement {
        let font = prefs.terminal_font_size;
        let down = self.stores.clone();
        let up = self.stores.clone();
        let debug = self.stores.clone();
        list(vec![setting_row(
            t("settings.terminal.font_size"),
            None,
            div()
                .flex()
                .items_center()
                .gap(px(10.0))
                .child(small_button("font-down", "−", move |_, _, cx| {
                    down.prefs.update(cx, |p, cx| {
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
                    up.prefs.update(cx, |p, cx| {
                        p.update(cx, |p| {
                            p.terminal_font_size = (p.terminal_font_size + 1.0).min(MAX_FONT)
                        })
                    });
                })),
        ),
        checkbox(
            "pref-terminal-debug",
            t("settings.terminal.debug"),
            prefs.terminal_debug,
            move |value: bool, _: &mut Window, cx: &mut gpui::App| {
                debug
                    .prefs
                    .update(cx, |p, cx| p.update(cx, |p| p.terminal_debug = value));
            },
        )
        .into_any_element()])
        .into_any_element()
    }

    fn data_tab(&self, cx: &Context<Self>) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap(px(16.0))
            .child(list(vec![setting_row(
                t("settings.data.title"),
                None,
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
            )]))
            .child(group_title(t("selfbuild.title")))
            .child(
                div()
                    .p(px(16.0))
                    .rounded(px(10.0))
                    .bg(theme::bg_panel())
                    .border_1()
                    .border_color(theme::border_subtle())
                    .child(selfbuild_section::body(&self.stores, cx)),
            )
            .into_any_element()
    }
}

impl Render for SettingsPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let prefs = self.stores.prefs.read(cx).prefs.clone();
        let tab = self.tab;

        let tab_bar = div()
            .flex()
            .gap(px(4.0))
            .border_b_1()
            .border_color(theme::border_subtle())
            .children(SettingsTab::ALL.into_iter().map(|item| {
                let active = item == tab;
                div()
                    .id(SharedString::from(format!("settings-{}", item.label_key())))
                    .px(px(14.0))
                    .py(px(8.0))
                    .text_size(px(13.0))
                    .cursor_pointer()
                    .border_b_2()
                    .border_color(if active {
                        theme::accent()
                    } else {
                        gpui::rgba(0)
                    })
                    .text_color(if active {
                        theme::text()
                    } else {
                        theme::text_secondary()
                    })
                    .hover(|s| s.text_color(theme::text()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.tab = item;
                        cx.notify();
                    }))
                    .child(t(item.label_key()))
            }));

        let content = match tab {
            SettingsTab::General => self.general_tab(&prefs),
            SettingsTab::Sessions => self.sessions_tab(&prefs, cx),
            SettingsTab::Terminal => self.terminal_tab(&prefs),
            SettingsTab::Data => self.data_tab(cx),
        };

        self.scrollbar.overlay(
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
                        .gap(px(16.0))
                        .w_full()
                        .max_w(px(760.0))
                        .p(px(24.0))
                        .child(
                            div()
                                .text_size(px(20.0))
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .child(t("settings.title")),
                        )
                        .child(tab_bar)
                        .child(content),
                ),
        )
    }
}
