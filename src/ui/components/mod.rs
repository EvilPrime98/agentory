pub mod button;
pub mod checkbox;
pub mod markdown;
pub mod preview;
pub mod scrollbar;
pub mod selectable;
pub mod segmented;
pub mod smooth_scroll;
pub mod text_area;
pub mod text_input;

pub fn is_altgr(ks: &gpui::Keystroke) -> bool {
    ks.modifiers.control
        && ks.modifiers.alt
        && ks
            .key_char
            .as_deref()
            .is_some_and(|c| c != ks.key && !c.is_empty() && !c.chars().any(char::is_control))
}
