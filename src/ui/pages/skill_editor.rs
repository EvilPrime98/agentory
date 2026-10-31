//! Skill editor page: edits an installed skill's `SKILL.md` in place.

use std::path::PathBuf;

use gpui::{
    Context, Entity, EventEmitter, IntoElement, ParentElement, Render,
    SharedString, Styled, Subscription, Window, div, prelude::*, px,
};

use crate::route::GoBack;
use crate::state::Stores;
use crate::ui::components::button::small_button;
use crate::ui::components::markdown::markdown;
use crate::ui::components::smooth_scroll::SmoothScroll;
use crate::ui::components::text_area::{TextArea, TextAreaEvent};
use crate::ui::theme;
use crate::ui::toast;

pub struct SkillEditorPage {
    name: String,
    path: PathBuf,
    area: Entity<TextArea>,
    preview_scroll: SmoothScroll,
    /// What is on disk, to tell whether the editor is dirty.
    saved: String,
    /// Why `SKILL.md` could not be read; the editor stays read-only-empty and saving is disabled.
    load_error: Option<SharedString>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<GoBack> for SkillEditorPage {}

impl SkillEditorPage {
    pub fn new(stores: Stores, name: String, cx: &mut Context<Self>) -> Self {
        let project = stores.skills.read(cx).project.clone().unwrap_or_default();
        let path = project
            .join(".claude")
            .join("skills")
            .join(&name)
            .join("SKILL.md");
        let (saved, load_error) = match std::fs::read_to_string(&path) {
            Ok(text) => (text, None),
            Err(e) => (
                String::new(),
                Some(format!("Could not read {}: {e}", path.display()).into()),
            ),
        };
        let area = cx.new(|cx| {
            let mut a = TextArea::new(cx);
            a.set_value(&saved, cx);
            a
        });
        let subs = vec![cx.subscribe(&area, |this, _, ev: &TextAreaEvent, cx| match ev {
            TextAreaEvent::Changed => cx.notify(),
            TextAreaEvent::Save => this.save(cx),
        })];
        Self {
            name,
            path,
            area,
            preview_scroll: SmoothScroll::new(),
            saved,
            load_error,
            _subs: subs,
        }
    }

    /// Whether the editor holds edits that are not on disk.
    pub fn is_dirty(&self, cx: &gpui::App) -> bool {
        self.load_error.is_none() && self.area.read(cx).value() != self.saved
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        if self.load_error.is_some() || !self.is_dirty(cx) {
            return;
        }
        let text = self.area.read(cx).value();
        match std::fs::write(&self.path, &text) {
            Ok(()) => {
                self.saved = text;
                toast::success(cx, format!("Saved {}", self.name));
            }
            Err(e) => toast::error(cx, format!("Could not save {}: {e}", self.name)),
        }
        cx.notify();
    }
}

impl Render for SkillEditorPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let dirty = self.is_dirty(cx);
        let text = self.area.read(cx).value();
        let preview = if text.trim().is_empty() {
            div()
                .text_size(px(12.0))
                .text_color(theme::text_muted())
                .child("Nothing to preview")
        } else {
            markdown(&text)
        };
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .p(px(24.0))
            .gap(px(12.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8.0))
                    .child(
                        div()
                            .flex()
                            .items_baseline()
                            .gap(px(8.0))
                            .min_w_0()
                            .child(
                                div()
                                    .text_size(px(20.0))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child("Editing Skill"),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(13.0))
                                    .text_color(theme::text_muted())
                                    .child(self.name.clone()),
                            )
                            .when(dirty, |s| {
                                s.child(
                                    div()
                                        .flex_none()
                                        .text_size(px(12.0))
                                        .text_color(theme::warning())
                                        .child("● Unsaved changes"),
                                )
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .child(small_button(
                                "skill-editor-back",
                                "Back",
                                cx.listener(|_, _, _, cx| cx.emit(GoBack)),
                            ))
                            .child(small_button(
                                "skill-editor-save",
                                "Save",
                                cx.listener(|this, _, _, cx| this.save(cx)),
                            )),
                    ),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(theme::text_muted())
                    .child("Updating skills with spm may overwrite local changes."),
            )
            .when_some(self.load_error.clone(), |s, e| {
                s.child(
                    div()
                        .text_size(px(12.0))
                        .text_color(theme::error())
                        .child(e),
                )
            })
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .gap(px(12.0))
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .min_w_0()
                            .rounded(px(6.0))
                            .border_1()
                            .border_color(theme::border_subtle())
                            .bg(theme::bg_app())
                            .child(self.area.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .rounded(px(6.0))
                            .border_1()
                            .border_color(theme::border_subtle())
                            .bg(theme::bg_app())
                            .child(self.preview_scroll.attach(
                                div()
                                    .id("skill-editor-preview")
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .min_h_0()
                                    .p(px(16.0))
                                    .child(preview),
                                window,
                            )),
                    ),
            )
    }
}
