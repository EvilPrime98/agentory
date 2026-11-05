use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    ColorExt, Div, InteractiveElement, IntoElement, MouseButton, MouseMoveEvent, MouseUpEvent,
    ParentElement, ScrollHandle, StatefulInteractiveElement, Styled, canvas, div, px,
};

use crate::ui::theme;

const MIN_THUMB: f32 = 24.0;
const TRACK_W: f32 = 8.0;

#[derive(Clone, Default)]
pub struct Scrollbar {
    handle: ScrollHandle,
    drag: Rc<Cell<Option<f32>>>,
    track_top: Rc<Cell<f32>>,
    target: Option<Rc<Cell<f32>>>,
}

impl Scrollbar {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn linked(handle: ScrollHandle, target: Rc<Cell<f32>>) -> Self {
        Self {
            handle,
            target: Some(target),
            ..Self::default()
        }
    }

    pub fn overlay<E>(&self, scroll: E) -> Div
    where
        E: StatefulInteractiveElement + Styled + IntoElement,
    {
        div()
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .child(scroll.track_scroll(&self.handle))
            .children(self.bar())
    }

    pub fn bar(&self) -> Option<gpui::Stateful<Div>> {
        let view = f32::from(self.handle.bounds().size.height);
        let max = f32::from(self.handle.max_offset().y);
        if max <= 0.0 || view <= 0.0 {
            return None;
        }
        let (top, height) = thumb(view, max, f32::from(self.handle.offset().y));
        let dragging = self.drag.get().is_some();

        let down = self.clone();
        let (move_bar, up_bar) = (self.clone(), self.clone());
        let origin = self.track_top.clone();

        Some(
            div()
                .id("scrollbar")
                .absolute()
                .top_0()
                .bottom_0()
                .right_0()
                .w(px(TRACK_W))
                .occlude()
                .on_mouse_down(MouseButton::Left, move |ev, window, cx| {
                    let y = f32::from(ev.position.y) - down.track_top.get();
                    let (top, height) = thumb(view, max, f32::from(down.handle.offset().y));
                    let grab = if y >= top && y <= top + height {
                        y - top
                    } else {
                        height / 2.0
                    };
                    down.drag.set(Some(grab));
                    down.scroll_to(y, grab, view, max);
                    window.refresh();
                    cx.stop_propagation();
                })
                .child(
                    canvas(
                        move |bounds, _, _| origin.set(f32::from(bounds.origin.y)),
                        move |_, _, window, _| {
                            let bar = move_bar.clone();
                            window.on_mouse_event(move |ev: &MouseMoveEvent, phase, window, _| {
                                if !phase.bubble() {
                                    return;
                                }
                                let Some(grab) = bar.drag.get() else { return };
                                let view = f32::from(bar.handle.bounds().size.height);
                                let max = f32::from(bar.handle.max_offset().y);
                                let y = f32::from(ev.position.y) - bar.track_top.get();
                                bar.scroll_to(y, grab, view, max);
                                window.refresh();
                            });
                            let bar = up_bar.clone();
                            window.on_mouse_event(move |_: &MouseUpEvent, phase, window, _| {
                                if phase.bubble() && bar.drag.take().is_some() {
                                    window.refresh();
                                }
                            });
                        },
                    )
                    .absolute()
                    .size_full(),
                )
                .child(
                    div()
                        .id("scrollbar-thumb")
                        .absolute()
                        .top(px(top))
                        .left(px(2.0))
                        .w(px(4.0))
                        .h(px(height))
                        .rounded(px(2.0))
                        .bg(if dragging {
                            theme::accent()
                        } else {
                            theme::text_muted().opacity(0.35)
                        })
                        .hover(|s| s.bg(theme::text_muted().opacity(0.6))),
                ),
        )
    }

    fn scroll_to(&self, y: f32, grab: f32, view: f32, max: f32) {
        if max <= 0.0 {
            return;
        }
        let (_, height) = thumb(view, max, 0.0);
        let frac = ((y - grab) / (view - height).max(1.0)).clamp(0.0, 1.0);
        let offset = -frac * max;
        let mut p = self.handle.offset();
        p.y = px(offset);
        self.handle.set_offset(p);
        if let Some(target) = &self.target {
            target.set(offset);
        }
    }
}

fn thumb(view: f32, max: f32, offset: f32) -> (f32, f32) {
    let height = (view * view / (view + max)).clamp(MIN_THUMB.min(view), view);
    let progress = (-offset / max).clamp(0.0, 1.0);
    (progress * (view - height).max(0.0), height)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thumb_spans_track_proportionally() {
        let (top, h) = thumb(400.0, 400.0, 0.0);
        assert_eq!((top, h), (0.0, 200.0));
        let (top, h) = thumb(400.0, 400.0, -400.0);
        assert_eq!((top, h), (200.0, 200.0));
    }

    #[test]
    fn thumb_has_minimum_height() {
        let (_, h) = thumb(400.0, 100_000.0, 0.0);
        assert_eq!(h, MIN_THUMB);
    }
}
