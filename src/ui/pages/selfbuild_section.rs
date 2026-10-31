//! The self-build block of the settings page: toolchain checklist, install buttons, the enable
//! button, progress and the build cache.

use gpui::{AnyElement, App, IntoElement, ParentElement, Styled, div, prelude::*, px};

use crate::i18n::{t, t_args};
use crate::selfbuild::cache::format_bytes;
use crate::selfbuild::clone::version_tag;
use crate::selfbuild::preflight::{State, Tool};
use crate::state::Stores;
use crate::state::sessions::begin_upstream_update;
use crate::state::selfbuild::{Phase, UpstreamState, Work};
use crate::ui::components::button::{ButtonVariant, button, small_button};
use crate::ui::confirm::{self, ConfirmOptions};
use crate::ui::theme;

fn status_text(phase: &Phase) -> (String, gpui::Rgba) {
    match phase {
        Phase::Idle => (t("selfbuild.status.idle"), theme::text_muted()),
        Phase::Ready => (t("selfbuild.status.ready"), theme::success()),
        Phase::Failed(e) => (t_args("selfbuild.status.failed", &[("error", e)]), theme::error()),
        Phase::Working(w) => {
            let text = match w {
                Work::Checking => t("selfbuild.status.checking"),
                Work::Installing(tool) => {
                    t_args("selfbuild.status.installing", &[("tool", &t(tool.label_key()))])
                }
                Work::Cloning => t("selfbuild.status.cloning"),
                Work::Building => t("selfbuild.status.building"),
                Work::Cleaning => t("selfbuild.status.cleaning"),
            };
            (text, theme::text())
        }
    }
}

/// Re-detect the toolchain, using the configured agent command.
pub fn recheck(stores: &Stores, cx: &mut App) {
    let program = stores.prefs.read(cx).prefs.program().to_string();
    stores.selfbuild.update(cx, |s, cx| s.refresh(&program, cx));
}

fn ask_install(stores: &Stores, tool: Tool, cx: &mut App) {
    let options = ConfirmOptions::new(
        t_args("selfbuild.install.title", &[("tool", &t(tool.label_key()))]),
        t_args("selfbuild.install.message", &[("command", &tool.install_command())]),
    )
    .labels(t("selfbuild.install"), t("common.cancel"));
    let store = stores.selfbuild.clone();
    confirm::ask(cx, options, move |answer, _, cx| {
        if answer == Some(true) {
            store.update(cx, |s, cx| s.install(tool, cx));
        }
    });
}

fn ask_enable(stores: &Stores, cx: &mut App) {
    let dir = stores
        .selfbuild
        .read(cx)
        .paths()
        .map(|p| p.clone_dir().display().to_string())
        .unwrap_or_default();
    let options = ConfirmOptions::new(
        t("selfbuild.enable.title"),
        t_args("selfbuild.enable.message", &[("tag", &version_tag()), ("dir", &dir)]),
    )
    .labels(t("selfbuild.enable.confirm"), t("common.cancel"));
    let store = stores.selfbuild.clone();
    confirm::ask(cx, options, move |answer, _, cx| {
        if answer == Some(true) {
            store.update(cx, |s, cx| s.enable(cx));
        }
    });
}

fn ask_clean(stores: &Stores, cx: &mut App) {
    let dir = stores
        .selfbuild
        .read(cx)
        .paths()
        .map(|p| p.target_dir().display().to_string())
        .unwrap_or_default();
    let options = ConfirmOptions::new(
        t("selfbuild.cache.clean.title"),
        t_args("selfbuild.cache.clean.message", &[("dir", &dir)]),
    )
    .labels(t("selfbuild.cache.clean.confirm"), t("common.cancel"));
    let store = stores.selfbuild.clone();
    confirm::ask(cx, options, move |answer, _, cx| {
        if answer == Some(true) {
            store.update(cx, |s, cx| s.clean_cache(cx));
        }
    });
}

fn ask_update(tag: String, cx: &mut App) {
    let options = ConfirmOptions::new(
        t_args("selfbuild.upstream.confirm.title", &[("tag", &tag)]),
        format!(
            "{}

{}",
            t_args("selfbuild.upstream.confirm.message", &[("tag", &tag)]),
            t("selfbuild.upstream.warn")
        ),
    )
    .labels(t("selfbuild.upstream.confirm.ok"), t("common.cancel"));
    confirm::ask(cx, options, move |answer, _, cx| {
        if answer == Some(true) {
            begin_upstream_update(tag.clone(), cx);
        }
    });
}

/// Base release, local changes and the "update available" action. Only when self-build is ready.
fn upstream_block(stores: &Stores, cx: &App) -> AnyElement {
    let store = stores.selfbuild.read(cx);
    let busy = store.busy();
    let (line, color) = match &store.upstream {
        UpstreamState::Unknown => (String::new(), theme::text_muted()),
        UpstreamState::Checking => (t("selfbuild.upstream.checking"), theme::text_muted()),
        UpstreamState::Failed(e) => (t_args("selfbuild.upstream.error", &[("error", e)]), theme::warning()),
        UpstreamState::Checked(s) => match &s.available {
            Some(tag) => (t_args("selfbuild.upstream.available", &[("tag", tag)]), theme::warning()),
            None => (t("selfbuild.upstream.current"), theme::success()),
        },
    };
    let base = match &store.upstream {
        UpstreamState::Checked(s) => Some(t_args(
            "selfbuild.upstream.base",
            &[("base", &s.base), ("count", &s.local_changes.to_string())],
        )),
        _ => None,
    };
    let available = store.update_available().map(str::to_string);
    let checking = store.upstream == UpstreamState::Checking;
    let check_stores = stores.clone();
    div()
        .flex()
        .flex_col()
        .gap(px(6.0))
        .when_some(base, |s, b| s.child(div().text_size(px(12.5)).child(b)))
        .when(!line.is_empty(), |s| s.child(div().text_size(px(12.5)).text_color(color).child(line)))
        .child(
            div()
                .flex()
                .gap(px(8.0))
                .when_some(available, |row, tag| {
                    row.child(button(
                        "selfbuild-upstream-update",
                        t("selfbuild.upstream.update"),
                        ButtonVariant::Classic,
                        move |_, _, cx| ask_update(tag.clone(), cx),
                    ))
                })
                .when(!checking && !busy, |row| {
                    row.child(small_button(
                        "selfbuild-upstream-check",
                        t("selfbuild.upstream.check"),
                        move |_, _, cx| check_stores.selfbuild.update(cx, |s, cx| s.check_upstream(cx)),
                    ))
                }),
        )
        .into_any_element()
}

fn tool_row(stores: &Stores, tool: Tool, state: Option<&State>, installable: bool) -> AnyElement {
    let (detail, color) = match state {
        Some(State::Found(d)) => (d.clone(), theme::success()),
        Some(State::Missing) => (t("selfbuild.missing"), theme::warning()),
        None => (String::new(), theme::text_muted()),
    };
    let missing = matches!(state, Some(State::Missing));
    let stores = stores.clone();
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(10.0))
        .child(
            div()
                .flex()
                .flex_col()
                .min_w_0()
                .child(div().text_size(px(13.0)).child(t(tool.label_key())))
                .child(
                    div()
                        .truncate()
                        .text_size(px(11.5))
                        .text_color(color)
                        .child(detail),
                ),
        )
        .when(missing && installable, |row| {
            row.child(small_button(
                gpui::SharedString::from(format!("selfbuild-install-{tool:?}")),
                t("selfbuild.install"),
                move |_, _, cx| ask_install(&stores, tool, cx),
            ))
        })
        .into_any_element()
}

/// The section's body (the caller wraps it in the settings `section`).
pub fn body(stores: &Stores, cx: &App) -> AnyElement {
    let store = stores.selfbuild.read(cx);
    let busy = store.busy();
    let can_enable = store.can_enable();
    let ready = store.phase == Phase::Ready;
    let installable = store.winget && !busy;
    let (status, status_color) = status_text(&store.phase);
    let cache = match store.cache_bytes {
        Some(n) => t_args("selfbuild.cache", &[("size", &format_bytes(n))]),
        None => t("selfbuild.cache.measuring"),
    };
    let has_cache = store.cache_bytes.is_some_and(|n| n > 0);
    let missing_any = store.checklist.items.iter().any(|i| !i.state.found());
    let no_winget = !store.winget && missing_any;
    let tail: Vec<String> = store.tail.iter().cloned().collect();
    let rows: Vec<AnyElement> = Tool::ALL
        .iter()
        .map(|&tool| tool_row(stores, tool, store.checklist.state(tool), installable))
        .collect();

    let (recheck_stores, enable_stores, clean_stores) = (stores.clone(), stores.clone(), stores.clone());
    div()
        .flex()
        .flex_col()
        .gap(px(10.0))
        .child(
            div()
                .text_size(px(12.5))
                .text_color(theme::text_muted())
                .child(t("selfbuild.hint")),
        )
        .children(rows)
        .when(no_winget, |s| {
            s.child(
                div()
                    .text_size(px(12.0))
                    .text_color(theme::warning())
                    .child(t("selfbuild.no_winget")),
            )
        })
        .child(div().text_size(px(12.5)).text_color(status_color).child(status))
        .when(ready, |s| s.child(upstream_block(stores, cx)))
        .when(!tail.is_empty(), |s| {
            s.child(
                div()
                    .flex()
                    .flex_col()
                    .p(px(8.0))
                    .rounded(px(6.0))
                    .bg(theme::bg_app())
                    .font_family(theme::FONT)
                    .text_size(px(11.0))
                    .text_color(theme::text_muted())
                    .children(tail.into_iter().map(|l| div().truncate().child(l))),
            )
        })
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap(px(8.0))
                .when(!ready, |s| {
                    s.child(
                        button(
                            "selfbuild-enable",
                            t("selfbuild.enable"),
                            if can_enable { ButtonVariant::Classic } else { ButtonVariant::Secondary },
                            move |_, _, cx| {
                                if enable_stores.selfbuild.read(cx).can_enable() {
                                    ask_enable(&enable_stores, cx);
                                }
                            },
                        )
                        .opacity(if can_enable { 1.0 } else { 0.5 }),
                    )
                })
                .child(button(
                    "selfbuild-recheck",
                    t("selfbuild.recheck"),
                    ButtonVariant::Secondary,
                    move |_, _, cx| recheck(&recheck_stores, cx),
                )),
        )
        .child(
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
                        .child(cache),
                )
                .when(has_cache && !busy, |row| {
                    row.child(small_button(
                        "selfbuild-clean",
                        t("selfbuild.cache.clean"),
                        move |_, _, cx| ask_clean(&clean_stores, cx),
                    ))
                }),
        )
        .into_any_element()
}
