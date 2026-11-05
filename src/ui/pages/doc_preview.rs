use gpui::{
    Context, EventEmitter, IntoElement, ParentElement, Render, SharedString, Styled, Window, div,
    prelude::*, px,
};

use crate::i18n::{t, t_args};
use crate::route::GoBack;
use crate::state::Stores;
use crate::ui::components::button::small_button;
use crate::ui::components::preview::preview_pane;
use crate::ui::components::smooth_scroll::SmoothScroll;
use crate::ui::theme;

pub struct DocPreviewPage {
    rel: String,
    text: String,
    scroll: SmoothScroll,
    load_error: Option<SharedString>,
}

impl EventEmitter<GoBack> for DocPreviewPage {}

impl DocPreviewPage {
    pub fn new(stores: Stores, project_id: String, rel: String, cx: &mut Context<Self>) -> Self {
        let path = stores
            .projects
            .read(cx)
            .get(&project_id)
            .map(|p| p.path.join("docs").join(&rel));
        let (text, load_error) = match path.map(std::fs::read_to_string) {
            Some(Ok(text)) => (text, None),
            Some(Err(e)) => (String::new(), Some(e.to_string().into())),
            None => (String::new(), Some(t("doc_preview.no_project").into())),
        };
        Self {
            rel,
            text,
            scroll: SmoothScroll::new(),
            load_error,
        }
    }
}

impl Render for DocPreviewPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
                            .truncate()
                            .text_size(px(20.0))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(format!("docs/{}", self.rel)),
                    )
                    .child(small_button(
                        "doc-preview-back",
                        t("common.back"),
                        cx.listener(|_, _, _, cx| cx.emit(GoBack)),
                    )),
            )
            .when_some(self.load_error.clone(), |s, e| {
                s.child(
                    div()
                        .text_size(px(12.0))
                        .text_color(theme::error())
                        .child(t_args(
                            "doc_preview.read_failed",
                            &[("path", &format!("docs/{}", self.rel)), ("error", &e)],
                        )),
                )
            })
            .child(div().flex().flex_1().min_h_0().child(preview_pane(
                "doc-preview-pane",
                &self.text,
                t("skill_editor.nothing_to_preview").into(),
                &self.scroll,
                window,
            )))
    }
}
