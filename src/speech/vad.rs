use std::path::Path;
use std::time::Duration;

use sherpa_onnx::{SileroVadModelConfig, VadModelConfig, VoiceActivityDetector};

use super::config::VAD_MODEL;
use super::engine::SAMPLE_RATE;

const MAX_SPEECH_SECS: f32 = 25.0;
const MIN_SPEECH_SECS: f32 = 0.25;
const BUFFER_SECS: f32 = 60.0;

pub struct Vad {
    inner: VoiceActivityDetector,
}

impl Vad {
    pub fn new(model_dir: &Path, silence: Duration) -> Result<Self, String> {
        let config = VadModelConfig {
            silero_vad: SileroVadModelConfig {
                model: Some(model_dir.join(VAD_MODEL).to_string_lossy().into_owned()),
                threshold: 0.5,
                min_silence_duration: silence.as_secs_f32(),
                min_speech_duration: MIN_SPEECH_SECS,
                window_size: 512,
                max_speech_duration: MAX_SPEECH_SECS,
            },
            sample_rate: SAMPLE_RATE as i32,
            num_threads: 1,
            provider: Some("cpu".into()),
            ..Default::default()
        };
        VoiceActivityDetector::create(&config, BUFFER_SECS)
            .map(|inner| Self { inner })
            .ok_or_else(|| "cannot load the voice activity model".to_string())
    }

    pub fn accept(&self, samples: &[f32]) {
        self.inner.accept_waveform(samples);
    }

    pub fn speaking(&self) -> bool {
        self.inner.detected()
    }

    pub fn flush(&self) {
        self.inner.flush();
    }

    pub fn next_segment(&self) -> Option<Vec<f32>> {
        let segment = self.inner.front()?;
        let samples = segment.samples().to_vec();
        drop(segment);
        self.inner.pop();
        Some(samples)
    }
}
