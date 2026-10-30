//! Design tokens. Dark theme only.

use gpui::{Pixels, Rgba, px, rgb, rgba};

pub fn accent() -> Rgba {
    rgb(0xd97757)
}
/// accent at 35 % alpha
pub fn accent_soft() -> Rgba {
    rgba(0xd9775759)
}
/// Text colour on top of a solid accent fill.
pub fn on_accent() -> Rgba {
    rgb(0x1f120c)
}
pub fn bg_app() -> Rgba {
    rgb(0x1e1e22)
}
pub fn bg_chrome() -> Rgba {
    rgb(0x26262b)
}
pub fn bg_panel() -> Rgba {
    rgb(0x202024)
}
pub fn bg_terminal() -> Rgba {
    rgb(0x15151a)
}
pub fn bg_modal() -> Rgba {
    rgb(0x1c1c1c)
}
/// rgba(255,255,255,0.08)
pub fn border_subtle() -> Rgba {
    rgba(0xffffff14)
}
pub fn error() -> Rgba {
    rgb(0xff5c5c)
}
pub fn warning() -> Rgba {
    rgb(0xf2b84b)
}
pub fn success() -> Rgba {
    rgb(0x4fc98a)
}
pub fn text() -> Rgba {
    rgb(0xf2f2f2)
}
pub fn text_muted() -> Rgba {
    rgb(0xa8a8b0)
}
/// Hover wash used on header/window buttons.
pub fn hover() -> Rgba {
    rgba(0xffffff14)
}

pub fn header_height() -> Pixels {
    px(48.0)
}
pub fn sidebar_width() -> Pixels {
    px(260.0)
}

/// Font for the whole app, UI and terminal. Embedded under `assets/fonts` so it does not depend
/// on the system (Cascadia Mono only ships with Windows 11).
pub const FONT: &str = "Cascadia Mono";

/// Register the font files embedded under `assets/fonts`. A missing or unreadable file only costs
/// that face and is logged, not fatal.
pub fn register_fonts(cx: &mut gpui::App) {
    use crate::assets::Assets;
    use gpui::AssetSource as _;
    let Ok(paths) = Assets.list("fonts/") else {
        return;
    };
    let fonts: Vec<_> = paths
        .iter()
        .filter(|p| p.ends_with(".ttf") || p.ends_with(".otf"))
        .filter_map(|p| match Assets.load(p) {
            Ok(Some(data)) => Some(data),
            _ => {
                tracing::warn!("embedded font {p} is missing");
                None
            }
        })
        .collect();
    if fonts.is_empty() {
        return;
    }
    if let Err(e) = cx.text_system().add_fonts(fonts) {
        tracing::warn!("could not register the embedded fonts: {e}");
    }
}
