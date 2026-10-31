//! Monochrome SVG icons (`assets/icons/*.svg`). Colour comes from the element's `text_color`,
//! like `currentColor`.

use gpui::{Pixels, Styled, Svg, svg};

macro_rules! icons {
    ($($variant:ident => $file:literal),+ $(,)?) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum Icon {
            $($variant),+
        }

        impl Icon {
            #[cfg(test)]
            pub const ALL: &'static [Icon] = &[$(Icon::$variant),+];

            pub fn path(self) -> &'static str {
                match self {
                    $(Icon::$variant => concat!("icons/", $file, ".svg")),+
                }
            }
        }
    };
}

icons! {
    Branch => "branch",
    Burger => "burger",
    ChevronDown => "chevron-down",
    ChevronRight => "chevron-right",
    Close => "close",
    Folder => "folder",
    FolderPlus => "folder-plus",
    Gear => "gear",
    Heart => "heart",
    Pencil => "pencil",
    Plus => "plus",
    Puzzle => "puzzle",
    Refresh => "refresh",
    Terminal => "terminal",
    Trash => "trash",
    WinMinimize => "win-minimize",
    WinMaximize => "win-maximize",
    WinRestore => "win-restore",
}

/// A square icon of `size` px.
pub fn icon(icon: Icon, size: impl Into<Pixels>) -> Svg {
    let size = size.into();
    svg().path(icon.path()).size(size).flex_none()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assets::Assets;
    use gpui::AssetSource;

    #[test]
    fn every_icon_asset_is_embedded() {
        for i in Icon::ALL {
            assert!(
                Assets.load(i.path()).unwrap().is_some(),
                "missing {}",
                i.path()
            );
        }
    }
}
