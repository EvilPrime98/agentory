//! Pixel art of the agent office: the sprite sheet (as character grids, one palette) and the
//! drawing of one desk cell. Everything paints on the single canvas of the office page.

use gpui::{Bounds, Pixels, Point, Rgba, Window, fill, point, px, rgb, size};

use crate::model::SessionStatus;
use crate::ui::theme;

/// Size of one art pixel on screen.
const S: f32 = 4.0;
pub const CELL_W: f32 = 160.0;
pub const CELL_H: f32 = 168.0;
/// Where the label under the desk starts, inside a cell.
pub const LABEL_Y: f32 = 128.0;

/// What a worker is doing, derived from the session status and the tool call in progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Waiting for the next prompt.
    Idle,
    /// Editing or writing files.
    Type,
    /// Reading files.
    Read,
    /// Running a shell command.
    Terminal,
    /// Searching code or the web.
    Search,
    /// Working with no tool in flight (thinking, or a tool that has no animation of its own).
    Think,
    /// Waiting for the user.
    Wait,
    /// The session exited.
    Gone,
}

impl Action {
    /// Pose and animation for a status and the name of the tool call in progress.
    pub fn of(status: SessionStatus, tool: Option<&str>) -> Self {
        match status {
            SessionStatus::Starting | SessionStatus::Idle => Self::Idle,
            SessionStatus::NeedsInput => Self::Wait,
            SessionStatus::Exited => Self::Gone,
            SessionStatus::Working => match tool {
                Some("Edit" | "Write" | "MultiEdit" | "NotebookEdit") => Self::Type,
                Some("Read") => Self::Read,
                Some("Bash" | "PowerShell" | "BashOutput") => Self::Terminal,
                Some("Grep" | "Glob" | "WebSearch" | "WebFetch") => Self::Search,
                _ => Self::Think,
            },
        }
    }
}

/// What the scene needs to draw one worker.
pub struct WorkerView {
    pub action: Action,
    /// Context window fill, 0–100.
    pub context_pct: f32,
}

const HEAD: [&str; 7] = [
    "..hhhhhh..",
    ".hhhhhhhh.",
    ".hssssssh.",
    ".sskssksh.",
    ".ssssssss.",
    "..ssmmss..",
    "...ssss...",
];

const TORSO: [&str; 5] = [
    ".bbbbbbbb.",
    "bbbbbbbbbb",
    "bbbbbbbbbb",
    "bbbbbbbbbb",
    "bbbbbbbbbb",
];

const BUBBLE_BANG: [&str; 7] = [
    "wwwwwww", "wwwkwww", "wwwkwww", "wwwkwww", "wwwwwww", "wwwkwww", "wwwwwww",
];

const SHIRTS: [u32; 6] = [0x4f7fd1, 0xd1704f, 0x5fae74, 0xb065c9, 0xd1b24f, 0x4fb6c4];
const HAIRS: [u32; 4] = [0x3b2a20, 0x1f1f24, 0x8a5a2b, 0xc9a24a];

fn rect(window: &mut Window, o: Point<Pixels>, x: f32, y: f32, w: f32, h: f32, c: Rgba) {
    window.paint_quad(fill(
        Bounds::new(point(o.x + px(x), o.y + px(y)), size(px(w), px(h))),
        c,
    ));
}

/// Draws a character grid, merging horizontal runs of one colour into a single quad.
fn sprite(
    window: &mut Window,
    o: Point<Pixels>,
    x: f32,
    y: f32,
    rows: &[&str],
    color: impl Fn(char) -> Option<Rgba>,
) {
    for (ry, row) in rows.iter().enumerate() {
        let cells: Vec<char> = row.chars().collect();
        let mut i = 0;
        while i < cells.len() {
            let ch = cells[i];
            let mut j = i + 1;
            while j < cells.len() && cells[j] == ch {
                j += 1;
            }
            if let Some(c) = color(ch) {
                rect(
                    window,
                    o,
                    x + i as f32 * S,
                    y + ry as f32 * S,
                    (j - i) as f32 * S,
                    S,
                    c,
                );
            }
            i = j;
        }
    }
}

fn wx_chair() -> f32 {
    20.0
}

fn tint_rgba(c: u32, a: f32) -> Rgba {
    let mut r = rgb(c);
    r.alpha = a;
    r
}

fn grayed(c: u32) -> u32 {
    let l = ((c >> 16 & 0xff) + (c >> 8 & 0xff) + (c & 0xff)) / 3;
    (l << 16) | (l << 8) | l
}

pub struct Placed {
    pub x: f32,
    pub y: f32,
    pub slot: usize,
    pub view: WorkerView,
}

/// `frame` is the fixed-timestep animation clock.
pub fn paint_scene(
    window: &mut Window,
    o: Point<Pixels>,
    width: f32,
    height: f32,
    desks: &[Placed],
    frame: u64,
) {
    rect(window, o, 0.0, 0.0, width, height, theme::bg_terminal());
    // Faint floor tiles so the room reads as a floor, not a flat panel.
    let (tw, th) = (CELL_W / 4.0, CELL_H / 4.0);
    for r in 0..(height / th).round() as usize {
        for c in 0..(width / tw).round() as usize {
            if (r + c) % 2 == 0 {
                rect(
                    window,
                    o,
                    c as f32 * tw,
                    r as f32 * th,
                    tw,
                    th,
                    tint_rgba(0xffffff, 0.02),
                );
            }
        }
    }
    for d in desks {
        let cell = point(o.x + px(d.x), o.y + px(d.y));
        paint_desk(window, cell, d.slot, &d.view, frame);
    }
}

fn paint_desk(window: &mut Window, o: Point<Pixels>, slot: usize, w: &WorkerView, frame: u64) {
    let gone = w.action == Action::Gone;
    let tint = |c: u32| rgb(if gone { grayed(c) } else { c });

    // Booth: border, back wall, and a rug under the desk.
    rect(
        window,
        o,
        8.0,
        8.0,
        CELL_W - 16.0,
        112.0,
        theme::border_subtle(),
    );
    rect(window, o, 9.0, 9.0, CELL_W - 18.0, 110.0, theme::bg_panel());
    rect(
        window,
        o,
        9.0,
        9.0,
        CELL_W - 18.0,
        4.0,
        theme::border_subtle(),
    );
    rect(
        window,
        o,
        20.0,
        108.0,
        CELL_W - 40.0,
        8.0,
        tint_rgba(0x000000, 0.18),
    );
    // Chair back behind the worker.
    rect(window, o, wx_chair(), 58.0, 48.0, 30.0, tint(0x3a3a46));

    let shirt = SHIRTS[slot % SHIRTS.len()];
    let hair = HAIRS[slot % HAIRS.len()];
    let bob = if matches!(w.action, Action::Idle) && frame / 8 % 2 == 1 {
        S
    } else {
        0.0
    };
    let (wx, wy) = (22.0, 40.0 + bob);
    let palette = |ch: char| match ch {
        'h' => Some(tint(hair)),
        's' => Some(tint(0xe8b98f)),
        'k' => Some(tint(0x2a2024)),
        'm' => Some(tint(0xb5604f)),
        'b' => Some(tint(shirt)),
        'w' => Some(rgb(0xf4f4f0)),
        _ => None,
    };
    sprite(window, o, wx, wy, &HEAD, palette);
    sprite(window, o, wx, wy + 7.0 * S, &TORSO, palette);

    // Desk top and front edge, over the torso's lower end.
    rect(window, o, 16.0, 84.0, 128.0, 20.0, tint(0x9a6f45));
    rect(window, o, 16.0, 104.0, 128.0, 8.0, tint(0x6e4c2e));
    rect(window, o, 16.0, 84.0, 128.0, 2.0, tint(0xb88a5a));
    // Keyboard and mug.
    rect(window, o, 36.0, 92.0, 36.0, 6.0, tint(0x3a3a46));
    rect(window, o, 38.0, 93.0, 32.0, 2.0, tint(0x5a5a68));
    rect(window, o, 124.0, 72.0, 10.0, 12.0, tint(0xe8e4dc));
    rect(window, o, 134.0, 75.0, 3.0, 6.0, tint(0xe8e4dc));

    // Hands on the desk: they alternate while typing, and one is raised while waiting.
    let hand = tint(0xe8b98f);
    let step = (frame / 2 % 2) as f32;
    match w.action {
        Action::Type | Action::Terminal => {
            rect(window, o, 38.0 + step * S, 80.0 - step * S, 8.0, 8.0, hand);
            rect(
                window,
                o,
                54.0 + (1.0 - step) * S,
                80.0 - (1.0 - step) * S,
                8.0,
                8.0,
                hand,
            );
        }
        Action::Wait => {
            rect(window, o, 38.0, 80.0, 8.0, 8.0, hand);
            let wave = if frame / 3 % 2 == 0 { 0.0 } else { S };
            rect(window, o, 66.0 + wave, 44.0, 8.0, 8.0, hand);
            rect(window, o, 66.0 + wave, 52.0, 8.0, 32.0, tint(shirt));
        }
        _ => {
            rect(window, o, 40.0, 80.0, 8.0, 8.0, hand);
            rect(window, o, 56.0, 80.0, 8.0, 8.0, hand);
        }
    }

    paint_monitor(window, o, w.action, frame);

    match w.action {
        Action::Wait => sprite(window, o, 100.0, 16.0, &BUBBLE_BANG, |c| match c {
            'w' => Some(rgb(0xf4f4f0)),
            'k' => Some(rgb(0xd1704f)),
            _ => None,
        }),
        Action::Think => {
            rect(window, o, 96.0, 14.0, 36.0, 24.0, rgb(0xf4f4f0));
            let lit = (frame / 4 % 4) as usize;
            for d in 0..3usize {
                let c = if d < lit { 0x2a2024 } else { 0xb0b0aa };
                rect(window, o, 102.0 + d as f32 * 10.0, 24.0, 6.0, 6.0, rgb(c));
            }
        }
        _ => {}
    }

    // Context meter.
    let track = CELL_W - 32.0;
    rect(window, o, 16.0, 116.0, track, 6.0, theme::border_subtle());
    let pct = w.context_pct.clamp(0.0, 100.0);
    let color = if pct >= 85.0 {
        theme::error()
    } else if pct >= 60.0 {
        theme::warning()
    } else {
        theme::success()
    };
    rect(window, o, 16.0, 116.0, track * pct / 100.0, 6.0, color);
}

fn paint_monitor(window: &mut Window, o: Point<Pixels>, action: Action, frame: u64) {
    let (mx, my) = (88.0, 44.0);
    rect(window, o, mx, my, 44.0, 36.0, rgb(0x2b2b33));
    rect(window, o, mx + 14.0, my + 36.0, 16.0, 4.0, rgb(0x2b2b33));
    let (sx, sy, sw, sh) = (mx + 4.0, my + 4.0, 36.0, 28.0);
    let bg = match action {
        Action::Type => 0x16241b,
        Action::Read => 0x1b1d2a,
        Action::Terminal => 0x0d0d10,
        Action::Search => 0x14263a,
        Action::Think => 0x1d1d24,
        Action::Wait => 0x4a3d12,
        Action::Idle => 0x22222a,
        Action::Gone => 0x18181c,
    };
    rect(window, o, sx, sy, sw, sh, rgb(bg));
    let n = (frame / 3) as usize;
    match action {
        Action::Type => {
            let shown = n % 6 + 1;
            for l in 0..shown {
                let len = 8.0 + ((l * 7 + 3) % 5) as f32 * 4.0;
                rect(
                    window,
                    o,
                    sx + 4.0,
                    sy + 3.0 + l as f32 * 4.0,
                    len,
                    2.0,
                    rgb(0x6fd08c),
                );
            }
        }
        Action::Read => {
            for l in 0..6usize {
                let len = 10.0 + ((l + n) * 5 % 4) as f32 * 5.0;
                rect(
                    window,
                    o,
                    sx + 4.0,
                    sy + 3.0 + l as f32 * 4.0,
                    len,
                    2.0,
                    rgb(0xd8dbe8),
                );
            }
        }
        Action::Terminal => {
            rect(window, o, sx + 4.0, sy + 4.0, 4.0, 2.0, rgb(0x6fd08c));
            rect(window, o, sx + 10.0, sy + 4.0, 14.0, 2.0, rgb(0xd8dbe8));
            if n % 2 == 0 {
                rect(window, o, sx + 4.0, sy + 12.0, 4.0, 4.0, rgb(0x6fd08c));
            }
        }
        Action::Search => {
            let x = sx + 6.0 + (n % 6) as f32 * 3.0;
            rect(window, o, x, sy + 6.0, 10.0, 10.0, rgb(0x7fb6ee));
            rect(window, o, x + 2.0, sy + 8.0, 6.0, 6.0, rgb(bg));
            rect(window, o, x + 8.0, sy + 16.0, 4.0, 4.0, rgb(0x7fb6ee));
        }
        Action::Wait => {
            rect(window, o, sx + 16.0, sy + 5.0, 4.0, 12.0, rgb(0xe8c24a));
            rect(window, o, sx + 16.0, sy + 20.0, 4.0, 4.0, rgb(0xe8c24a));
        }
        Action::Idle => rect(window, o, sx + 4.0, sy + 3.0, 12.0, 2.0, rgb(0x4a4a56)),
        Action::Think | Action::Gone => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_picks_the_action_only_while_working() {
        assert_eq!(
            Action::of(SessionStatus::Working, Some("Edit")),
            Action::Type
        );
        assert_eq!(
            Action::of(SessionStatus::Working, Some("Bash")),
            Action::Terminal
        );
        assert_eq!(
            Action::of(SessionStatus::Working, Some("Grep")),
            Action::Search
        );
        assert_eq!(
            Action::of(SessionStatus::Working, Some("Read")),
            Action::Read
        );
        assert_eq!(Action::of(SessionStatus::Working, None), Action::Think);
        assert_eq!(Action::of(SessionStatus::Idle, Some("Edit")), Action::Idle);
        assert_eq!(Action::of(SessionStatus::NeedsInput, None), Action::Wait);
        assert_eq!(
            Action::of(SessionStatus::Exited, Some("Edit")),
            Action::Gone
        );
    }

    #[test]
    fn sprites_are_rectangular() {
        for rows in [&HEAD[..], &TORSO[..]] {
            assert!(rows.iter().all(|r| r.len() == rows[0].len()));
        }
        assert!(BUBBLE_BANG.iter().all(|r| r.len() == BUBBLE_BANG[0].len()));
    }
}
