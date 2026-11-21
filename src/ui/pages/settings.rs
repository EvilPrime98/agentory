//! Settings page.

use gpui::{
    AnyElement, Context, Div, Entity, EventEmitter, InteractiveElement, IntoElement, ParentElement,
    Render, SharedString, StatefulInteractiveElement, Styled, Subscription, Window, div,
    prelude::*, px,
};

use crate::i18n::{LanguagePref, t, t_args};
use crate::platform::logging;
use crate::route::{GoBack, Navigate};
use crate::state::Stores;
use crate::state::prefs::{
    AUTO_ENTER_STEP_MS, DEFAULT_DOC_PROMPT, DEFAULT_ISSUE_PROMPT, DEFAULT_WORKFLOW_TOOLS,
    MAX_AUTO_ENTER_MS, MAX_FONT, MAX_VAD_SILENCE_MS, MIN_AUTO_ENTER_MS, MIN_FONT,
    MIN_VAD_SILENCE_MS, SpeechLanguage, VAD_SILENCE_STEP_MS,
};
use crate::state::prefs::{Prefs, SessionView};
use crate::ui::components::button::{ButtonVariant, button, small_button};
use crate::ui::components::checkbox::checkbox;
use crate::ui::components::dropdown::dropdown;
use crate::ui::components::scrollbar::Scrollbar;
use crate::ui::components::segmented::segmented;
use crate::ui::components::text_area::{TextArea, TextAreaEvent};
use crate::ui::components::text_input::{TextInput, TextInputEvent};
use crate::ui::pages::selfbuild_section;
use crate::ui::theme::{self, ThemePref};

#[derive(Clone, Copy, PartialEq, Eq)]
enum SettingsTab {
    General,
    Sessions,
    Projects,
    Terminal,
    Speech,
    Data,
}

impl SettingsTab {
    const ALL: [Self; 6] = [
        Self::General,
        Self::Sessions,
        Self::Projects,
        Self::Terminal,
        Self::Speech,
        Self::Data,
    ];

    fn label_key(self) -> &'static str {
        match self {
            Self::General => "settings.tab.general",
            Self::Sessions => "settings.tab.sessions",
            Self::Projects => "settings.tab.projects",
            Self::Terminal => "settings.tab.terminal",
            Self::Speech => "settings.tab.speech",
            Self::Data => "settings.tab.data",
        }
    }
}

pub struct SettingsPage {
    scrollbar: Scrollbar,
    stores: Stores,
    claude_input: Entity<TextInput>,
    doc_prompt_input: Entity<TextArea>,
    issue_prompt_input: Entity<TextArea>,
    tools_input: Entity<TextArea>,
    tab: SettingsTab,
    mic_open: bool,
    lang_open: bool,
    mic_devices: Vec<String>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<Navigate> for SettingsPage {}
impl EventEmitter<GoBack> for SettingsPage {}

const CONTROL_W: f32 = 280.0;
const TOGGLE_W: f32 = 190.0;
const FIELD_H: f32 = 32.0;
const AREA_MAX_ROWS: usize = 8;

fn area_height(input: &Entity<TextArea>, cx: &gpui::App) -> f32 {
    let rows = input.read(cx).rows().min(AREA_MAX_ROWS);
    TextArea::height_for_rows(rows) + 2.0
}

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
        .child(
            div()
                .text_size(px(13.0))
                .overflow_hidden()
                .text_ellipsis()
                .child(label.into()),
        )
        .when_some(hint, |d, hint| {
            d.child(
                div()
                    .text_size(px(12.0))
                    .text_color(theme::text_muted())
                    .child(hint),
            )
        })
}

fn ms_stepper(
    id: &'static str,
    ms: u32,
    (min, max, step): (u32, u32, u32),
    stores: &Stores,
    field: fn(&mut Prefs) -> &mut u32,
) -> Div {
    let (down, up) = (stores.clone(), stores.clone());
    div()
        .flex()
        .items_center()
        .gap(px(10.0))
        .child(small_button(
            format!("{id}-down"),
            "−",
            move |_, _, cx| {
                down.prefs.update(cx, |p, cx| {
                    p.update(cx, |p| {
                        let v = field(p);
                        *v = v.saturating_sub(step).max(min);
                    })
                });
            },
        ))
        .child(
            div()
                .w(px(64.0))
                .flex()
                .justify_center()
                .text_size(px(13.0))
                .child(format!("{ms} ms")),
        )
        .child(small_button(format!("{id}-up"), "+", move |_, _, cx| {
            up.prefs.update(cx, |p, cx| {
                p.update(cx, |p| {
                    let v = field(p);
                    *v = (*v + step).min(max);
                })
            });
        }))
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
    input: impl IntoElement,
    height: f32,
    toggle: Option<AnyElement>,
) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(
            div()
                .flex()
                .items_start()
                .justify_between()
                .gap(px(16.0))
                .child(label_block(label, hint))
                .children(toggle.map(|t| div().flex_none().w(px(TOGGLE_W)).child(t))),
        )
        .child(
            div()
                .flex()
                .h(px(height))
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

fn prompt_area(
    placeholder: &str,
    initial: &str,
    cx: &mut Context<SettingsPage>,
) -> Entity<TextArea> {
    cx.new(|cx| {
        TextArea::new(cx)
            .with_placeholder(placeholder.to_string())
            .with_value(initial)
            .with_line_numbers()
    })
}

impl SettingsPage {
    pub fn new(stores: Stores, cx: &mut Context<Self>) -> Self {
        let initial = stores.prefs.read(cx).prefs.claude_command.clone();
        let claude_input = cx.new(|cx| TextInput::new("claude", cx).with_value(initial));
        let initial = stores.prefs.read(cx).prefs.doc_prompt.clone();
        let doc_prompt_input = prompt_area(DEFAULT_DOC_PROMPT, &initial, cx);
        let initial = stores.prefs.read(cx).prefs.issue_prompt.clone();
        let issue_prompt_input = prompt_area(DEFAULT_ISSUE_PROMPT, &initial, cx);
        let initial = stores.prefs.read(cx).prefs.workflow_allowed_tools.clone();
        let tools_input = prompt_area(DEFAULT_WORKFLOW_TOOLS, &initial, cx);
        let subs = vec![
            cx.observe(&stores.prefs, |_, _, cx| cx.notify()),
            cx.observe(&doc_prompt_input, |_, _, cx| cx.notify()),
            cx.observe(&issue_prompt_input, |_, _, cx| cx.notify()),
            cx.observe(&tools_input, |_, _, cx| cx.notify()),
            cx.observe(&stores.projects, |_, _, cx| cx.notify()),
            cx.observe(&stores.selfbuild, |_, _, cx| cx.notify()),
            cx.observe(&stores.speech, |_, _, cx| cx.notify()),
            cx.subscribe(&claude_input, |this, input, ev: &TextInputEvent, cx| {
                if matches!(ev, TextInputEvent::Changed) {
                    let value = input.read(cx).value().to_string();
                    this.stores
                        .prefs
                        .update(cx, |p, cx| p.update(cx, |p| p.claude_command = value));
                }
            }),
            cx.subscribe(&tools_input, |this, input, ev: &TextAreaEvent, cx| {
                if matches!(ev, TextAreaEvent::Changed) {
                    let value = input.read(cx).value().to_string();
                    this.stores.prefs.update(cx, |p, cx| {
                        p.update(cx, |p| p.workflow_allowed_tools = value)
                    });
                }
            }),
            cx.subscribe(&doc_prompt_input, |this, input, ev: &TextAreaEvent, cx| {
                if matches!(ev, TextAreaEvent::Changed) {
                    let value = input.read(cx).value().to_string();
                    this.stores
                        .prefs
                        .update(cx, |p, cx| p.update(cx, |p| p.doc_prompt = value));
                }
            }),
            cx.subscribe(
                &issue_prompt_input,
                |this, input, ev: &TextAreaEvent, cx| {
                    if matches!(ev, TextAreaEvent::Changed) {
                        let value = input.read(cx).value().to_string();
                        this.stores
                            .prefs
                            .update(cx, |p, cx| p.update(cx, |p| p.issue_prompt = value));
                    }
                },
            ),
        ];
        selfbuild_section::recheck(&stores, cx);
        Self {
            scrollbar: Scrollbar::new(),
            stores,
            claude_input,
            doc_prompt_input,
            issue_prompt_input,
            tools_input,
            tab: SettingsTab::General,
            mic_open: false,
            lang_open: false,
            mic_devices: Vec::new(),
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
                FIELD_H,
                None,
            ),
        ])
        .into_any_element()
    }

    fn sessions_tab(&self, prefs: &crate::state::prefs::Prefs, cx: &gpui::App) -> AnyElement {
        let stores = self.stores.clone();
        let set = move |change: fn(&mut crate::state::prefs::Prefs, bool)| {
            let stores = stores.clone();
            move |value: bool, _: &mut Window, cx: &mut gpui::App| {
                stores
                    .prefs
                    .update(cx, |p, cx| p.update(cx, |p| change(p, value)));
            }
        };

        let view_stores = self.stores.clone();
        let default_view = prefs.default_view;
        let col = div().flex().flex_col().gap(px(16.0)).child(list(vec![
            setting_row(
                t("settings.sessions.view_title"),
                Some(t("settings.sessions.view_hint").into()),
                segmented(
                    "pref-view",
                    SessionView::ALL
                        .iter()
                        .map(|v| t(v.label_key()).into())
                        .collect(),
                    SessionView::ALL
                        .iter()
                        .position(|v| *v == default_view)
                        .unwrap_or(0),
                    move |i, _, cx| {
                        view_stores.prefs.update(cx, |p, cx| {
                            p.update(cx, |p| p.default_view = SessionView::ALL[i])
                        });
                    },
                )
                .w_full(),
            ),
            checkbox(
                "pref-worktree",
                t("settings.sessions.worktree"),
                prefs.ask_worktree,
                set(|p, v| p.ask_worktree = v),
            )
            .into_any_element(),
            wide_row(
                t("settings.sessions.doc_prompt_title"),
                Some(t("settings.sessions.doc_prompt").into()),
                self.doc_prompt_input.clone(),
                area_height(&self.doc_prompt_input, cx),
                Some(
                    checkbox(
                        "pref-send-doc",
                        t("settings.sessions.send_doc"),
                        prefs.send_doc_prompt,
                        set(|p, v| p.send_doc_prompt = v),
                    )
                    .into_any_element(),
                ),
            ),
            wide_row(
                t("settings.sessions.issue_prompt_title"),
                Some(t("settings.sessions.issue_prompt").into()),
                self.issue_prompt_input.clone(),
                area_height(&self.issue_prompt_input, cx),
                Some(
                    checkbox(
                        "pref-send-issue",
                        t("settings.sessions.send_issue"),
                        prefs.send_issue_prompt,
                        set(|p, v| p.send_issue_prompt = v),
                    )
                    .into_any_element(),
                ),
            ),
            wide_row(
                t("settings.sessions.tools_title"),
                Some(t("settings.sessions.tools").into()),
                self.tools_input.clone(),
                area_height(&self.tools_input, cx),
                None,
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

        col.into_any_element()
    }

    fn projects_tab(&self, cx: &Context<Self>) -> AnyElement {
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

        let col = div()
            .flex()
            .flex_col()
            .gap(px(16.0))
            .child(group_title(t("settings.projects.title")))
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(theme::text_muted())
                    .child(t("settings.projects.hint")),
            );
        let body = if git_projects.is_empty() {
            div()
                .text_size(px(13.0))
                .text_color(theme::text_muted())
                .child(t("settings.projects.empty"))
                .into_any_element()
        } else {
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
            list(project_rows).into_any_element()
        };
        col.child(body).into_any_element()
    }

    fn terminal_tab(&self, font: f32) -> AnyElement {
        let down = self.stores.clone();
        let up = self.stores.clone();
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
        )])
        .into_any_element()
    }

    fn speech_tab(&self, prefs: &crate::state::prefs::Prefs, cx: &Context<Self>) -> AnyElement {
        let speech = self.stores.speech.read(cx);
        let installed = speech.installed;
        let downloading = speech.download;
        let error = speech.error.clone();

        let enable_stores = self.stores.clone();
        let lang_stores = self.stores.clone();
        let mic_stores = self.stores.clone();
        let mic_page = cx.entity();
        let toggle_page = mic_page.clone();
        let lang_page = mic_page.clone();
        let toggle_lang_page = mic_page.clone();
        let silence = prefs.vad_silence_ms;
        let auto_enter = prefs.speech_auto_enter;
        let auto_enter_ms = prefs.speech_auto_enter_ms;
        let auto_toggle = self.stores.clone();
        let language = prefs.speech_language;

        let status = match downloading {
            Some(bytes) => {
                let total = crate::speech::model::total_size().max(1);
                t_args(
                    "speech.downloading",
                    &[("percent", &(bytes * 100 / total).to_string())],
                )
            }
            None if installed => t("settings.speech.model_installed"),
            None => t("settings.speech.model_missing"),
        };
        let model_stores = self.stores.clone();
        let model_control = div()
            .flex()
            .items_center()
            .gap(px(10.0))
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(theme::text_muted())
                    .child(status),
            )
            .when(!installed && downloading.is_none(), |d| {
                d.child(button(
                    "speech-download",
                    t("settings.speech.download"),
                    ButtonVariant::Classic,
                    move |_, _, cx| {
                        model_stores.speech.update(cx, |s, cx| s.download_model(cx));
                    },
                ))
            });

        let mut devices: Vec<SharedString> = vec![t("settings.speech.mic_default").into()];
        devices.extend(self.mic_devices.iter().cloned().map(SharedString::from));
        let selected = self
            .mic_devices
            .iter()
            .position(|d| *d == prefs.mic_device)
            .map_or(0, |i| i + 1);
        let device_names = self.mic_devices.clone();

        let mut rows = vec![
            checkbox(
                "pref-speech",
                t("settings.speech.enable"),
                prefs.speech_enabled,
                move |value, _, cx| {
                    enable_stores
                        .prefs
                        .update(cx, |p, cx| p.update(cx, |p| p.speech_enabled = value));
                },
            )
            .into_any_element(),
        ];
        if !crate::speech::AVAILABLE {
            rows.push(
                div()
                    .text_size(px(12.0))
                    .text_color(theme::warning())
                    .child(t("speech.unavailable"))
                    .into_any_element(),
            );
        }
        rows.extend([
            setting_row(
                t("settings.speech.model"),
                Some(t("settings.speech.model_hint").into()),
                model_control,
            ),
            setting_row(
                t("settings.speech.language"),
                Some(t("settings.speech.language_hint").into()),
                dropdown(
                    "pref-speech-language",
                    SpeechLanguage::ALL
                        .iter()
                        .map(|v| t(v.label_key()).into())
                        .collect(),
                    SpeechLanguage::ALL
                        .iter()
                        .position(|v| *v == language)
                        .unwrap_or(0),
                    self.lang_open,
                    move |open, _, cx| {
                        toggle_lang_page.update(cx, |page, cx| {
                            page.lang_open = open;
                            cx.notify();
                        });
                    },
                    move |i, _, cx| {
                        lang_stores.prefs.update(cx, |p, cx| {
                            p.update(cx, |p| p.speech_language = SpeechLanguage::ALL[i])
                        });
                        lang_page.update(cx, |page, cx| {
                            page.lang_open = false;
                            cx.notify();
                        });
                    },
                )
                .relative()
                .w_full(),
            ),
            setting_row(
                t("settings.speech.mic"),
                None,
                dropdown(
                    "pref-mic",
                    devices,
                    selected,
                    self.mic_open,
                    move |open, _, cx| {
                        toggle_page.update(cx, |page, cx| {
                            if open {
                                page.mic_devices = crate::speech::input_devices();
                            }
                            page.mic_open = open;
                            cx.notify();
                        });
                    },
                    move |i, _, cx| {
                        let name = i
                            .checked_sub(1)
                            .and_then(|i| device_names.get(i).cloned())
                            .unwrap_or_default();
                        mic_stores
                            .prefs
                            .update(cx, |p, cx| p.update(cx, |p| p.mic_device = name));
                        mic_page.update(cx, |page, cx| {
                            page.mic_open = false;
                            cx.notify();
                        });
                    },
                )
                .relative()
                .w_full(),
            ),
            setting_row(
                t("settings.speech.silence"),
                Some(t("settings.speech.silence_hint").into()),
                ms_stepper(
                    "silence",
                    silence,
                    (MIN_VAD_SILENCE_MS, MAX_VAD_SILENCE_MS, VAD_SILENCE_STEP_MS),
                    &self.stores,
                    |p| &mut p.vad_silence_ms,
                ),
            ),
            checkbox(
                "pref-speech-auto-enter",
                t("settings.speech.auto_enter"),
                auto_enter,
                move |value, _, cx| {
                    auto_toggle
                        .prefs
                        .update(cx, |p, cx| p.update(cx, |p| p.speech_auto_enter = value));
                },
            )
            .into_any_element(),
        ]);
        if auto_enter {
            rows.push(setting_row(
                t("settings.speech.auto_enter_delay"),
                Some(t("settings.speech.auto_enter_hint").into()),
                ms_stepper(
                    "auto-enter",
                    auto_enter_ms,
                    (MIN_AUTO_ENTER_MS, MAX_AUTO_ENTER_MS, AUTO_ENTER_STEP_MS),
                    &self.stores,
                    |p| &mut p.speech_auto_enter_ms,
                ),
            ));
        }
        let mut col = div().flex().flex_col().gap(px(12.0)).child(list(rows));
        if let Some(error) = error {
            col = col.child(
                div()
                    .text_size(px(12.0))
                    .text_color(theme::error())
                    .child(error),
            );
        }
        col.child(
            div()
                .text_size(px(12.0))
                .text_color(theme::text_muted())
                .child(t("settings.speech.hotkey")),
        )
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
            SettingsTab::Projects => self.projects_tab(cx),
            SettingsTab::Terminal => self.terminal_tab(prefs.terminal_font_size),
            SettingsTab::Speech => self.speech_tab(&prefs, cx),
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
