
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpeechEvent {
    Listening,
    SpeechStarted,
    SpeechEnded,
    Transcribing,
    Final(String),
    NoSpeech,
    AudioDropped,
    Error(String),
    Stopped,
}
