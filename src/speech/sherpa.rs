
use std::path::Path;

use sherpa_onnx::{OfflineRecognizer, OfflineRecognizerConfig, OfflineWhisperModelConfig};

use super::config::{DECODER, ENCODER, TOKENS};
use super::engine::{SAMPLE_RATE, SpeechEngine};

const THREADS: i32 = 4;

pub struct WhisperEngine {
    recognizer: OfflineRecognizer,
}

impl WhisperEngine {
    pub fn new(model_dir: &Path, language: Option<&str>) -> Result<Self, String> {
        let path = |name: &str| model_dir.join(name).to_string_lossy().into_owned();
        let mut config = OfflineRecognizerConfig::default();
        config.model_config.whisper = OfflineWhisperModelConfig {
            encoder: Some(path(ENCODER)),
            decoder: Some(path(DECODER)),
            language: Some(language.unwrap_or_default().to_string()),
            task: Some("transcribe".into()),
            tail_paddings: -1,
            ..Default::default()
        };
        config.model_config.tokens = Some(path(TOKENS));
        config.model_config.num_threads = THREADS;
        config.model_config.provider = Some("cpu".into());
        let recognizer = OfflineRecognizer::create(&config)
            .ok_or_else(|| "cannot load the speech model".to_string())?;
        Ok(Self { recognizer })
    }
}

impl SpeechEngine for WhisperEngine {
    fn transcribe(&mut self, samples: &[f32]) -> Result<String, String> {
        let stream = self.recognizer.create_stream();
        stream.accept_waveform(SAMPLE_RATE as i32, samples);
        self.recognizer.decode(&stream);
        let result = stream
            .get_result()
            .ok_or_else(|| "the recognizer returned no result".to_string())?;
        Ok(result.text.trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::super::vad::Vad;
    use super::*;
    use sherpa_onnx::Wave;
    use std::time::Duration;

    #[test]
    #[ignore]
    fn vad_and_whisper_transcribe_a_wav() {
        let dir = std::env::var("AGENTORY_SPEECH_MODEL_DIR").expect("AGENTORY_SPEECH_MODEL_DIR");
        let wav = std::env::var("AGENTORY_SPEECH_WAV").expect("AGENTORY_SPEECH_WAV");
        let dir = Path::new(&dir);
        let wave = Wave::read(&wav).expect("wav");
        assert_eq!(wave.sample_rate(), SAMPLE_RATE as i32);

        let vad = Vad::new(dir, Duration::from_millis(800)).unwrap();
        let mut engine = WhisperEngine::new(dir, Some("en")).unwrap();
        let silence = vec![0.0; SAMPLE_RATE as usize * 2];
        for chunk in wave.samples().chunks(1600).chain(silence.chunks(1600)) {
            vad.accept(chunk);
        }
        vad.flush();
        let mut text = String::new();
        while let Some(segment) = vad.next_segment() {
            text.push_str(&engine.transcribe(&segment).unwrap());
            text.push(' ');
        }
        println!("transcript: {text}");
        assert!(!text.trim().is_empty());
    }
}
