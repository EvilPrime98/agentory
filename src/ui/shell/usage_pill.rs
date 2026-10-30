//! Header pill with the 5-hour plan usage, opening a popover with the plan limits and one row per
//! session (see `state::usage`).

use std::time::{Instant, SystemTime, UNIX_EPOCH};

use gpui::{
    AnyElement, Context, InteractiveElement, IntoElement, ParentElement, Render, Rgba,
    StatefulInteractiveElement, Styled, Subscription, Window, deferred, div, prelude::*, px,
};

use crate::state::Stores;
use crate::state::sessions::SessionKind;
use crate::state::usage::{Limit, SessionUsage};
use crate::ui::theme;

const WARN_PERCENT: f32 = 75.0;
const CRITICAL_PERCENT: f32 = 90.0;
/// A click that closes the popover through "click outside" must not reopen it via the pill.
const REOPEN_GUARD_MS: u128 = 250;

pub struct UsagePill {
    stores: Stores,
    open: bool,
    closed_at: Option<Instant>,
    _subs: Vec<Subscription>,
}

fn bar_color(percent: f32) -> Rgba {
    if percent >= CRITICAL_PERCENT {
        theme::error()
    } else if percent >= WARN_PERCENT {
        theme::warning()
    } else {
        theme::accent()
    }
}

fn bar(percent: f32, width: f32, height: f32, color: Rgba) -> impl IntoElement {
    div()
        .flex_none()
        .w(px(width))
        .h(px(height))
        .rounded(px(height / 2.0))
        .bg(theme::hover())
        .child(
            div()
                .h_full()
                .w(px(width * percent.clamp(0.0, 100.0) / 100.0))
                .rounded(px(height / 2.0))
                .bg(color),
        )
}

fn tokens(n: u64) -> String {
    match n {
        0..1_000 => n.to_string(),
        1_000..1_000_000 => format!("{:.1}k", n as f64 / 1e3),
        _ => format!("{:.1}M", n as f64 / 1e6),
    }
}

fn resets_in(resets_at: u64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let mins = resets_at.saturating_sub(now) / 60;
    match mins {
        0 => "resets now".into(),
        1..60 => format!("resets in {mins}m"),
        60..1440 => format!("resets in {}h {}m", mins / 60, mins % 60),
        _ => format!("resets in {}d {}h", mins / 1440, mins % 1440 / 60),
    }
}

fn limit_row(label: &'static str, limit: Option<Limit>, stale: bool) -> impl IntoElement {
    let (percent, detail) = match limit {
        Some(l) => (l.percent, l.resets_at.map(resets_in).unwrap_or_default()),
        None => (0.0, "no data".into()),
    };
    let color = if stale {
        theme::text_muted()
    } else {
        bar_color(percent)
    };
    div()
        .flex()
        .flex_col()
        .gap(px(4.0))
        .child(
            div()
                .flex()
                .justify_between()
                .child(div().child(label))
                .child(
                    div()
                        .text_color(theme::text_muted())
                        .child(format!("{percent:.0}% · {detail}")),
                ),
        )
        .child(bar(percent, 328.0, 6.0, color))
}

fn session_row(name: String, usage: Option<&SessionUsage>) -> impl IntoElement {
    let row = div().flex().flex_col().gap(px(3.0)).child(
        div()
            .flex()
            .justify_between()
            .gap(px(8.0))
            .child(div().min_w_0().truncate().child(name))
            .child(div().flex_none().text_color(theme::text_muted()).child(
                usage.map_or("—".to_string(), |u| {
                    format!("{:.0}% context", u.context_percent())
                }),
            )),
    );
    let Some(u) = usage else {
        return row;
    };
    row.child(bar(
        u.context_percent(),
        328.0,
        4.0,
        bar_color(u.context_percent()),
    ))
    .child(
        div()
            .text_size(px(12.0))
            .text_color(theme::text_muted())
            .child(format!(
                "in {} · out {} · cache read {} · cache write {}",
                tokens(u.input),
                tokens(u.output),
                tokens(u.cache_read),
                tokens(u.cache_write),
            )),
    )
}

impl UsagePill {
    pub fn new(stores: Stores, cx: &mut Context<Self>) -> Self {
        let subs = vec![
            cx.observe(&stores.usage, |_, _, cx| cx.notify()),
            cx.observe(&stores.sessions, |_, _, cx| cx.notify()),
        ];
        Self {
            stores,
            open: false,
            closed_at: None,
            _subs: subs,
        }
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        if self.open {
            self.open = false;
            self.closed_at = Some(Instant::now());
            cx.notify();
        }
    }

    fn toggle(&mut self, cx: &mut Context<Self>) {
        if self.open {
            self.close(cx);
        } else if self
            .closed_at
            .is_none_or(|t| t.elapsed().as_millis() > REOPEN_GUARD_MS)
        {
            self.open = true;
            self.stores
                .usage
                .update(cx, |u, cx| u.refresh_limits(false, cx));
            cx.notify();
        }
    }

    fn popover(&self, cx: &mut Context<Self>) -> AnyElement {
        let usage = self.stores.usage.read(cx);
        let stale = usage.unavailable;
        let plan = usage.plan.unwrap_or_default();
        let rows: Vec<AnyElement> = self
            .stores
            .sessions
            .read(cx)
            .items
            .iter()
            .filter(|s| s.kind == SessionKind::Agent)
            .map(|s| session_row(s.name.clone(), usage.sessions.get(&s.id)).into_any_element())
            .collect();

        deferred(
            div()
                .id("usage-popover")
                .occlude()
                .absolute()
                .top(px(38.0))
                .right_0()
                .w(px(360.0))
                .p(px(16.0))
                .flex()
                .flex_col()
                .gap(px(12.0))
                .rounded(px(10.0))
                .border_1()
                .border_color(theme::border_subtle())
                .bg(theme::bg_modal())
                .text_size(px(13.0))
                .text_color(theme::text())
                .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close(cx)))
                .when(stale, |s| {
                    s.child(
                        div()
                            .text_color(theme::warning())
                            .child("Usage unavailable — showing last known values"),
                    )
                })
                .child(limit_row("5-hour limit", plan.five_hour, stale))
                .child(limit_row("Weekly limit", plan.weekly, stale))
                .child(div().h(px(1.0)).bg(theme::border_subtle()))
                .child(
                    div()
                        .id("usage-sessions")
                        .flex()
                        .flex_col()
                        .gap(px(10.0))
                        .max_h(px(300.0))
                        .overflow_y_scroll()
                        .when(rows.is_empty(), |s| {
                            s.child(
                                div()
                                    .text_color(theme::text_muted())
                                    .child("No sessions running"),
                            )
                        })
                        .children(rows),
                ),
        )
        .with_priority(1)
        .into_any_element()
    }
}

impl Render for UsagePill {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let usage = self.stores.usage.read(cx);
        let five_hour = usage.plan.and_then(|p| p.five_hour);
        let stale = usage.unavailable || five_hour.is_none();
        let percent = five_hour.map_or(0.0, |l| l.percent);
        let color = if stale {
            theme::text_muted()
        } else {
            bar_color(percent)
        };
        let label = match five_hour {
            Some(_) => format!("{percent:.0}%"),
            None => "—".to_string(),
        };

        div()
            .relative()
            .flex_none()
            .mr(px(4.0))
            .child(
                div()
                    .id("usage-pill")
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .h(px(30.0))
                    .px(px(10.0))
                    .rounded(px(8.0))
                    .cursor_pointer()
                    .text_size(px(13.0))
                    .text_color(if stale {
                        theme::text_muted()
                    } else {
                        theme::text()
                    })
                    .hover(|s| s.bg(theme::hover()))
                    .when(self.open, |s| s.bg(theme::hover()))
                    .on_click(cx.listener(|this, _, _, cx| this.toggle(cx)))
                    .child("5h")
                    .child(bar(percent, 40.0, 4.0, color))
                    .child(label),
            )
            .when(self.open, |s| s.child(self.popover(cx)))
    }
}
