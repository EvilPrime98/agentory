//! App state: one entity per data domain, reachable everywhere through the [`Stores`] global.

pub mod git;
pub mod prefs;
pub mod projects;
pub mod review;
pub mod selfbuild;
pub mod sessions;
pub mod skills;
pub mod usage;
pub mod workflows;

use gpui::{App, AppContext as _, Entity, Global};

/// Handles to the shared entities. Stored as a gpui `Global` so overlays and background tasks can
/// reach them without threading them through every constructor.
#[derive(Clone)]
pub struct Stores {
    pub git: Entity<git::GitStore>,
    pub prefs: Entity<prefs::PrefsStore>,
    pub projects: Entity<projects::ProjectsStore>,
    pub review: Entity<review::ReviewStore>,
    pub selfbuild: Entity<selfbuild::SelfBuildStore>,
    pub sessions: Entity<sessions::SessionsStore>,
    pub skills: Entity<skills::SkillsStore>,
    pub usage: Entity<usage::UsageStore>,
    pub workflows: Entity<workflows::WorkflowsStore>,
}

impl Global for Stores {}

impl Stores {
    pub fn new(cx: &mut App) -> Self {
        let prefs = cx.new(|_| prefs::PrefsStore::load());
        // Language follows the preference; changing it re-renders every window.
        let initial = prefs.read(cx).prefs.language;
        crate::i18n::set(initial);
        let mut last_language = initial;
        cx.observe(&prefs, move |prefs, cx| {
            let language = prefs.read(cx).prefs.language;
            if language != last_language {
                last_language = language;
                crate::i18n::apply(language, cx);
            }
        })
        .detach();
        // Same for the theme; System also follows OS appearance changes (see `AppRoot::new`).
        let initial_theme = prefs.read(cx).prefs.theme;
        crate::ui::theme::set_mode(initial_theme.resolve(cx.window_appearance()));
        let mut last_theme = initial_theme;
        cx.observe(&prefs, move |prefs, cx| {
            let theme = prefs.read(cx).prefs.theme;
            if theme != last_theme {
                last_theme = theme;
                crate::ui::theme::apply(theme, cx);
            }
        })
        .detach();
        let sessions = cx.new(sessions::SessionsStore::new);
        // Terminals follow the font size preference whichever page is mounted.
        let sessions_for_font = sessions.clone();
        cx.observe(&prefs, move |prefs, cx| {
            let size = prefs.read(cx).prefs.terminal_font_size;
            sessions_for_font.update(cx, |s, cx| s.set_font_size(size, cx));
        })
        .detach();
        let usage = cx.new(|cx| usage::UsageStore::new(sessions.clone(), cx));
        let selfbuild = cx.new(|cx| selfbuild::SelfBuildStore::new(prefs.clone(), cx));
        Self {
            git: cx.new(|_| git::GitStore::default()),
            prefs,
            selfbuild,
            review: cx.new(|_| review::ReviewStore::default()),
            projects: cx.new(|_| projects::ProjectsStore::load()),
            sessions,
            skills: cx.new(|_| skills::SkillsStore::default()),
            usage,
            workflows: cx.new(|_| workflows::WorkflowsStore::default()),
        }
    }
}
