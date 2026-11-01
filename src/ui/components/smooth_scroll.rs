//! Eased wheel scrolling for a vertically scrolling list.
//!
//! GPUI applies wheel deltas to the scroll offset immediately. `SmoothScroll` owns a
//! `ScrollHandle` and a target offset instead: notched wheel (line) deltas retarget an
//! exponential-decay animation, precise (trackpad) deltas apply instantly. The wheel event
//! is only consumed while the list can still move, so leftover delta chains to the parent.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    IntoElement, ParentElement, ScrollDelta, ScrollHandle, ScrollWheelEvent,
    StatefulInteractiveElement, Styled, div, px,
};

use crate::ui::theme;

/// Fraction of the remaining distance covered each frame.
const DECAY: f32 = 0.18;
const FRAME_PAD: f32 = 12.0;
/// Distance (px) under which the animation snaps to its target.
const SETTLE: f32 = 0.5;

#[derive(Clone, Default)]
pub struct SmoothScroll {
    handle: ScrollHandle,
    /// Target scroll offset (<= 0, like `ScrollHandle::offset`).
    target: Rc<Cell<f32>>,
}

impl SmoothScroll {
    pub fn new() -> Self {
        Self::default()
    }

    /// Apply to the scrolling element: tracks the handle, hides overflow and routes the wheel.
    pub fn attach<E>(&self, el: E, window: &mut gpui::Window) -> E
    where
        E: StatefulInteractiveElement + Styled,
    {
        self.step(window);
        let this = self.clone();
        let view = window.current_view();
        let line = f32::from(window.line_height());
        el.overflow_hidden()
            .track_scroll(&self.handle)
            .on_scroll_wheel(move |ev, _, cx| {
                if this.on_wheel(ev, line) {
                    cx.stop_propagation();
                    cx.notify(view);
                }
            })
    }

    pub fn framed<E>(&self, el: E, window: &mut gpui::Window) -> gpui::Div
    where
        E: StatefulInteractiveElement + Styled + IntoElement,
    {
        let list = self.attach(el.pr(px(10.0)), window);
        let max = f32::from(self.handle.max_offset().y);
        let view = f32::from(self.handle.bounds().size.height);
        let thumb = (max > 0.0 && view > 0.0).then(|| {
            let h = (view * view / (view + max)).max(24.0).min(view);
            let progress = (-f32::from(self.handle.offset().y) / max).clamp(0.0, 1.0);
            div()
                .absolute()
                .right(px(4.0))
                .top(px(FRAME_PAD + progress * (view - h).max(0.0)))
                .w(px(4.0))
                .h(px(h))
                .rounded_full()
                .bg(theme::text_muted())
                .opacity(0.7)
        });
        div()
            .relative()
            .p(px(FRAME_PAD))
            .rounded(px(10.0))
            .bg(theme::bg_panel())
            .border_1()
            .border_color(theme::border_subtle())
            .overflow_hidden()
            .child(list)
            .children(thumb)
    }

    /// Advance the animation one frame, requesting another until it settles.
    fn step(&self, window: &mut gpui::Window) {
        let max = f32::from(self.handle.max_offset().y);
        // Content may have shrunk (filtering): keep the target inside the bounds.
        let target = self.target.get().clamp(-max, 0.0);
        self.target.set(target);

        let mut offset = self.handle.offset();
        let current = f32::from(offset.y);
        let remaining = target - current;
        if remaining.abs() <= f32::EPSILON {
            return;
        }
        let next = if remaining.abs() <= SETTLE {
            target
        } else {
            current + remaining * DECAY
        };
        offset.y = px(next);
        self.handle.set_offset(offset);
        if next != target {
            window.request_animation_frame();
        }
    }

    /// Returns true when the event was consumed.
    fn on_wheel(&self, ev: &ScrollWheelEvent, line: f32) -> bool {
        let max = f32::from(self.handle.max_offset().y);
        if max <= 0.0 {
            return false;
        }
        let current = f32::from(self.handle.offset().y);
        let (delta, precise) = match ev.delta {
            ScrollDelta::Lines(l) => (l.y * line, false),
            ScrollDelta::Pixels(p) => (f32::from(p.y), true),
        };
        if delta == 0.0 {
            return false;
        }
        // Retarget from the pending target so rapid ticks accumulate.
        let base = if precise { current } else { self.target.get() };
        let next = (base + delta).clamp(-max, 0.0);
        let can_move = (next - base).abs() > f32::EPSILON;
        if !can_move {
            return false;
        }
        self.target.set(next);
        if precise {
            let mut offset = self.handle.offset();
            offset.y = px(next);
            self.handle.set_offset(offset);
        }
        true
    }
}
