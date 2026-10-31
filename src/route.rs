//! Routing. GPUI has no router: an `enum Route` plus a back/forward stack.

use crate::i18n::t;

/// App pages. `Skills` and `Docs` carry the id of the project it manages; `SkillEditor` the project id and
/// the name of the installed skill being edited.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    Sessions,
    Settings,
    Skills(String),
    Docs(String),
    SkillEditor(String, String),
    /// Review and apply the change of the Modify session with this id.
    Review(String),
}

impl Route {
    pub fn home() -> Self {
        Self::Sessions
    }

    /// Page name for the window title (`"<page> · Agentory"`).
    pub fn title(&self) -> String {
        t(match self {
            Self::Sessions => "route.sessions",
            Self::Settings => "settings.title",
            Self::Skills(_) => "skills.title",
            Self::Docs(_) => "docs.title",
            Self::SkillEditor(..) => "skill_editor.title",
            Self::Review(_) => "review.title",
        })
    }
}

pub fn window_title(route: &Route) -> String {
    format!("{} · Agentory", route.title())
}

/// Back/forward stack. Always holds at least one route.
#[derive(Debug, Clone)]
pub struct History {
    stack: Vec<Route>,
    index: usize,
}

impl Default for History {
    fn default() -> Self {
        Self {
            stack: vec![Route::home()],
            index: 0,
        }
    }
}

impl History {
    pub fn current(&self) -> &Route {
        &self.stack[self.index]
    }

    /// Navigate to `route`, dropping any forward entries. No-op when already there.
    pub fn push(&mut self, route: Route) {
        if *self.current() == route {
            return;
        }
        self.stack.truncate(self.index + 1);
        self.stack.push(route);
        self.index += 1;
    }

    pub fn can_back(&self) -> bool {
        self.index > 0
    }

    #[cfg(test)]
    pub fn can_forward(&self) -> bool {
        self.index + 1 < self.stack.len()
    }

    pub fn back(&mut self) -> bool {
        if !self.can_back() {
            return false;
        }
        self.index -= 1;
        true
    }

    #[cfg(test)]
    pub fn forward(&mut self) -> bool {
        if !self.can_forward() {
            return false;
        }
        self.index += 1;
        true
    }

    /// `history.back()` with fallback to `/`.
    pub fn back_or_home(&mut self) {
        if !self.back() {
            self.push(Route::home());
        }
    }
}

/// Emitted by pages and the sidebar; `AppRoot` subscribes and pushes the route.
pub struct Navigate(pub Route);

/// `history.back()` with fallback to `/`.
pub struct GoBack;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_back_forward() {
        let mut h = History::default();
        h.push(Route::Settings);
        assert_eq!(h.current(), &Route::Settings);
        assert!(h.back());
        assert_eq!(h.current(), &Route::Sessions);
        assert!(h.forward());
        assert_eq!(h.current(), &Route::Settings);
    }

    #[test]
    fn push_drops_forward_entries_and_dedupes() {
        let mut h = History::default();
        h.push(Route::Settings);
        h.push(Route::Settings);
        h.back();
        h.push(Route::Settings);
        assert!(!h.can_forward());
        h.back();
        assert_eq!(h.current(), &Route::home());
    }

    #[test]
    fn back_falls_back_home() {
        let mut h = History::default();
        h.back_or_home();
        assert_eq!(h.current(), &Route::home());
        h.push(Route::Settings);
        h.back_or_home();
        assert_eq!(h.current(), &Route::home());
    }
}
