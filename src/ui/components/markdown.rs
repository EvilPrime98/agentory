//! Markdown renderer: `pulldown-cmark` events mapped to GPUI elements.
//!
//! `markdown(source)` returns a non-scrolling element; the caller wraps it in whatever scrolls.
//! A leading YAML frontmatter block is shown as a key/value card above the body. Supports
//! headings, paragraphs, emphasis, inline code, nested/ordered/task lists, fenced code (plain
//! monospace), tables, blockquotes, links (opened in the system browser) and rules.

use std::ops::Range;

use gpui::{
    AnyElement, Div, FontStyle, FontWeight, HighlightStyle, InteractiveText, IntoElement,
    ParentElement, StrikethroughStyle, Styled, StyledText, UnderlineStyle, div, prelude::*, px,
};
use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use crate::ui::theme;

/// Render `source` (a whole markdown file, frontmatter included).
pub fn markdown(source: &str) -> Div {
    let (meta, body) = split_frontmatter(source);
    let mut root = div().flex().flex_col().flex_none().gap(px(10.0)).text_size(px(13.0));
    if let Some(meta) = meta {
        root = root.child(meta_card(&meta));
    }
    let opts = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES;
    let mut events = Parser::new_ext(body, opts);
    let mut ids = 0usize;
    root.children(blocks(&mut events, None, &mut ids))
}

/// Split a leading `---` block off. Returns its key/value pairs and the remaining body. A block
/// that never closes is not frontmatter: the whole text is the body.
fn split_frontmatter(src: &str) -> (Option<Vec<(String, String)>>, &str) {
    let mut lines = src.split_inclusive('\n');
    match lines.next() {
        Some(first) if first.trim_end() == "---" => {}
        _ => return (None, src),
    }
    let mut offset = src.split_inclusive('\n').next().map_or(0, str::len);
    let mut pairs: Vec<(String, String)> = Vec::new();
    for line in lines {
        offset += line.len();
        let t = line.trim_end();
        if t == "---" {
            return (Some(pairs), &src[offset..]);
        }
        if t.trim().is_empty() {
            continue;
        }
        if line.starts_with([' ', '\t']) || t.trim_start().starts_with("- ") {
            // Continuation of the previous value (multi-line scalar or list item).
            if let Some((_, v)) = pairs.last_mut() {
                if !v.is_empty() {
                    v.push(' ');
                }
                v.push_str(t.trim());
            }
        } else if let Some((k, v)) = t.split_once(':') {
            pairs.push((k.trim().to_string(), v.trim().trim_matches(['"', '\'']).to_string()));
        }
    }
    (None, src)
}

fn meta_card(pairs: &[(String, String)]) -> Div {
    let mut card = div()
        .flex()
        .flex_col()
        .gap(px(4.0))
        .p(px(10.0))
        .rounded(px(6.0))
        .border_1()
        .border_color(theme::border_subtle())
        .bg(theme::bg_panel());
    for (k, v) in pairs {
        card = card.child(
            div()
                .flex()
                .gap(px(8.0))
                .text_size(px(12.0))
                .child(div().flex_none().w(px(110.0)).text_color(theme::text_muted()).child(k.clone()))
                .child(div().flex_1().min_w_0().child(v.clone())),
        );
    }
    card
}

// ---- inline ----------------------------------------------------------------------------------

#[derive(Default, Clone)]
struct Style {
    bold: bool,
    italic: bool,
    strike: bool,
    code: bool,
    link: Option<String>,
}

#[derive(Default)]
struct Inline {
    text: String,
    highlights: Vec<(Range<usize>, HighlightStyle)>,
    links: Vec<(Range<usize>, String)>,
    stack: Vec<Style>,
    cur: Style,
}

impl Inline {
    fn push(&mut self, s: &str) {
        if s.is_empty() {
            return;
        }
        let start = self.text.len();
        self.text.push_str(s);
        let range = start..self.text.len();
        let c = &self.cur;
        let mut h = HighlightStyle::default();
        let mut styled = false;
        if c.bold {
            h.font_weight = Some(FontWeight::BOLD);
            styled = true;
        }
        if c.italic {
            h.font_style = Some(FontStyle::Italic);
            styled = true;
        }
        if c.strike {
            h.strikethrough = Some(StrikethroughStyle {
                thickness: px(1.0),
                color: None,
            });
            styled = true;
        }
        if c.code {
            h.background_color = Some(gpui::rgb_to_hsla(theme::bg_terminal()));
            h.color = Some(gpui::rgb_to_hsla(theme::accent()));
            styled = true;
        }
        if c.link.is_some() {
            h.color = Some(gpui::rgb_to_hsla(theme::accent()));
            h.underline = Some(UnderlineStyle {
                thickness: px(1.0),
                color: Some(gpui::rgb_to_hsla(theme::accent())),
                wavy: false,
            });
            styled = true;
        }
        if styled {
            self.highlights.push((range.clone(), h));
        }
        if let Some(url) = &self.cur.link {
            self.links.push((range, url.clone()));
        }
    }

    fn open(&mut self, f: impl FnOnce(&mut Style)) {
        self.stack.push(self.cur.clone());
        f(&mut self.cur);
    }

    fn close(&mut self) {
        if let Some(prev) = self.stack.pop() {
            self.cur = prev;
        }
    }

    fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    fn into_element(self, ids: &mut usize) -> AnyElement {
        let styled = StyledText::new(self.text).with_highlights(self.highlights);
        if self.links.is_empty() {
            return styled.into_any_element();
        }
        *ids += 1;
        let (ranges, urls): (Vec<_>, Vec<_>) = self.links.into_iter().unzip();
        InteractiveText::new(("md-text", *ids), styled)
            .on_click(ranges, move |i, _, cx| cx.open_url(&urls[i]))
            .into_any_element()
    }
}

/// Feed one inline-level event into `inline`. Returns false when the event is not inline.
fn inline_event(inline: &mut Inline, ev: &Event) -> bool {
    match ev {
        Event::Text(t) | Event::InlineHtml(t) | Event::Html(t) => inline.push(t),
        Event::Code(t) => {
            inline.open(|s| s.code = true);
            inline.push(t);
            inline.close();
        }
        Event::SoftBreak => inline.push(" "),
        Event::HardBreak => inline.push("\n"),
        Event::FootnoteReference(t) => inline.push(&format!("[{t}]")),
        Event::TaskListMarker(done) => inline.push(if *done { "☑ " } else { "☐ " }),
        Event::Start(Tag::Emphasis) => inline.open(|s| s.italic = true),
        Event::Start(Tag::Strong) => inline.open(|s| s.bold = true),
        Event::Start(Tag::Strikethrough) => inline.open(|s| s.strike = true),
        Event::Start(Tag::Link { dest_url, .. }) => {
            let url = dest_url.to_string();
            inline.open(|s| s.link = Some(url));
        }
        // Images render as their alt text.
        Event::Start(Tag::Image { .. }) => inline.open(|_| {}),
        Event::End(TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough | TagEnd::Link | TagEnd::Image) => {
            inline.close()
        }
        _ => return false,
    }
    true
}

// ---- blocks ----------------------------------------------------------------------------------

type Events<'a, 'b> = &'b mut dyn Iterator<Item = Event<'a>>;

fn flush(inline: &mut Inline, out: &mut Vec<AnyElement>, ids: &mut usize) {
    if !inline.is_empty() {
        out.push(std::mem::take(inline).into_element(ids));
    }
}

/// Parse blocks until `end` (or the end of the stream).
fn blocks<'a>(events: Events<'a, '_>, end: Option<TagEnd>, ids: &mut usize) -> Vec<AnyElement> {
    let mut out: Vec<AnyElement> = Vec::new();
    // Tight list items carry bare inline events; they gather here until a block boundary.
    let mut inline = Inline::default();
    while let Some(ev) = events.next() {
        if let Event::End(e) = &ev
            && Some(*e) == end
        {
            break;
        }
        if inline_event(&mut inline, &ev) {
            continue;
        }
        match ev {
            Event::Start(Tag::Paragraph) => {
                flush(&mut inline, &mut out, ids);
                let el = inline_until(events, TagEnd::Paragraph, ids);
                out.push(el);
            }
            Event::Start(Tag::Heading { level, .. }) => {
                flush(&mut inline, &mut out, ids);
                let size = match level {
                    HeadingLevel::H1 => 22.0,
                    HeadingLevel::H2 => 19.0,
                    HeadingLevel::H3 => 17.0,
                    HeadingLevel::H4 => 15.0,
                    _ => 14.0,
                };
                let el = inline_until(events, TagEnd::Heading(level), ids);
                out.push(
                    div()
                        .mt(px(4.0))
                        .text_size(px(size))
                        .font_weight(FontWeight::BOLD)
                        .child(el)
                        .into_any_element(),
                );
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                flush(&mut inline, &mut out, ids);
                let mut code = String::new();
                for ev in &mut *events {
                    match ev {
                        Event::Text(t) => code.push_str(&t),
                        Event::End(TagEnd::CodeBlock) => break,
                        _ => {}
                    }
                }
                let _ = matches!(kind, CodeBlockKind::Fenced(_));
                out.push(
                    div()
                        .p(px(10.0))
                        .rounded(px(6.0))
                        .bg(theme::bg_terminal())
                        .text_size(px(12.0))
                        .child(code.trim_end_matches('\n').to_string())
                        .into_any_element(),
                );
            }
            Event::Start(Tag::BlockQuote(_)) => {
                flush(&mut inline, &mut out, ids);
                let inner = blocks(events, Some(TagEnd::BlockQuote(None)), ids);
                out.push(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(6.0))
                        .pl(px(10.0))
                        .border_l_2()
                        .border_color(theme::border_subtle())
                        .text_color(theme::text_muted())
                        .children(inner)
                        .into_any_element(),
                );
            }
            Event::Start(Tag::List(first)) => {
                flush(&mut inline, &mut out, ids);
                out.push(list(events, first, ids));
            }
            Event::Start(Tag::Table(_)) => {
                flush(&mut inline, &mut out, ids);
                out.push(table(events, ids));
            }
            Event::Rule => {
                flush(&mut inline, &mut out, ids);
                out.push(div().h(px(1.0)).bg(theme::border_subtle()).into_any_element());
            }
            // Any other container (footnote definitions, HTML blocks, ...): render its content.
            Event::Start(_) => flush(&mut inline, &mut out, ids),
            Event::End(_) => flush(&mut inline, &mut out, ids),
            _ => {}
        }
    }
    flush(&mut inline, &mut out, ids);
    out
}

/// Collect inline events up to `end` into one text element.
fn inline_until<'a>(events: Events<'a, '_>, end: TagEnd, ids: &mut usize) -> AnyElement {
    let mut inline = Inline::default();
    for ev in &mut *events {
        if let Event::End(e) = &ev
            && *e == end
        {
            break;
        }
        inline_event(&mut inline, &ev);
    }
    inline.into_element(ids)
}

fn list<'a>(events: Events<'a, '_>, first: Option<u64>, ids: &mut usize) -> AnyElement {
    let mut col = div().flex().flex_col().gap(px(4.0));
    let mut n = first.unwrap_or(1);
    while let Some(ev) = events.next() {
        match ev {
            Event::Start(Tag::Item) => {
                let marker = if first.is_some() {
                    let m = format!("{n}.");
                    n += 1;
                    m
                } else {
                    "•".to_string()
                };
                let inner = blocks(events, Some(TagEnd::Item), ids);
                col = col.child(
                    div()
                        .flex()
                        .gap(px(8.0))
                        .child(div().flex_none().text_color(theme::text_muted()).child(marker))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .flex_1()
                                .min_w_0()
                                .gap(px(4.0))
                                .children(inner),
                        ),
                );
            }
            Event::End(TagEnd::List(_)) => break,
            _ => {}
        }
    }
    div().pl(px(6.0)).child(col).into_any_element()
}

fn table<'a>(events: Events<'a, '_>, ids: &mut usize) -> AnyElement {
    let mut rows: Vec<(bool, Vec<AnyElement>)> = Vec::new();
    let mut head = false;
    let mut cells: Vec<AnyElement> = Vec::new();
    while let Some(ev) = events.next() {
        match ev {
            Event::Start(Tag::TableHead) => head = true,
            Event::End(TagEnd::TableHead) => {
                rows.push((true, std::mem::take(&mut cells)));
                head = false;
            }
            Event::End(TagEnd::TableRow) => rows.push((head, std::mem::take(&mut cells))),
            Event::Start(Tag::TableCell) => cells.push(inline_until(events, TagEnd::TableCell, ids)),
            Event::End(TagEnd::Table) => break,
            _ => {}
        }
    }
    let mut t = div()
        .flex()
        .flex_col()
        .rounded(px(6.0))
        .border_1()
        .border_color(theme::border_subtle())
        .overflow_hidden();
    let last = rows.len().saturating_sub(1);
    for (i, (is_head, cells)) in rows.into_iter().enumerate() {
        t = t.child(
            div()
                .flex()
                .when(is_head, |r| r.bg(theme::bg_panel()).font_weight(FontWeight::BOLD))
                .when(i != last, |r| r.border_b_1().border_color(theme::border_subtle()))
                .children(cells.into_iter().map(|c| {
                    div().flex_1().min_w_0().px(px(8.0)).py(px(5.0)).child(c)
                })),
        );
    }
    t.into_any_element()
}

#[cfg(test)]
mod tests {
    use super::split_frontmatter;

    #[test]
    fn splits_frontmatter() {
        let (m, body) = split_frontmatter("---\r\nname: a\r\ndescription: \"b\"\r\n---\r\n# Hi\r\n");
        assert_eq!(m.unwrap(), vec![("name".into(), "a".into()), ("description".into(), "b".into())]);
        assert_eq!(body, "# Hi\r\n");
    }

    #[test]
    fn unclosed_or_absent_is_body() {
        assert!(split_frontmatter("---\nname: a\n").0.is_none());
        assert_eq!(split_frontmatter("# x").1, "# x");
    }
}
