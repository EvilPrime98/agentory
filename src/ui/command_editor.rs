use std::time::Instant;

use gpui::{
    App, AppContext as _, Context, Entity, Focusable, Global, InteractiveElement, IntoElement,
    MouseButton, ParentElement, Render, SharedString, Styled, Subscription, Window, div, px,
};

use crate::i18n::t;
use crate::model::CustomCommand;
use crate::platform::shells::{self, Shell};
use crate::state::Stores;
use crate::ui::components::button::{ButtonVariant, button};
use crate::ui::components::dropdown::dropdown;
use crate::ui::components::text_input::{TextInput, TextInputEvent};
use crate::ui::theme;

struct Editing {
    project_id: String,
    command_id: String,
    shell: Option<String>,
    shells: Vec<Shell>,
    focus_name: bool,
    shell_menu: bool,
    shell_menu_closed_at: Option<Instant>,
}

const REOPEN_GUARD_MS: u128 = 200;

pub struct CommandEditor {
    editing: Option<Editing>,
    name: Entity<TextInput>,
    command: Entity<TextInput>,
    focus_command: bool,
    _subs: Vec<Subscription>,
}

struct EditorGlobal(Entity<CommandEditor>);
impl Global for EditorGlobal {}

pub fn init(cx: &mut App) -> Entity<CommandEditor> {
    let host = cx.new(|cx| {
        let name = cx.new(|cx| TextInput::new(t("commands.name_placeholder"), cx));
        let command = cx.new(|cx| TextInput::new(t("commands.command_placeholder"), cx));
        let subs = vec![
            cx.subscribe(
                &name,
                |this: &mut CommandEditor, _, ev: &TextInputEvent, cx| {
                    match ev {
                        TextInputEvent::Submit => this.focus_command = true,
                        TextInputEvent::Cancel => this.close(cx),
                        TextInputEvent::Changed => {}
                    }
                    cx.notify();
                },
            ),
            cx.subscribe(&command, |this, _, ev: &TextInputEvent, cx| match ev {
                TextInputEvent::Submit => this.save(cx),
                TextInputEvent::Cancel => this.close(cx),
                TextInputEvent::Changed => {}
            }),
        ];
        CommandEditor {
            editing: None,
            name,
            command,
            focus_command: false,
            _subs: subs,
        }
    });
    cx.set_global(EditorGlobal(host.clone()));
    host
}

pub fn open(project_id: &str, existing: Option<&CustomCommand>, cx: &mut App) {
    let Some(host) = cx.try_global::<EditorGlobal>().map(|g| g.0.clone()) else {
        return;
    };
    let (name, command) = existing.map_or((String::new(), String::new()), |c| {
        (c.name.clone(), c.command.clone())
    });
    host.update(cx, |h, cx| {
        h.name.update(cx, |i, cx| i.set_value(name, cx));
        h.command.update(cx, |i, cx| i.set_value(command, cx));
        h.editing = Some(Editing {
            project_id: project_id.to_string(),
            command_id: existing.map(|c| c.id.clone()).unwrap_or_default(),
            shell: existing.and_then(|c| c.shell.clone()),
            shells: shells::detect(),
            focus_name: true,
            shell_menu: false,
            shell_menu_closed_at: None,
        });
        cx.notify();
    });
}

impl CommandEditor {
    fn close(&mut self, cx: &mut Context<Self>) {
        if self.editing.take().is_some() {
            cx.notify();
        }
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let Some(e) = &self.editing else { return };
        let command = self.command.read(cx).value().trim().to_string();
        if command.is_empty() {
            return;
        }
        let name = self.name.read(cx).value().trim().to_string();
        let saved = CustomCommand {
            id: e.command_id.clone(),
            name: if name.is_empty() {
                command.clone()
            } else {
                name
            },
            command,
            shell: e.shell.clone(),
        };
        let project_id = e.project_id.clone();
        self.editing = None;
        cx.notify();
        let projects = cx.global::<Stores>().projects.clone();
        cx.defer(move |cx| projects.update(cx, |p, cx| p.save_command(&project_id, saved, cx)));
    }
}

fn field(label: SharedString, input: Entity<TextInput>) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap(px(6.0))
        .child(
            div()
                .text_size(px(12.0))
                .text_color(theme::text_muted())
                .child(label),
        )
        .child(
            div()
                .flex()
                .h(px(32.0))
                .px(px(8.0))
                .rounded(px(4.0))
                .border_1()
                .border_color(theme::border_subtle())
                .bg(theme::bg_app())
                .child(input),
        )
}

impl Render for CommandEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focus_command = std::mem::take(&mut self.focus_command);
        let Some(e) = &mut self.editing else {
            return div().into_any_element();
        };
        if std::mem::take(&mut e.focus_name) {
            window.focus(&self.name.focus_handle(cx), cx);
        } else if focus_command {
            window.focus(&self.command.focus_handle(cx), cx);
        }
        let mut labels: Vec<SharedString> = vec![t("commands.shell_default").into()];
        labels.extend(e.shells.iter().map(|s| SharedString::from(s.label)));
        let selected = e
            .shell
            .as_deref()
            .and_then(|id| e.shells.iter().position(|s| s.id == id))
            .map_or(0, |i| i + 1);
        let ids: Vec<&'static str> = e.shells.iter().map(|s| s.id).collect();
        let title = if e.command_id.is_empty() {
            t("commands.add")
        } else {
            t("commands.edit")
        };
        let this = cx.entity();
        let this_select = this.clone();
        let menu_open = e.shell_menu;

        div()
            .id("command-editor-overlay")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .p(px(16.0))
            .bg(theme::scrim())
            .occlude()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.close(cx)),
            )
            .child(
                div()
                    .id("command-editor")
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .flex()
                    .flex_col()
                    .gap(px(14.0))
                    .w(px(420.0))
                    .max_w_full()
                    .p(px(20.0))
                    .bg(theme::bg_modal())
                    .border_1()
                    .border_color(theme::border_subtle())
                    .rounded(px(14.0))
                    .child(
                        div()
                            .text_size(px(15.0))
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(theme::text())
                            .child(title),
                    )
                    .child(field(t("commands.name").into(), self.name.clone()))
                    .child(field(t("commands.command").into(), self.command.clone()))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(6.0))
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .text_color(theme::text_muted())
                                    .child(t("commands.shell")),
                            )
                            .child(dropdown(
                                "command-editor-shell",
                                labels,
                                selected,
                                menu_open,
                                move |open, _, cx| {
                                    this.update(cx, |h, cx| {
                                        if let Some(e) = &mut h.editing {
                                            if open
                                                && e.shell_menu_closed_at.is_some_and(|t| {
                                                    t.elapsed().as_millis() <= REOPEN_GUARD_MS
                                                })
                                            {
                                                return;
                                            }
                                            if e.shell_menu && !open {
                                                e.shell_menu_closed_at = Some(Instant::now());
                                            }
                                            e.shell_menu = open;
                                            cx.notify();
                                        }
                                    });
                                },
                                move |i, _, cx| {
                                    let shell = i.checked_sub(1).map(|n| ids[n].to_string());
                                    this_select.update(cx, |h, cx| {
                                        if let Some(e) = &mut h.editing {
                                            e.shell = shell;
                                            e.shell_menu = false;
                                            e.shell_menu_closed_at = Some(Instant::now());
                                            cx.notify();
                                        }
                                    });
                                },
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap(px(8.0))
                            .mt(px(4.0))
                            .child(button(
                                "command-editor-cancel",
                                t("common.cancel"),
                                ButtonVariant::Secondary,
                                cx.listener(|this, _, _, cx| this.close(cx)),
                            ))
                            .child(button(
                                "command-editor-save",
                                t("common.save"),
                                ButtonVariant::Classic,
                                cx.listener(|this, _, _, cx| this.save(cx)),
                            )),
                    ),
            )
            .into_any_element()
    }
}
