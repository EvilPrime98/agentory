
use std::ops::Range;

use gpui::{Font, Hsla, Pixels, SharedString, TextRun, Window, WrappedLine, px};

use super::buffer::Buffer;

pub const NEWLINE_CELL: f32 = 6.0;

#[derive(Clone)]
pub struct Params {
    pub font: Font,
    pub font_size: Pixels,
    pub line_height: Pixels,
    pub width: Pixels,
    pub line_numbers: bool,
    pub placeholder: SharedString,
    pub color: Hsla,
    pub faint: Hsla,
}

impl Params {
    pub fn same_shape(&self, other: &Params) -> bool {
        self.font == other.font
            && self.font_size == other.font_size
            && self.line_height == other.line_height
            && self.width == other.width
            && self.line_numbers == other.line_numbers
            && self.placeholder == other.placeholder
            && self.color == other.color
            && self.faint == other.faint
    }
}

pub struct WrapLayout {
    pub version: usize,
    pub params: Params,
    pub gutter: Pixels,
    pub lines: Vec<WrappedLine>,
    pub row_starts: Vec<usize>,
    breaks: Vec<Vec<(usize, Pixels)>>,
    pub placeholder: Vec<WrappedLine>,
}

fn run(font: &Font, len: usize, color: Hsla) -> TextRun {
    TextRun {
        len,
        font: font.clone(),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
        letter_spacing: None,
    }
}

fn shape(
    window: &Window,
    text: &str,
    params: &Params,
    wrap: Pixels,
    color: Hsla,
) -> Vec<WrappedLine> {
    window
        .text_system()
        .shape_text(
            SharedString::from(text.to_string()),
            params.font_size,
            &[run(&params.font, text.len(), color)],
            Some(wrap),
            None,
        )
        .map(|lines| lines.into_vec())
        .unwrap_or_default()
}

fn line_breaks(line: &WrappedLine) -> Vec<(usize, Pixels)> {
    let mut breaks = vec![(0, px(0.0))];
    for boundary in line.wrap_boundaries() {
        let glyph = &line.runs()[boundary.run_ix].glyphs[boundary.glyph_ix];
        breaks.push((glyph.index, glyph.position.x));
    }
    breaks
}

pub fn gutter_width(window: &Window, params: &Params, line_count: usize) -> Pixels {
    if !params.line_numbers {
        return px(0.0);
    }
    let digits = line_count.to_string().len();
    let ts = window.text_system();
    let digit = ts
        .advance(ts.resolve_font(&params.font), params.font_size, '0')
        .map(|s| s.width)
        .unwrap_or(params.font_size * 0.6);
    digit * digits as f32 + px(14.0)
}

impl WrapLayout {
    pub fn new(buffer: &Buffer, params: Params, window: &Window) -> Self {
        let gutter = gutter_width(window, &params, buffer.line_count());
        let wrap = (params.width - gutter).max(px(20.0));
        let mut lines = shape(window, &buffer.text(), &params, wrap, params.color);
        while lines.len() < buffer.line_count() {
            lines.push(WrappedLine::default());
        }
        let mut row_starts = Vec::with_capacity(lines.len() + 1);
        let mut breaks = Vec::with_capacity(lines.len());
        let mut rows = 0;
        for line in &lines {
            row_starts.push(rows);
            let b = line_breaks(line);
            rows += b.len();
            breaks.push(b);
        }
        row_starts.push(rows);
        let placeholder = if buffer.is_empty() && !params.placeholder.is_empty() {
            shape(window, &params.placeholder, &params, wrap, params.faint)
        } else {
            Vec::new()
        };
        Self {
            version: buffer.version(),
            params,
            gutter,
            lines,
            row_starts,
            breaks,
            placeholder,
        }
    }

    pub fn is_current(&self, buffer: &Buffer, params: &Params) -> bool {
        self.version == buffer.version() && self.params.same_shape(params)
    }

    pub fn line_height(&self) -> Pixels {
        self.params.line_height
    }

    pub fn total_rows(&self) -> usize {
        *self.row_starts.last().unwrap_or(&1)
    }

    pub fn placeholder_rows(&self) -> usize {
        self.placeholder
            .iter()
            .map(|line| line.wrap_boundaries().len() + 1)
            .sum()
    }

    pub fn height(&self) -> Pixels {
        self.line_height() * self.total_rows().max(self.placeholder_rows()) as f32
    }

    pub fn line_of_row(&self, row: usize) -> usize {
        self.row_starts
            .partition_point(|&start| start <= row)
            .saturating_sub(1)
            .min(self.lines.len().saturating_sub(1))
    }

    pub fn row_columns(&self, line: usize, k: usize) -> Range<usize> {
        let b = &self.breaks[line];
        let start = b[k].0;
        let end = b.get(k + 1).map_or(self.lines[line].len(), |next| next.0);
        start..end
    }

    pub fn rows_in_line(&self, line: usize) -> usize {
        self.breaks[line].len()
    }

    fn x_in_row(&self, line: usize, k: usize, col: usize) -> Pixels {
        self.lines[line].unwrapped_layout.x_for_index(col) - self.breaks[line][k].1
    }

    pub fn locate(&self, line: usize, col: usize) -> (usize, Pixels) {
        let line = line.min(self.lines.len() - 1);
        let b = &self.breaks[line];
        let k = b.partition_point(|&(index, _)| index <= col).max(1) - 1;
        (self.row_starts[line] + k, self.x_in_row(line, k, col))
    }

    pub fn locate_offset(&self, buffer: &Buffer, offset: usize) -> (usize, Pixels) {
        let point = buffer.offset_to_point(offset);
        self.locate(point.line, point.column)
    }

    pub fn offset_at(&self, buffer: &Buffer, row: usize, x: Pixels) -> usize {
        let row = row.min(self.total_rows().saturating_sub(1));
        let line = self.line_of_row(row);
        let k = (row - self.row_starts[line]).min(self.breaks[line].len() - 1);
        let cols = self.row_columns(line, k);
        let layout = &self.lines[line].unwrapped_layout;
        let target = x.max(px(0.0)) + self.breaks[line][k].1;
        let mut col = layout
            .closest_index_for_x(target)
            .clamp(cols.start, cols.end);
        if k + 1 < self.breaks[line].len() && col >= cols.end {
            let text = &self.lines[line].text;
            col = text[..cols.end]
                .char_indices()
                .next_back()
                .map_or(cols.start, |(i, _)| i)
                .max(cols.start);
        }
        buffer.clip(buffer.line_start(line) + col)
    }

    pub fn offset_for_point(&self, buffer: &Buffer, x: Pixels, y: Pixels) -> usize {
        let row = (y / self.line_height()).floor().max(0.0) as usize;
        self.offset_at(buffer, row, x)
    }

    pub fn range_rects(
        &self,
        buffer: &Buffer,
        range: Range<usize>,
        newline_cell: bool,
        rows: Range<usize>,
    ) -> Vec<(usize, Pixels, Pixels)> {
        let mut out = Vec::new();
        if range.is_empty() {
            return out;
        }
        let first = buffer.line_of(range.start);
        let last = buffer.line_of(range.end);
        for line in first..=last {
            let line_start = buffer.line_start(line);
            let line_end = buffer.line_end(line);
            if range.end <= line_start && line != first {
                break;
            }
            let from = range.start.max(line_start) - line_start;
            let past_end = range.end > line_end;
            let to = range.end.min(line_end) - line_start;
            let count = self.rows_in_line(line);
            for k in 0..count {
                let row = self.row_starts[line] + k;
                if row < rows.start || row >= rows.end {
                    continue;
                }
                let cols = self.row_columns(line, k);
                let last_row = k + 1 == count;
                let a = from.clamp(cols.start, cols.end);
                let b = to.clamp(cols.start, cols.end);
                let x0 = self.x_in_row(line, k, a);
                let mut x1 = self.x_in_row(line, k, b);
                if last_row && past_end && newline_cell {
                    x1 += px(NEWLINE_CELL);
                }
                if x1 > x0 {
                    out.push((row, x0, x1));
                }
            }
        }
        out
    }
}
