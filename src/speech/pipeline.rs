use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;

use futures_channel::mpsc::UnboundedSender;

use super::config::PipelineConfig;
use super::engine::SpeechEngine;
use super::events::SpeechEvent;

pub type Events = UnboundedSender<SpeechEvent>;

pub struct PipelineHandle {
    stop: Arc<AtomicBool>,
}

impl PipelineHandle {
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

impl Drop for PipelineHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

pub fn spawn(config: PipelineConfig, events: Events) -> PipelineHandle {
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    let thread_events = events.clone();
    let spawned = std::thread::Builder::new()
        .name("agentory-speech".into())
        .spawn(move || {
            if let Err(e) = run(config, &thread_events, &flag) {
                tracing::warn!("speech pipeline failed: {e}");
                let _ = thread_events.unbounded_send(SpeechEvent::Error(e));
            }
            let _ = thread_events.unbounded_send(SpeechEvent::Stopped);
        });
    if let Err(e) = spawned {
        let _ = events.unbounded_send(SpeechEvent::Error(e.to_string()));
        let _ = events.unbounded_send(SpeechEvent::Stopped);
    }
    PipelineHandle { stop }
}

#[cfg(not(feature = "speech"))]
fn run(_: PipelineConfig, _: &Events, _: &Arc<AtomicBool>) -> Result<(), String> {
    Err("this build does not include speech recognition".into())
}

#[cfg(feature = "speech")]
fn run(config: PipelineConfig, events: &Events, stop: &Arc<AtomicBool>) -> Result<(), String> {
    use std::sync::atomic::AtomicU64;
    use std::sync::mpsc::{self, RecvTimeoutError};
    use std::time::Duration;

    use sherpa_onnx::LinearResampler;

    use super::engine::SAMPLE_RATE;
    use super::sherpa::WhisperEngine;
    use super::vad::Vad;
    use crate::platform::audio;

    const CHUNK_QUEUE: usize = 1024;

    let (seg_tx, seg_rx) = mpsc::channel::<Vec<f32>>();
    let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
    let decoder = {
        let events = events.clone();
        let dir = config.model_dir.clone();
        let language = config.language.clone();
        std::thread::Builder::new()
            .name("agentory-stt".into())
            .spawn(move || {
                let mut engine = match WhisperEngine::new(&dir, language.as_deref()) {
                    Ok(engine) => {
                        let _ = ready_tx.send(Ok(()));
                        engine
                    }
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                        return;
                    }
                };
                decode_loop(&mut engine, &seg_rx, &events);
            })
            .map_err(|e| e.to_string())?
    };
    ready_rx
        .recv()
        .map_err(|_| "the speech worker stopped unexpectedly".to_string())??;
    if stop.load(Ordering::SeqCst) {
        drop(seg_tx);
        let _ = decoder.join();
        return Ok(());
    }

    let vad = Vad::new(&config.model_dir, config.silence)?;
    let (tx, rx) = mpsc::sync_channel::<Vec<f32>>(CHUNK_QUEUE);
    let dropped = Arc::new(AtomicU64::new(0));
    let on_error: audio::ErrorSink = {
        let events = events.clone();
        let stop = stop.clone();
        Arc::new(move |e| {
            let _ = events.unbounded_send(SpeechEvent::Error(format!("Microphone: {e}")));
            stop.store(true, Ordering::SeqCst);
        })
    };
    let capture = audio::start(&config.device, tx, dropped.clone(), on_error)?;
    let resampler = if capture.sample_rate == SAMPLE_RATE {
        None
    } else {
        Some(
            LinearResampler::create(capture.sample_rate as i32, SAMPLE_RATE as i32)
                .ok_or("cannot create the audio resampler")?,
        )
    };

    let mut segmenter = Segmenter {
        vad,
        speaking: false,
        segments: seg_tx,
        events: events.clone(),
    };
    let feed = |segmenter: &mut Segmenter, chunk: Vec<f32>| {
        let chunk = match &resampler {
            Some(r) => r.resample(&chunk, false),
            None => chunk,
        };
        segmenter.feed(&chunk);
    };

    let _ = events.unbounded_send(SpeechEvent::Listening);
    loop {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(chunk) => feed(&mut segmenter, chunk),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        if dropped.swap(0, Ordering::Relaxed) > 0 {
            let _ = events.unbounded_send(SpeechEvent::AudioDropped);
        }
        if stop.load(Ordering::SeqCst) {
            break;
        }
    }
    drop(capture);
    while let Ok(chunk) = rx.try_recv() {
        feed(&mut segmenter, chunk);
    }
    segmenter.finish();
    drop(segmenter);
    let _ = decoder.join();
    Ok(())
}

#[cfg(feature = "speech")]
struct Segmenter {
    vad: super::vad::Vad,
    speaking: bool,
    segments: std::sync::mpsc::Sender<Vec<f32>>,
    events: Events,
}

#[cfg(feature = "speech")]
impl Segmenter {
    fn feed(&mut self, samples: &[f32]) {
        self.vad.accept(samples);
        self.update_speaking();
        self.drain();
    }

    fn finish(&mut self) {
        self.vad.flush();
        self.update_speaking();
        self.drain();
    }

    fn update_speaking(&mut self) {
        let speaking = self.vad.speaking();
        if speaking != self.speaking {
            self.speaking = speaking;
            let _ = self.events.unbounded_send(if speaking {
                SpeechEvent::SpeechStarted
            } else {
                SpeechEvent::SpeechEnded
            });
        }
    }

    fn drain(&mut self) {
        while let Some(samples) = self.vad.next_segment() {
            let _ = self.events.unbounded_send(SpeechEvent::Transcribing);
            let _ = self.segments.send(samples);
        }
    }
}

fn decode_loop(engine: &mut dyn SpeechEngine, queue: &Receiver<Vec<f32>>, events: &Events) {
    while let Ok(samples) = queue.recv() {
        let event = match engine.transcribe(&samples) {
            Ok(text) if usable(&text) => SpeechEvent::Final(text.trim().to_string()),
            Ok(_) => SpeechEvent::NoSpeech,
            Err(e) => {
                let _ = events.unbounded_send(SpeechEvent::Error(e));
                SpeechEvent::NoSpeech
            }
        };
        let _ = events.unbounded_send(event);
    }
}

fn usable(text: &str) -> bool {
    let t = text.trim();
    let marker =
        (t.starts_with('[') && t.ends_with(']')) || (t.starts_with('(') && t.ends_with(')'));
    !t.is_empty() && !marker
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_channel::mpsc::unbounded;
    use std::sync::mpsc;

    struct Fake;

    impl SpeechEngine for Fake {
        fn transcribe(&mut self, samples: &[f32]) -> Result<String, String> {
            match samples.len() {
                1 => Ok(" hello world ".into()),
                2 => Ok("[BLANK_AUDIO]".into()),
                _ => Err("boom".into()),
            }
        }
    }

    #[test]
    fn markers_and_blanks_are_not_usable() {
        assert!(usable("hola"));
        assert!(!usable("  "));
        assert!(!usable("[BLANK_AUDIO]"));
        assert!(!usable("(music)"));
        assert!(usable("(a) then more"));
    }

    #[test]
    fn decode_loop_reports_each_utterance_in_order() {
        let (seg_tx, seg_rx) = mpsc::channel();
        let (tx, mut rx) = unbounded();
        seg_tx.send(vec![0.0]).unwrap();
        seg_tx.send(vec![0.0; 2]).unwrap();
        seg_tx.send(vec![0.0; 3]).unwrap();
        drop(seg_tx);
        decode_loop(&mut Fake, &seg_rx, &tx);
        drop(tx);
        let mut got = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            got.push(ev);
        }
        assert_eq!(
            got,
            vec![
                SpeechEvent::Final("hello world".into()),
                SpeechEvent::NoSpeech,
                SpeechEvent::Error("boom".into()),
                SpeechEvent::NoSpeech,
            ]
        );
    }
}
