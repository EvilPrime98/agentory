//! App state: one entity per data domain, reachable everywhere through the [`Stores`] global.

pub mod prefs;
pub mod projects;
pub mod sessions;
pub mod skills;

use gpui::{App, AppContext as _, Entity, Global};

/// Handles to the shared entities. Stored as a gpui `Global` so overlays and background tasks can
/// reach them without threading them through every constructor.
#[derive(Clone)]
pub struct Stores {
    pub prefs: Entity<prefs::PrefsStore>,
    pub projects: Entity<projects::ProjectsStore>,
    pub sessions: Entity<sessions::SessionsStore>,
    pub skills: Entity<skills::SkillsStore>,
}

impl Global for Stores {}

impl Stores {
    pub fn new(cx: &mut App) -> Self {
        Self {
            prefs: cx.new(|_| prefs::PrefsStore::load()),
            projects: cx.new(|_| projects::ProjectsStore::load()),
            sessions: cx.new(sessions::SessionsStore::new),
            skills: cx.new(|_| skills::SkillsStore::default()),
        }
    }
}
