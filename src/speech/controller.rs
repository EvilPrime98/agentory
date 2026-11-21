
use std::time::Duration;

use futures_util::StreamExt as _;
use gpui::{Context, Entity};

use super::config::PipelineConfig;
use super::events::SpeechEvent;
use super::model;
use super::pipeline::{self, PipelineHandle};
use crate::i18n::t;
use crate::runtime;
use crate::state::prefs::PrefsStore;
use crate::state::sessions::SessionsStore;
use crate::ui::terminal::Terminal;
use crate::ui::toast;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Loading,
    Listening,
    Speaking,
    Transcribing,
    Sending,
}

pub struct SpeechController {
    sessions: Entity<SessionsStore>,
    prefs: Entity<PrefsStore>,
    pub installed: bool,
    pub download: Option<u64>,
    pub error: Option<String>,
    session: Option<String>,
    pipeline: Option<PipelineHandle>,
    generation: u64,
    loaded: bool,
    speaking: bool,
    finishing: bool,
    pending: usize,
    warned_overflow: bool,
    enter_gen: u64,
    enter_armed: bool,
}

impl SpeechController {
    pub fn new(
        sessions: Entity<SessionsStore>,
        prefs: Entity<PrefsStore>,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&sessions, |this, sessions, cx| {
            let gone = this
                .session
                .as_deref()
                .is_some_and(|id| sessions.read(cx).get(id).is_none());
            if gone {
                this.cancel(cx);
            }
        })
        .detach();
        cx.observe(&prefs, |this, prefs, cx| {
            if !prefs.read(cx).prefs.speech_enabled && this.pipeline.is_some() {
                this.cancel(cx);
            }
        })
        .detach();
        Self {
            sessions,
            prefs,
            installed: model::is_installed(),
            download: None,
            error: None,
            session: None,
            pipeline: None,
            generation: 0,
            loaded: false,
            speaking: false,
            finishing: false,
            pending: 0,
            warned_overflow: false,
            enter_gen: 0,
            enter_armed: false,
        }
    }

    pub fn phase(&self) -> Phase {
        if self.pipeline.is_none() {
            Phase::Idle
        } else if !self.loaded {
            Phase::Loading
        } else if self.speaking {
            Phase::Speaking
        } else if self.pending > 0 || self.finishing {
            Phase::Transcribing
        } else if self.enter_armed {
            Phase::Sending
        } else {
            Phase::Listening
        }
    }

    pub fn session(&self) -> Option<&str> {
        self.session.as_deref()
    }

    pub fn is_listening_on(&self, session: &str) -> bool {
        self.pipeline.is_some() && !self.finishing && self.session.as_deref() == Some(session)
    }

    pub fn toggle(&mut self, session: &str, cx: &mut Context<Self>) {
        if !self.prefs.read(cx).prefs.speech_enabled {
            return;
        }
        if self.is_listening_on(session) {
            self.stop(cx);
        } else {
            self.cancel(cx);
            self.start(session, cx);
        }
    }

    fn start(&mut self, session: &str, cx: &mut Context<Self>) {
        self.error = None;
        if !super::AVAILABLE {
            return self.fail(t("speech.unavailable"), cx);
        }
        let Some(dir) = model::dir().filter(|_| model::is_installed()) else {
            self.installed = false;
            return self.fail(t("speech.model_missing"), cx);
        };
        let config = PipelineConfig::from_prefs(&self.prefs.read(cx).prefs, dir);
        let (tx, mut rx) = futures_channel::mpsc::unbounded();
        self.generation += 1;
        let generation = self.generation;
        self.pipeline = Some(pipeline::spawn(config, tx));
        self.session = Some(session.to_string());
        self.loaded = false;
        self.speaking = false;
        self.finishing = false;
        self.pending = 0;
        self.warned_overflow = false;
        cx.notify();
        cx.spawn(async move |this, cx| {
            while let Some(event) = rx.next().await {
                match this.update(cx, |this, cx| this.on_event(generation, event, cx)) {
                    Ok(true) => {}
                    _ => break,
                }
            }
        })
        .detach();
    }

    pub fn stop(&mut self, cx: &mut Context<Self>) {
        if let Some(p) = &self.pipeline {
            p.stop();
            self.disarm_enter();
            self.finishing = true;
            cx.notify();
        }
    }

    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        if self.pipeline.is_none() {
            return;
        }
        self.generation += 1;
        self.reset();
        cx.notify();
    }

    fn reset(&mut self) {
        self.pipeline = None;
        self.session = None;
        self.loaded = false;
        self.speaking = false;
        self.finishing = false;
        self.pending = 0;
        self.disarm_enter();
    }

    fn disarm_enter(&mut self) {
        self.enter_gen += 1;
        self.enter_armed = false;
    }

    fn fail(&mut self, message: String, cx: &mut Context<Self>) {
        toast::error(cx, message.clone());
        self.error = Some(message);
        cx.notify();
    }

    fn on_event(&mut self, generation: u64, event: SpeechEvent, cx: &mut Context<Self>) -> bool {
        if generation != self.generation {
            return false;
        }
        match event {
            SpeechEvent::Listening => self.loaded = true,
            SpeechEvent::SpeechStarted => {
                self.speaking = true;
                self.disarm_enter();
            }
            SpeechEvent::SpeechEnded => self.speaking = false,
            SpeechEvent::Transcribing => self.pending += 1,
            SpeechEvent::Final(text) => {
                self.pending = self.pending.saturating_sub(1);
                if self.paste(&text, cx) && !self.finishing {
                    self.arm_enter(cx);
                }
            }
            SpeechEvent::NoSpeech => self.pending = self.pending.saturating_sub(1),
            SpeechEvent::AudioDropped => {
                tracing::warn!("speech: dropped audio, recognition is falling behind");
                if !self.warned_overflow {
                    self.warned_overflow = true;
                    toast::error(cx, t("speech.dropped"));
                }
            }
            SpeechEvent::Error(message) => self.fail(message, cx),
            SpeechEvent::Stopped => {
                self.reset();
                cx.notify();
                return false;
            }
        }
        cx.notify();
        true
    }

    fn terminal(&self, cx: &Context<Self>) -> Option<Entity<Terminal>> {
        self.session
            .as_deref()
            .and_then(|id| self.sessions.read(cx).get(id))
            .map(|s| s.terminal.clone())
    }

    fn paste(&mut self, text: &str, cx: &mut Context<Self>) -> bool {
        if text.trim().is_empty() {
            return false;
        }
        let Some(terminal) = self.terminal(cx) else {
            return false;
        };
        terminal.update(cx, |t, _| t.paste(&format!("{text} ")));
        true
    }

    fn arm_enter(&mut self, cx: &mut Context<Self>) {
        let prefs = &self.prefs.read(cx).prefs;
        if !prefs.speech_auto_enter {
            return;
        }
        let delay = Duration::from_millis(u64::from(prefs.speech_auto_enter_ms));
        let Some(terminal) = self.terminal(cx) else {
            return;
        };
        let typed = terminal.read(cx).key_input_count;
        self.enter_gen += 1;
        self.enter_armed = true;
        let token = self.enter_gen;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            this.update(cx, |this, cx| {
                if this.enter_gen != token {
                    return;
                }
                this.enter_armed = false;
                let idle = !this.speaking && this.pending == 0 && !this.finishing;
                if let Some(terminal) = this.terminal(cx).filter(|_| idle) {
                    terminal.update(cx, |t, _| {
                        if t.key_input_count == typed {
                            t.send(b"\r");
                        }
                    });
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn download_model(&mut self, cx: &mut Context<Self>) {
        if self.download.is_some() {
            return;
        }
        self.download = Some(0);
        self.error = None;
        cx.notify();
        let (tx, mut rx) = futures_channel::mpsc::unbounded::<u64>();
        cx.spawn({
            async move |this, cx| {
                let result = runtime::run(model::download(tx)).await;
                this.update(cx, |this, cx| {
                    this.download = None;
                    this.installed = model::is_installed();
                    match result {
                        Ok(()) => toast::success(cx, t("speech.model_ready")),
                        Err(e) => this.fail(e, cx),
                    }
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
        cx.spawn(async move |this, cx| {
            while let Some(bytes) = rx.next().await {
                let alive = this.update(cx, |this, cx| {
                    if this.download.is_some() {
                        this.download = Some(bytes);
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
    }
}
