//! Design tokens, in a dark and a light variant.
//!
//! Every token is a function that reads the global [`mode`], so call sites stay unchanged. Call
//! [`apply`] when the preference or the OS appearance changes; it re-renders every window.

use std::sync::atomic::{AtomicU8, Ordering};

use gpui::{Pixels, Rgba, WindowAppearance, px, rgb, rgba};
use serde::{Deserialize, Serialize};

/// The concrete palette in use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Dark = 0,
    Light = 1,
}

/// What the user picked in Settings.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ThemePref {
    #[default]
    System,
    Light,
    Dark,
}

impl ThemePref {
    /// Next value when cycling in Settings: System, Light, Dark.
    pub fn next(self) -> Self {
        match self {
            Self::System => Self::Light,
            Self::Light => Self::Dark,
            Self::Dark => Self::System,
        }
    }

    pub fn label_key(self) -> &'static str {
        match self {
            Self::System => "settings.theme.system",
            Self::Light => "settings.theme.light",
            Self::Dark => "settings.theme.dark",
        }
    }

    /// The palette this preference resolves to, given the OS appearance.
    pub fn resolve(self, os: WindowAppearance) -> Mode {
        match self {
            Self::Light => Mode::Light,
            Self::Dark => Mode::Dark,
            Self::System => match os {
                WindowAppearance::Dark | WindowAppearance::VibrantDark => Mode::Dark,
                WindowAppearance::Light | WindowAppearance::VibrantLight => Mode::Light,
            },
        }
    }
}

static MODE: AtomicU8 = AtomicU8::new(Mode::Dark as u8);

pub fn mode() -> Mode {
    if MODE.load(Ordering::Relaxed) == Mode::Light as u8 {
        Mode::Light
    } else {
        Mode::Dark
    }
}

/// Set the palette without re-rendering.
pub fn set_mode(mode: Mode) {
    MODE.store(mode as u8, Ordering::Relaxed);
}

/// Resolve `pref` against the OS appearance, set the palette and re-render every window.
pub fn apply(pref: ThemePref, cx: &mut gpui::App) {
    set_mode(pref.resolve(cx.window_appearance()));
    cx.refresh_windows();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_follows_the_os() {
        assert_eq!(ThemePref::System.resolve(WindowAppearance::Dark), Mode::Dark);
        assert_eq!(ThemePref::System.resolve(WindowAppearance::Light), Mode::Light);
        assert_eq!(ThemePref::Light.resolve(WindowAppearance::Dark), Mode::Light);
        assert_eq!(ThemePref::Dark.resolve(WindowAppearance::Light), Mode::Dark);
    }

    fn luminance(c: Rgba) -> f32 {
        let f = |v: f32| {
            if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * f(c.red) + 0.7152 * f(c.green) + 0.0722 * f(c.blue)
    }

    fn contrast(a: Rgba, b: Rgba) -> f32 {
        let (x, y) = (luminance(a), luminance(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    #[test]
    fn text_meets_wcag_aa_in_both_modes() {
        // One test mutates the global mode, so both modes are checked in sequence.
        for m in [Mode::Dark, Mode::Light] {
            set_mode(m);
            for bg in [bg_app(), bg_chrome(), bg_panel(), bg_terminal(), bg_modal()] {
                for (name, fg) in [
                    ("text", text()),
                    ("text_secondary", text_secondary()),
                    ("text_muted", text_muted()),
                    ("error", error()),
                    ("warning", warning()),
                    ("success", success()),
                ] {
                    let r = contrast(fg, bg);
                    assert!(r >= 4.5, "{name} on {bg:?} in {m:?}: {r:.2}");
                }
            }
        }
        set_mode(Mode::Dark);
    }
}

pub fn accent() -> Rgba {
    rgb(0xd97757)
}

/// Pick the dark or light value for the current [`mode`].
fn pick(dark: Rgba, light: Rgba) -> Rgba {
    match mode() {
        Mode::Dark => dark,
        Mode::Light => light,
    }
}

/// accent at 35 % alpha
pub fn accent_soft() -> Rgba {
    pick(rgba(0xd9775759), rgba(0xd9775740))
}
/// Text colour on top of a solid accent fill.
pub fn on_accent() -> Rgba {
    rgb(0x1f120c)
}
pub fn bg_app() -> Rgba {
    pick(rgb(0x1e1e22), rgb(0xf6f4f0))
}
pub fn bg_chrome() -> Rgba {
    pick(rgb(0x26262b), rgb(0xebe8e2))
}
pub fn bg_panel() -> Rgba {
    pick(rgb(0x202024), rgb(0xfbfaf7))
}
pub fn bg_terminal() -> Rgba {
    pick(rgb(0x15151a), rgb(0xfdfcfa))
}
pub fn bg_modal() -> Rgba {
    pick(rgb(0x1c1c1c), rgb(0xffffff))
}
/// Dim layer behind modals.
pub fn scrim() -> Rgba {
    pick(rgba(0x0000008c), rgba(0x00000052))
}
/// Dark tile behind the logo mark.
pub fn bg_logo() -> Rgba {
    pick(rgb(0x111111), rgb(0x2a2420))
}
/// 8 % wash of the foreground colour.
pub fn border_subtle() -> Rgba {
    pick(rgba(0xffffff14), rgba(0x00000020))
}
/// Hovered border and unchecked checkbox outline.
pub fn border_strong() -> Rgba {
    pick(rgba(0xffffff4d), rgba(0x00000066))
}
pub fn error() -> Rgba {
    pick(rgb(0xff5c5c), rgb(0xb42318))
}
pub fn warning() -> Rgba {
    pick(rgb(0xf2b84b), rgb(0x7a5200))
}
pub fn success() -> Rgba {
    pick(rgb(0x4fc98a), rgb(0x16703f))
}
pub fn text() -> Rgba {
    pick(rgb(0xf2f2f2), rgb(0x1f1f23))
}
/// Between [`text`] and [`text_muted`]: secondary labels and button text.
pub fn text_secondary() -> Rgba {
    pick(rgb(0xd0d0d0), rgb(0x3a3a40))
}
pub fn text_muted() -> Rgba {
    pick(rgb(0xa8a8b0), rgb(0x5a5a62))
}
/// Placeholders and loader captions.
pub fn text_faint() -> Rgba {
    pick(rgb(0x8a8a8a), rgb(0x6f6f76))
}
/// Dot of a session that has exited.
pub fn status_exited() -> Rgba {
    pick(rgb(0x55555c), rgb(0xa0a0a8))
}
/// Hover wash used on header/window buttons.
pub fn hover() -> Rgba {
    pick(rgba(0xffffff14), rgba(0x0000000f))
}
/// Lighter hover wash for ghost buttons.
pub fn hover_soft() -> Rgba {
    pick(rgba(0xffffff0f), rgba(0x0000000a))
}
/// Pressed wash for ghost buttons.
pub fn pressed() -> Rgba {
    pick(rgba(0xffffff1a), rgba(0x00000018))
}
/// Hovered fill of a secondary button.
pub fn btn_hover() -> Rgba {
    pick(rgb(0x2a2a2a), rgb(0xefece6))
}
/// Pressed fill of a secondary button.
pub fn btn_active() -> Rgba {
    pick(rgb(0x262626), rgb(0xe6e2da))
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
