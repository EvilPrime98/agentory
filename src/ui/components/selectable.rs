
use std::cell::{Cell, RefCell};
use std::ops::Range;
use std::rc::Rc;

use gpui::{Bounds, HighlightStyle, Pixels, Point, StyledText, TextLayout, point, px};

use crate::ui::theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Pos {
    pub entry: usize,
    pub block: usize,
    pub off: usize,
}

impl Pos {
    fn key(self) -> (usize, usize) {
        (self.entry, self.block)
    }
}

pub fn ordered(a: Pos, b: Pos) -> (Pos, Pos) {
    if a <= b { (a, b) } else { (b, a) }
}

struct Block {
    key: (usize, usize),
    text: String,
    layout: TextLayout,
    prefix: String,
    sep: &'static str,
}

#[derive(Default, Clone)]
pub struct Registry(Rc<RefCell<Vec<Block>>>);

impl Registry {
    pub fn clear(&self) {
        self.0.borrow_mut().clear();
    }

    pub fn hit(&self, at: Point<Pixels>, slop: Option<f32>) -> Option<Pos> {
        let blocks = self.0.borrow();
        let mut best: Option<(f32, &Block, Bounds<Pixels>)> = None;
        for b in blocks.iter() {
            let bounds = b.layout.bounds();
            if bounds.size.width <= px(0.0) || bounds.size.height <= px(0.0) {
                continue;
            }
            let dy = f32::from((bounds.top() - at.y).max(at.y - bounds.bottom()).max(px(0.0)));
            let dx = f32::from((bounds.left() - at.x).max(at.x - bounds.right()).max(px(0.0)));
            if slop.is_some_and(|s| dx > s || dy > s) {
                continue;
            }
            let dist = dy * 10_000.0 + dx;
            if best.as_ref().is_none_or(|(d, ..)| dist < *d) {
                best = Some((dist, b, bounds));
            }
        }
        let (_, b, bounds) = best?;
        let clamped = point(
            at.x.clamp(bounds.left(), bounds.right()),
            at.y.clamp(bounds.top(), bounds.bottom() - px(1.0)),
        );
        let off = match b.layout.index_for_position(clamped) {
            Ok(i) | Err(i) => i.min(b.text.len()),
        };
        let off = (0..=off)
            .rev()
            .find(|i| b.text.is_char_boundary(*i))
            .unwrap_or(0);
        Some(Pos {
            entry: b.key.0,
            block: b.key.1,
            off,
        })
    }

    pub fn copy(&self, a: Pos, b: Pos) -> String {
        let (start, end) = ordered(a, b);
        let mut out = String::new();
        for block in self.0.borrow().iter() {
            if block.key < start.key() || block.key > end.key() {
                continue;
            }
            let from = if block.key == start.key() {
                start.off
            } else {
                0
            };
            let to = if block.key == end.key() {
                end.off.min(block.text.len())
            } else {
                block.text.len()
            };
            let Some(piece) = block.text.get(from..to).filter(|p| !p.is_empty()) else {
                continue;
            };
            if !out.is_empty() {
                out.push_str(block.sep);
            }
            if from == 0 {
                out.push_str(&block.prefix);
            }
            out.push_str(piece);
        }
        out
    }
}

pub struct Sink {
    reg: Registry,
    entry: usize,
    sel: Option<(Pos, Pos)>,
    next: Cell<usize>,
    pending: RefCell<Option<String>>,
    depth: Cell<usize>,
}

impl Sink {
    pub fn new(reg: &Registry, entry: usize, sel: Option<(Pos, Pos)>) -> Self {
        Self {
            reg: reg.clone(),
            entry,
            sel,
            next: Cell::new(0),
            pending: RefCell::new(None),
            depth: Cell::new(0),
        }
    }

    pub fn depth(&self) -> usize {
        self.depth.get()
    }

    pub fn set_depth(&self, depth: usize) {
        self.depth.set(depth);
    }

    pub fn item_marker(&self, marker: String) {
        *self.pending.borrow_mut() = Some(marker);
    }

    pub fn text(
        &self,
        text: String,
        mut highlights: Vec<(Range<usize>, HighlightStyle)>,
    ) -> StyledText {
        let block = self.next.get();
        self.next.set(block + 1);
        let key = (self.entry, block);
        let prefix = self.pending.borrow_mut().take().unwrap_or_default();
        if let Some((s, e)) = self.sel
            && s.key() <= key
            && key <= e.key()
        {
            let from = if key == s.key() { s.off } else { 0 };
            let to = if key == e.key() {
                e.off.min(text.len())
            } else {
                text.len()
            };
            if from < to {
                highlights = with_selection(highlights, from..to);
            }
        }
        let styled = StyledText::new(text.clone()).with_highlights(highlights);
        self.reg.0.borrow_mut().push(Block {
            key,
            text,
            layout: styled.layout().clone(),
            sep: if prefix.is_empty() { "\n\n" } else { "\n" },
            prefix,
        });
        styled
    }
}

fn with_selection(
    highlights: Vec<(Range<usize>, HighlightStyle)>,
    sel: Range<usize>,
) -> Vec<(Range<usize>, HighlightStyle)> {
    let mut color = gpui::rgb_to_hsla(theme::accent());
    color.alpha = 0.4;
    let paint = HighlightStyle {
        background_color: Some(color),
        ..HighlightStyle::default()
    };
    let mut out = Vec::new();
    let mut cursor = sel.start;
    for (r, h) in highlights {
        if r.end <= sel.start || r.start >= sel.end {
            out.push((r, h));
            continue;
        }
        if r.start < sel.start {
            out.push((r.start..sel.start, h));
        }
        let (a, b) = (r.start.max(sel.start), r.end.min(sel.end));
        if a > cursor {
            out.push((cursor..a, paint));
        }
        let mut inner = h;
        inner.background_color = Some(color);
        out.push((a..b, inner));
        cursor = b;
        if r.end > sel.end {
            out.push((sel.end..r.end, h));
        }
    }
    if cursor < sel.end {
        out.push((cursor..sel.end, paint));
    }
    out.sort_by_key(|(r, _)| r.start);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(r: &Range<usize>) -> (usize, usize) {
        (r.start, r.end)
    }

    #[test]
    fn selection_fills_gaps_and_overrides_background() {
        let code = HighlightStyle {
            color: Some(gpui::rgb_to_hsla(theme::accent())),
            ..HighlightStyle::default()
        };
        let out = with_selection(vec![(2..6, code)], 4..10);
        let spans: Vec<_> = out.iter().map(|(r, _)| span(r)).collect();
        assert_eq!(spans, vec![(2, 4), (4, 6), (6, 10)]);
        assert!(out[1].1.background_color.is_some());
        assert!(out[1].1.color.is_some());
        assert!(out[2].1.color.is_none());
    }

    #[test]
    fn plain_selection_is_one_range() {
        let out = with_selection(Vec::new(), 1..3);
        assert_eq!(out.len(), 1);
        assert_eq!(span(&out[0].0), (1, 3));
    }

    #[test]
    fn positions_order_by_document() {
        let p = |entry, block, off| Pos { entry, block, off };
        assert!(p(1, 5, 9) < p(2, 0, 0));
        assert!(p(1, 0, 9) < p(1, 1, 0));
        assert_eq!(ordered(p(2, 0, 0), p(1, 0, 0)), (p(1, 0, 0), p(2, 0, 0)));
    }
}
