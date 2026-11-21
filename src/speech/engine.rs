pub const SAMPLE_RATE: u32 = 16_000;

pub trait SpeechEngine: Send {
    fn transcribe(&mut self, samples: &[f32]) -> Result<String, String>;
}
