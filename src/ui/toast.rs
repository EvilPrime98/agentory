//! Toasts: top-center HUD cards, 2.5 s (errors 5 s), slide/fade in and out, countdown bar.

use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    Animation, AnimationExt as _, App, AppContext as _, Context, Entity, Global,
    InteractiveElement, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Window, div, px, relative,
};

use crate::ui::theme;

const ENTER: Duration = Duration::from_millis(200);
const EXIT: Duration = Duration::from_millis(150);
const TICK: Duration = Duration::from_millis(33);
const MAX_VISIBLE: usize = 4;
const SLIDE: f32 = 16.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Success,
    Error,
}

impl Kind {
    fn ttl(self) -> Duration {
        Duration::from_millis(if self == Kind::Error { 5000 } else { 2500 })
    }
}

type OnClick = Rc<dyn Fn(&mut Window, &mut App)>;

struct Toast {
    id: u64,
    kind: Kind,
    text: SharedString,
    born: Instant,
    ttl: Duration,
    remaining: Duration,
    hovered: bool,
    exit_started: Option<Instant>,
    on_click: Option<OnClick>,
}

#[derive(Default)]
pub struct ToastHost {
    items: Vec<Toast>,
    next_id: u64,
    ticking: bool,
}

impl ToastHost {
    fn start_exit(&mut self, id: u64) {
        if let Some(t) = self.items.iter_mut().find(|t| t.id == id)
            && t.exit_started.is_none()
        {
            t.exit_started = Some(Instant::now());
        }
    }

    /// Advances countdowns (paused while hovered) and drops toasts whose exit finished.
    /// Returns whether any toast is left.
    fn tick(&mut self, dt: Duration) -> bool {
        let now = Instant::now();
        for t in &mut self.items {
            if t.exit_started.is_none() && !t.hovered {
                t.remaining = t.remaining.saturating_sub(dt);
                if t.remaining.is_zero() {
                    t.exit_started = Some(now);
                }
            }
        }
        self.items
            .retain(|t| t.exit_started.is_none_or(|s| now.duration_since(s) < EXIT));
        !self.items.is_empty()
    }
}

struct ToastGlobal(Entity<ToastHost>);
impl Global for ToastGlobal {}

pub fn init(cx: &mut App) -> Entity<ToastHost> {
    let host = cx.new(|_| ToastHost::default());
    cx.set_global(ToastGlobal(host.clone()));
    host
}

pub fn success(cx: &mut App, text: impl Into<SharedString>) {
    push(cx, Kind::Success, text.into(), None);
}

/// A success toast whose click runs `on_click` before dismissing it.
pub fn success_with(
    cx: &mut App,
    text: impl Into<SharedString>,
    on_click: impl Fn(&mut Window, &mut App) + 'static,
) {
    push(cx, Kind::Success, text.into(), Some(Rc::new(on_click)));
}

pub fn error(cx: &mut App, text: impl Into<SharedString>) {
    push(cx, Kind::Error, text.into(), None);
}

fn push(cx: &mut App, kind: Kind, text: SharedString, on_click: Option<OnClick>) {
    let Some(host) = cx.try_global::<ToastGlobal>().map(|g| g.0.clone()) else {
        return;
    };
    host.update(cx, |host, cx| {
        let id = host.next_id;
        host.next_id += 1;
        let ttl = kind.ttl();
        host.items.push(Toast {
            id,
            kind,
            text,
            born: Instant::now(),
            ttl,
            remaining: ttl,
            hovered: false,
            exit_started: None,
            on_click,
        });
        let live: Vec<u64> = host
            .items
            .iter()
            .filter(|t| t.exit_started.is_none())
            .map(|t| t.id)
            .collect();
        if live.len() > MAX_VISIBLE {
            host.start_exit(live[0]);
        }
        cx.notify();
        if !host.ticking {
            host.ticking = true;
            cx.spawn(async move |this, cx| {
                let mut last = Instant::now();
                loop {
                    cx.background_executor().timer(TICK).await;
                    let now = Instant::now();
                    let dt = now.duration_since(last);
                    last = now;
                    let more = this
                        .update(cx, |host, cx| {
                            let more = host.tick(dt);
                            host.ticking = more;
                            cx.notify();
                            more
                        })
                        .unwrap_or(false);
                    if !more {
                        break;
                    }
                }
            })
            .detach();
        }
    });
}

impl Render for ToastHost {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .absolute()
            .top(px(16.0))
            .left_0()
            .right_0()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(8.0))
            .children(self.items.iter().map(|t| {
                let id = t.id;
                let on_click = t.on_click.clone();
                // glow = accent (#ff5c5c / #d97757) as HSL at 35 % alpha
                let (accent, tag, glow) = if t.kind == Kind::Error {
                    (theme::error(), "[ERR]", gpui::hsla(0.0, 1.0, 0.68, 0.35))
                } else {
                    (theme::accent(), "[OK]", gpui::hsla(0.042, 0.63, 0.60, 0.35))
                };
                let frac = t.remaining.as_secs_f32() / t.ttl.as_secs_f32();
                let card = div()
                    .id(("toast", id))
                    .occlude()
                    .relative()
                    .overflow_hidden()
                    .max_w(px(480.0))
                    .px(px(14.0))
                    .pt(px(9.0))
                    .pb(px(11.0))
                    .rounded(px(2.0))
                    .bg(theme::bg_modal())
                    .border_1()
                    .border_color(accent)
                    .shadow(vec![gpui::BoxShadow {
                        color: glow,
                        inset: false,
                        offset: gpui::point(px(0.0), px(0.0)),
                        blur_radius: px(14.0),
                        spread_radius: px(0.0),
                    }])
                    .cursor_pointer()
                    .on_hover(cx.listener(move |host, hovered: &bool, _, cx| {
                        if let Some(t) = host.items.iter_mut().find(|t| t.id == id) {
                            t.hovered = *hovered;
                            cx.notify();
                        }
                    }))
                    .on_click(cx.listener(move |host, _, window, cx| {
                        host.start_exit(id);
                        cx.notify();
                        if let Some(f) = &on_click {
                            // Deferred: the callback may update entities this click is inside of.
                            let f = f.clone();
                            window.defer(cx, move |window, cx| f(window, cx));
                        }
                    }))
                    .font_family(theme::FONT)
                    .text_size(px(13.0))
                    .text_color(theme::text())
                    .child(
                        div()
                            .flex()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .flex_none()
                                    .text_color(accent)
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .child(tag),
                            )
                            .child(div().flex_1().min_w_0().child(t.text.clone())),
                    )
                    .child(
                        div()
                            .absolute()
                            .bottom_0()
                            .left_0()
                            .h(px(2.0))
                            .w(relative(frac.clamp(0.0, 1.0)))
                            .bg(accent),
                    );
                if t.exit_started.is_some() {
                    card.with_animation(
                        ("toast-out", id),
                        Animation::new(EXIT).with_easing(gpui::ease_in_out),
                        |el, p| el.top(px(-SLIDE * p)).opacity(1.0 - p),
                    )
                    .into_any_element()
                } else if t.born.elapsed() < ENTER {
                    card.with_animation(
                        ("toast-in", id),
                        Animation::new(ENTER).with_easing(gpui::ease_out_quint()),
                        |el, p| el.top(px(-SLIDE * (1.0 - p))).opacity(p),
                    )
                    .into_any_element()
                } else {
                    card.into_any_element()
                }
            }))
    }
}
