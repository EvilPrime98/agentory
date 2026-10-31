//! Docs page: the project's `docs/**/*.md` files, each deletable or the start of an agent session.

use std::path::{Path, PathBuf};

use gpui::{
    AnyElement, Context, EventEmitter, InteractiveElement, IntoElement, ParentElement, Render,
    SharedString, StatefulInteractiveElement, Styled, Window, div, prelude::*, px,
};

use super::skills::{card, icon_button, muted_note, row_base};
use crate::i18n::{t, t_args};
use crate::route::{GoBack, Navigate, Route};
use crate::state::Stores;
use crate::state::sessions;
use crate::ui::confirm::{self, ConfirmOptions};
use crate::ui::icons::{Icon, icon};
use crate::ui::{theme, toast};

struct Doc {
    /// Path relative to `docs/`, with `/` separators.
    rel: String,
    /// The first `# heading`, when the file has one.
    title: Option<String>,
}

/// What the last scan found.
enum Scan {
    NoFolder,
    Docs(Vec<Doc>),
}

pub struct DocsPage {
    stores: Stores,
    project_id: String,
    scan: Scan,
}

impl EventEmitter<Navigate> for DocsPage {}
impl EventEmitter<GoBack> for DocsPage {}

fn docs_dir(root: &Path) -> PathBuf {
    root.join("docs")
}

/// First level-1 heading outside code fences.
fn first_heading(text: &str) -> Option<String> {
    let mut fenced = false;
    for line in text.lines() {
        let line = line.trim_end();
        if line.trim_start().starts_with("```") || line.trim_start().starts_with("~~~") {
            fenced = !fenced;
        } else if !fenced {
            if let Some(h) = line.strip_prefix("# ") {
                let h = h.trim();
                if !h.is_empty() {
                    return Some(h.to_string());
                }
            }
        }
    }
    None
}

fn collect(dir: &Path, base: &Path, out: &mut Vec<Doc>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        // `file_type` does not follow symlinks, so a link cycle cannot loop the scan.
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if kind.is_dir() {
            collect(&path, base, out);
        } else if kind.is_file()
            && path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("md"))
        {
            let Ok(rel) = path.strip_prefix(base) else {
                continue;
            };
            let rel = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            let title = std::fs::read_to_string(&path)
                .ok()
                .and_then(|t| first_heading(&t));
            out.push(Doc { rel, title });
        }
    }
}

fn scan(root: &Path) -> Scan {
    let dir = docs_dir(root);
    if !dir.is_dir() {
        return Scan::NoFolder;
    }
    let mut docs = Vec::new();
    collect(&dir, &dir, &mut docs);
    docs.sort_by(|a, b| a.rel.cmp(&b.rel));
    Scan::Docs(docs)
}

impl DocsPage {
    pub fn new(stores: Stores, project_id: String, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            stores,
            project_id,
            scan: Scan::NoFolder,
        };
        this.rescan(cx);
        this
    }

    fn project_root(&self, cx: &Context<Self>) -> Option<PathBuf> {
        self.stores
            .projects
            .read(cx)
            .get(&self.project_id)
            .map(|p| p.path.clone())
    }

    fn rescan(&mut self, cx: &mut Context<Self>) {
        self.scan = self
            .project_root(cx)
            .map_or(Scan::NoFolder, |root| scan(&root));
        cx.notify();
    }

    fn start_working(&self, rel: &str, cx: &mut Context<Self>) {
        let mut input = format!("@docs/{rel} ");
        if self.stores.prefs.read(cx).prefs.send_doc_prompt {
            // Enter, so the prefilled mention is submitted.
            input.push('\r');
        }
        sessions::begin_new_input(&self.project_id, Some(input), cx);
        cx.emit(Navigate(Route::Sessions));
    }

    fn confirm_delete(&self, rel: String, cx: &mut Context<Self>) {
        let Some(root) = self.project_root(cx) else {
            return;
        };
        let path = docs_dir(&root).join(&rel);
        let options =
            ConfirmOptions::new(
            t("docs.delete.title"),
            t_args("docs.delete.body", &[("rel", &rel)]),
        )
        .labels(t("common.delete"), t("common.cancel"));
        let page = cx.entity();
        confirm::ask(cx, options, move |answer, _, cx| {
            if answer != Some(true) {
                return;
            }
            if let Err(e) = trash::delete(&path) {
                tracing::error!("could not trash {}: {e}", path.display());
                toast::error(cx, t_args("docs.delete.failed", &[("rel", &rel), ("error", &e.to_string())]));
            }
            page.update(cx, |this, cx| this.rescan(cx));
        });
    }
}

impl Render for DocsPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (count, body): (Option<usize>, Vec<AnyElement>) = match &self.scan {
            Scan::NoFolder => (
                None,
                vec![muted_note(t("docs.no_folder").into()).into_any_element()],
            ),
            Scan::Docs(docs) if docs.is_empty() => (
                Some(0),
                vec![muted_note(t("docs.no_files").into()).into_any_element()],
            ),
            Scan::Docs(docs) => (
                Some(docs.len()),
                docs.iter()
                    .enumerate()
                    .map(|(i, doc)| {
                        let (work_rel, del_rel) = (doc.rel.clone(), doc.rel.clone());
                        row_base(i)
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .min_w_0()
                                    .child(
                                        div()
                                            .truncate()
                                            .text_size(px(13.0))
                                            .text_color(theme::text())
                                            .child(doc.rel.clone()),
                                    )
                                    .when_some(doc.title.clone(), |s, t| {
                                        s.child(
                                            div()
                                                .truncate()
                                                .text_size(px(11.0))
                                                .text_color(theme::text_muted())
                                                .child(t),
                                        )
                                    }),
                            )
                            .child(icon_button(
                                SharedString::from(format!("work-doc-{}", doc.rel)),
                                Icon::Play,
                                cx.listener(move |this, _, _, cx| {
                                    this.start_working(&work_rel, cx)
                                }),
                            ))
                            .child(icon_button(
                                SharedString::from(format!("rm-doc-{}", doc.rel)),
                                Icon::Trash,
                                cx.listener(move |this, _, _, cx| {
                                    this.confirm_delete(del_rel.clone(), cx)
                                }),
                            ))
                            .into_any_element()
                    })
                    .collect(),
            ),
        };

        div()
            .id("docs-scroll")
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
                    .w_full()
                    .max_w(px(1100.0))
                    .p(px(24.0))
                    .gap(px(12.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_size(px(20.0))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child(t("docs.title")),
                            )
                            .child(
                                div()
                                    .id("docs-refresh")
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .size(px(26.0))
                                    .rounded(px(6.0))
                                    .cursor_pointer()
                                    .hover(|s| s.bg(theme::hover()))
                                    .on_click(cx.listener(|this, _, _, cx| this.rescan(cx)))
                                    .child(icon(Icon::Refresh, px(15.0)).text_color(theme::text())),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .child(card(&t("docs.card.files"), count, None, body)),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heading_skips_code_fences() {
        let t = "intro\n```\n# not this\n```\n# Real title \n";
        assert_eq!(first_heading(t).as_deref(), Some("Real title"));
        assert_eq!(first_heading("no heading"), None);
        assert_eq!(first_heading("## sub only"), None);
    }
}
