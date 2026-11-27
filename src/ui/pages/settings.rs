//! Settings page.

use gpui::{
    AnyElement, Context, Div, Entity, EventEmitter, InteractiveElement, IntoElement, ParentElement,
    Render, SharedString, StatefulInteractiveElement, Styled, Subscription, Window, div,
    prelude::*, px,
};

use crate::i18n::{LanguagePref, t, t_args};
use crate::model::{Folder, normalize_folder};
use crate::platform::logging;
use crate::route::{GoBack, Navigate};
use crate::shortcuts::{self, ShortcutError};
use crate::state::Stores;
use crate::state::prefs::{
    AUTO_ENTER_STEP_MS, DEFAULT_DOC_PROMPT, DEFAULT_ISSUE_PROMPT, DEFAULT_WORKFLOW_TOOLS,
    MAX_AUTO_ENTER_MS, MAX_FONT, MAX_VAD_SILENCE_MS, MAX_WORKERS, MIN_AUTO_ENTER_MS, MIN_FONT,
    MIN_VAD_SILENCE_MS, MIN_WORKERS, SpeechLanguage, VAD_SILENCE_STEP_MS,
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
    Shortcuts,
    Data,
}

impl SettingsTab {
    const ALL: [Self; 7] = [
        Self::General,
        Self::Sessions,
        Self::Projects,
        Self::Terminal,
        Self::Speech,
        Self::Shortcuts,
        Self::Data,
    ];

    fn label_key(self) -> &'static str {
        match self {
            Self::General => "settings.tab.general",
            Self::Sessions => "settings.tab.sessions",
            Self::Projects => "settings.tab.projects",
            Self::Terminal => "settings.tab.terminal",
            Self::Speech => "settings.tab.speech",
            Self::Shortcuts => "settings.tab.shortcuts",
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
    /// The shortcut row waiting for a key combo.
    capture: Option<Capture>,
    shortcut_error: Option<String>,
    scope: Option<String>,
    project_search: Entity<TextInput>,
    project_list_scrollbar: Scrollbar,
    project_detail_scrollbar: Scrollbar,
    folder_fields: Vec<FolderField>,
    _subs: Vec<Subscription>,
}

struct FolderField {
    folder: Folder,
    input: Entity<TextInput>,
    _sub: Subscription,
}

fn folder_label_key(folder: Folder) -> &'static str {
    match folder {
        Folder::Docs => "settings.folders.docs",
        Folder::Issues => "settings.folders.issues",
    }
}

struct Capture {
    key: &'static str,
    _intercept: Subscription,
}

fn pretty_combo(combo: &str) -> String {
    combo
        .split('-')
        .map(|part| {
            let mut c = part.chars();
            c.next().map_or(String::new(), |f| {
                f.to_uppercase().chain(c).collect::<String>()
            })
        })
        .collect::<Vec<_>>()
        .join(" + ")
}

fn shortcut_error_text(e: &ShortcutError) -> String {
    match e {
        ShortcutError::Invalid(c) => t_args("settings.shortcuts.err_invalid", &[("combo", c)]),
        ShortcutError::NoModifier(c) => t_args("settings.shortcuts.err_modifier", &[("combo", c)]),
        ShortcutError::Duplicate(c, other) => {
            let other = shortcuts::action(other).map_or(String::new(), |a| t(a.label_key));
            t_args(
                "settings.shortcuts.err_duplicate",
                &[("combo", c), ("other", &other)],
            )
        }
    }
}

impl EventEmitter<Navigate> for SettingsPage {}
impl EventEmitter<GoBack> for SettingsPage {}

const CONTROL_W: f32 = 280.0;
const TOGGLE_W: f32 = 190.0;
const FIELD_H: f32 = 32.0;
const PROJECT_LIST_W: f32 = 220.0;
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
    unit: &'static str,
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
                .child(format!("{ms}{unit}")),
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
        .child(input_box(height, input))
        .into_any_element()
}

fn input_box(height: f32, input: impl IntoElement) -> Div {
    div()
        .flex()
        .h(px(height))
        .px(px(10.0))
        .rounded(px(6.0))
        .border_1()
        .border_color(theme::border_subtle())
        .bg(theme::bg_app())
        .child(input)
}

fn group_title(title: impl Into<SharedString>) -> gpui::Div {
    div()
        .text_size(px(13.0))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .child(title.into())
}

fn custom_dot() -> Div {
    div()
        .flex_none()
        .size(px(6.0))
        .rounded(px(3.0))
        .bg(theme::accent())
}

fn detail_section(title: String, hint: String, control: impl IntoElement) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(group_title(title))
        .child(
            div()
                .text_size(px(12.0))
                .text_color(theme::text_muted())
                .child(hint),
        )
        .child(control)
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
        let project_search = cx.new(|cx| TextInput::new(t("settings.projects.search"), cx));
        let subs = vec![
            cx.observe(&stores.prefs, |_, _, cx| cx.notify()),
            cx.observe(&doc_prompt_input, |_, _, cx| cx.notify()),
            cx.observe(&issue_prompt_input, |_, _, cx| cx.notify()),
            cx.observe(&tools_input, |_, _, cx| cx.notify()),
            cx.observe(&stores.projects, |this, projects, cx| {
                let removed = this
                    .scope
                    .as_deref()
                    .is_some_and(|id| projects.read(cx).get(id).is_none());
                if removed {
                    this.select_scope(None, cx);
                }
                cx.notify();
            }),
            cx.subscribe(&project_search, |_, input, ev: &TextInputEvent, cx| {
                if matches!(ev, TextInputEvent::Cancel) {
                    input.update(cx, |i, cx| i.set_value("", cx));
                }
                cx.notify();
            }),
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
        let mut this = Self {
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
            capture: None,
            shortcut_error: None,
            scope: None,
            project_search,
            project_list_scrollbar: Scrollbar::new(),
            project_detail_scrollbar: Scrollbar::new(),
            folder_fields: Vec::new(),
            _subs: subs,
        };
        this.select_scope(None, cx);
        this
    }

    fn select_scope(&mut self, scope: Option<String>, cx: &mut Context<Self>) {
        self.folder_fields = Folder::ALL
            .into_iter()
            .map(|folder| {
                let initial = match &scope {
                    None => self.stores.prefs.read(cx).prefs.folder(folder).to_string(),
                    Some(id) => self
                        .stores
                        .projects
                        .read(cx)
                        .get(id)
                        .and_then(|p| p.folder_override(folder))
                        .unwrap_or_default()
                        .to_string(),
                };
                let input = cx.new(|cx| TextInput::new(folder.builtin(), cx).with_value(initial));
                let project = scope.clone();
                let sub = cx.subscribe(&input, move |this, input, ev: &TextInputEvent, cx| {
                    if matches!(ev, TextInputEvent::Changed) {
                        let value = input.read(cx).value().to_string();
                        this.save_folder(project.as_deref(), folder, &value, cx);
                        cx.notify();
                    }
                });
                FolderField {
                    folder,
                    input,
                    _sub: sub,
                }
            })
            .collect();
        self.scope = scope;
        cx.notify();
    }

    fn save_folder(
        &mut self,
        project: Option<&str>,
        folder: Folder,
        raw: &str,
        cx: &mut Context<Self>,
    ) {
        let value = normalize_folder(raw);
        if value.is_none() && !raw.trim().is_empty() {
            return;
        }
        match project {
            None => {
                let value = value.unwrap_or_else(|| folder.builtin().to_string());
                self.stores
                    .prefs
                    .update(cx, |p, cx| p.update(cx, |p| *p.folder_mut(folder) = value));
            }
            Some(id) => self
                .stores
                .projects
                .update(cx, |p, cx| p.set_folder(id, folder, value, cx)),
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
            setting_row(
                t("settings.sessions.max_workers"),
                Some(t("settings.sessions.max_workers_hint").into()),
                ms_stepper(
                    "workers",
                    prefs.max_workers,
                    (MIN_WORKERS, MAX_WORKERS, 1),
                    &self.stores,
                    |p| &mut p.max_workers,
                    "",
                ),
            ),
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

    fn folder_inputs(&self, cx: &mut Context<Self>) -> Div {
        let scope = self.scope.as_deref();
        let columns: Vec<_> = self
            .folder_fields
            .iter()
            .map(|field| {
                let folder = field.folder;
                let input = field.input.clone();
                let app = self.stores.prefs.read(cx).prefs.folder(folder).to_string();
                let placeholder = match scope {
                    None => folder.builtin().to_string(),
                    Some(_) => folder.resolve(None, &app),
                };
                input.update(cx, |i, _| i.set_placeholder(placeholder));
                let value = input.read(cx).value();
                let invalid = !value.trim().is_empty() && normalize_folder(value).is_none();
                let error = invalid.then(|| {
                    let used = match scope {
                        None => folder.resolve(None, &app),
                        Some(id) => self.stores.folder(id, folder, cx),
                    };
                    t_args("settings.folders.invalid", &[("dir", &used)])
                });
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(4.0))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(theme::text_muted())
                            .child(t(folder_label_key(folder))),
                    )
                    .child(input_box(FIELD_H, input))
                    .when_some(error, |d, e| {
                        d.child(
                            div()
                                .text_size(px(12.0))
                                .text_color(theme::warning())
                                .child(e),
                        )
                    })
            })
            .collect();
        div().flex().items_start().gap(px(12.0)).children(columns)
    }

    fn scope_row(
        &self,
        scope: Option<String>,
        label: String,
        custom: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let active = self.scope == scope;
        let id = SharedString::from(format!("scope-{}", scope.as_deref().unwrap_or_default()));
        div()
            .id(id)
            .flex()
            .flex_none()
            .items_center()
            .justify_between()
            .gap(px(8.0))
            .h(px(FIELD_H))
            .px(px(10.0))
            .rounded(px(6.0))
            .text_size(px(13.0))
            .cursor_pointer()
            .map(|d| {
                if active {
                    d.bg(theme::accent_wash())
                } else {
                    d.hover(|s| s.bg(theme::hover()))
                }
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                if this.scope != scope {
                    this.select_scope(scope.clone(), cx);
                }
            }))
            .child(div().min_w_0().truncate().child(label))
            .when(custom, |d| d.child(custom_dot()))
            .into_any_element()
    }

    fn project_list(&self, cx: &mut Context<Self>) -> Div {
        let query = self.project_search.read(cx).value().trim().to_lowercase();
        let projects: Vec<_> = self
            .stores
            .projects
            .read(cx)
            .items
            .iter()
            .filter(|p| !p.is_global())
            .map(|p| {
                let custom =
                    p.use_worktree.is_some() || p.docs_dir.is_some() || p.issues_dir.is_some();
                (p.id.clone(), p.name.clone(), custom)
            })
            .collect();
        let none_added = projects.is_empty();
        let rows: Vec<_> = projects
            .into_iter()
            .filter(|(_, name, _)| name.to_lowercase().contains(&query))
            .map(|(id, name, custom)| self.scope_row(Some(id), name, custom, cx))
            .collect();
        let note = if none_added {
            Some("settings.projects.empty")
        } else if rows.is_empty() {
            Some("settings.projects.no_match")
        } else {
            None
        };
        let defaults = self.scope_row(None, t("settings.projects.defaults"), false, cx);

        div()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(PROJECT_LIST_W))
            .min_h_0()
            .border_r_1()
            .border_color(theme::border_subtle())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_none()
                    .gap(px(8.0))
                    .p(px(8.0))
                    .border_b_1()
                    .border_color(theme::border_subtle())
                    .child(input_box(FIELD_H, self.project_search.clone()))
                    .child(defaults),
            )
            .child(
                self.project_list_scrollbar.overlay(
                    div()
                        .id("settings-project-list")
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .p(px(8.0))
                        .overflow_y_scroll()
                        .children(rows)
                        .when_some(note, |d, key| {
                            d.child(
                                div()
                                    .px(px(10.0))
                                    .text_size(px(12.0))
                                    .text_color(theme::text_muted())
                                    .child(t(key)),
                            )
                        }),
                ),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(6.0))
                    .px(px(18.0))
                    .py(px(8.0))
                    .border_t_1()
                    .border_color(theme::border_subtle())
                    .text_size(px(11.0))
                    .text_color(theme::text_muted())
                    .child(custom_dot())
                    .child(t("settings.projects.custom")),
            )
    }

    fn scope_detail(&self, cx: &mut Context<Self>) -> Div {
        let project = self
            .scope
            .as_deref()
            .and_then(|id| self.stores.projects.read(cx).get(id));
        let title = project.map_or_else(|| t("settings.folders.default"), |p| p.name.clone());
        let path = project.map(|p| p.path.display().to_string());
        let worktree = project
            .filter(|p| p.is_git_repo())
            .map(|p| (p.id.clone(), p.use_worktree));

        let sessions = worktree.map(|(id, choice)| {
            let store = self.stores.projects.clone();
            let selected = match choice {
                None => 0,
                Some(true) => 1,
                Some(false) => 2,
            };
            let seg_id = SharedString::from(format!("wt-{id}"));
            detail_section(
                t("settings.projects.sessions"),
                t("settings.projects.hint"),
                div().flex().child(segmented(
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
                )),
            )
        });
        let folders = detail_section(
            t("settings.folders.title"),
            t("settings.folders.hint"),
            self.folder_inputs(cx),
        );

        let content = div()
            .flex()
            .flex_col()
            .gap(px(20.0))
            .p(px(16.0))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(
                        div()
                            .text_size(px(15.0))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .truncate()
                            .child(title),
                    )
                    .when_some(path, |d, path| {
                        d.child(
                            div()
                                .text_size(px(12.0))
                                .text_color(theme::text_muted())
                                .truncate()
                                .child(path),
                        )
                    }),
            )
            .children(sessions)
            .child(folders);

        div().flex().flex_col().flex_1().min_w_0().min_h_0().child(
            self.project_detail_scrollbar.overlay(
                div()
                    .id("settings-project-detail")
                    .overflow_y_scroll()
                    .child(content),
            ),
        )
    }

    fn projects_tab(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .flex_1()
            .min_h_0()
            .rounded(px(10.0))
            .bg(theme::bg_panel())
            .border_1()
            .border_color(theme::border_subtle())
            .child(self.project_list(cx))
            .child(self.scope_detail(cx))
            .into_any_element()
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

    fn start_capture(&mut self, key: &'static str, cx: &mut Context<Self>) {
        let page = cx.entity().downgrade();
        let intercept = cx.intercept_keystrokes(move |ev, _, cx| {
            cx.stop_propagation();
            let keystroke = ev.keystroke.clone();
            page.update(cx, |this, cx| this.capture_key(&keystroke, cx))
                .ok();
        });
        self.capture = Some(Capture {
            key,
            _intercept: intercept,
        });
        self.shortcut_error = None;
        cx.notify();
    }

    fn capture_key(&mut self, ks: &gpui::Keystroke, cx: &mut Context<Self>) {
        let Some(key) = self.capture.as_ref().map(|c| c.key) else {
            return;
        };
        if ks.key == "escape" && ks.modifiers == gpui::Modifiers::default() {
            self.capture = None;
            self.shortcut_error = None;
        } else if self.apply_shortcut(key, &ks.unparse(), cx) {
            self.capture = None;
        }
        cx.notify();
    }

    /// Stores `combo` for the action if it passes the shared validator, else shows why not.
    fn apply_shortcut(&mut self, key: &'static str, combo: &str, cx: &mut Context<Self>) -> bool {
        let prefs = self.stores.prefs.read(cx).prefs.clone();
        match shortcuts::validate(key, combo, |a| prefs.effective_shortcut(a)) {
            Ok(combo) => {
                self.shortcut_error = None;
                self.stores
                    .prefs
                    .update(cx, |p, cx| p.update(cx, |p| p.set_shortcut(key, combo)));
                true
            }
            Err(e) => {
                self.shortcut_error = Some(shortcut_error_text(&e));
                false
            }
        }
    }

    fn shortcuts_tab(&self, prefs: &Prefs, cx: &Context<Self>) -> AnyElement {
        let capturing = self.capture.as_ref().map(|c| c.key);
        let rows = shortcuts::ACTIONS
            .iter()
            .map(|a| {
                let key = a.key;
                let default = a.default;
                let combo = prefs.effective_shortcut(a);
                let waiting = capturing == Some(key);
                let changed = combo != default;
                let field = div()
                    .id(SharedString::from(format!("shortcut-{key}")))
                    .flex()
                    .items_center()
                    .justify_center()
                    .h(px(FIELD_H))
                    .min_w(px(150.0))
                    .px(px(12.0))
                    .rounded(px(6.0))
                    .border_1()
                    .border_color(if waiting {
                        theme::accent()
                    } else {
                        theme::border_subtle()
                    })
                    .text_size(px(12.0))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| this.start_capture(key, cx)))
                    .child(if waiting {
                        t("settings.shortcuts.press")
                    } else {
                        pretty_combo(&combo)
                    });
                let control =
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .child(field)
                        .when(changed, |d| {
                            d.child(small_button(
                                SharedString::from(format!("shortcut-reset-{key}")),
                                t("settings.shortcuts.reset"),
                                cx.listener(move |this, _, _, cx| {
                                    this.capture = None;
                                    this.apply_shortcut(key, default, cx);
                                    cx.notify();
                                }),
                            ))
                        });
                setting_row(t(a.label_key), None, control)
            })
            .collect();
        let reset_all = cx.listener(|this, _, _, cx| {
            this.capture = None;
            this.shortcut_error = None;
            this.stores.prefs.update(cx, |p, cx| {
                p.update(cx, |p| {
                    for a in &shortcuts::ACTIONS {
                        p.set_shortcut(a.key, a.default.into());
                    }
                })
            });
            cx.notify();
        });
        div()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(theme::text_muted())
                    .child(t("settings.shortcuts.hint")),
            )
            .child(list(rows))
            .when_some(self.shortcut_error.clone(), |d, e| {
                d.child(
                    div()
                        .text_size(px(12.0))
                        .text_color(theme::warning())
                        .child(e),
                )
            })
            .child(div().flex().child(button(
                "shortcuts-reset-all",
                t("settings.shortcuts.reset_all"),
                ButtonVariant::Secondary,
                reset_all,
            )))
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
                    " ms",
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
                    " ms",
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
                        this.capture = None;
                        this.shortcut_error = None;
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
            SettingsTab::Shortcuts => self.shortcuts_tab(&prefs, cx),
            SettingsTab::Data => self.data_tab(cx),
        };

        let column = div()
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
            .child(content);

        let page = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .items_center();

        if tab == SettingsTab::Projects {
            return page.child(column.flex_1().min_h_0()).into_any_element();
        }

        self.scrollbar
            .overlay(page.id("settings-scroll").overflow_y_scroll().child(column))
            .into_any_element()
    }
}
