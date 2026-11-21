
use std::path::PathBuf;
use std::time::Duration;

use crate::state::prefs::Prefs;

pub struct ModelFile {
    pub name: &'static str,
    pub url: &'static str,
    pub sha256: &'static str,
    pub size: u64,
}

pub struct ModelSpec {
    pub dir: &'static str,
    pub files: &'static [ModelFile],
}

pub const ENCODER: &str = "small-encoder.int8.onnx";
pub const DECODER: &str = "small-decoder.int8.onnx";
pub const TOKENS: &str = "small-tokens.txt";
pub const VAD_MODEL: &str = "silero_vad.onnx";

pub const WHISPER_SMALL: ModelSpec = ModelSpec {
    dir: "whisper-small-int8",
    files: &[
        ModelFile {
            name: ENCODER,
            url: "https://huggingface.co/csukuangfj/sherpa-onnx-whisper-small/resolve/main/small-encoder.int8.onnx",
            sha256: "4cbe7b22fa9026b843b60a68640c747de05bafb1a11b57edc0e66c232d9f33a9",
            size: 112_442_483,
        },
        ModelFile {
            name: DECODER,
            url: "https://huggingface.co/csukuangfj/sherpa-onnx-whisper-small/resolve/main/small-decoder.int8.onnx",
            sha256: "acad50b5c782696e91b55914cc5ab4f756f1532f76e22aa6fc615f39fb69a8ee",
            size: 262_226_114,
        },
        ModelFile {
            name: TOKENS,
            url: "https://huggingface.co/csukuangfj/sherpa-onnx-whisper-small/resolve/main/small-tokens.txt",
            sha256: "b34b360dbb493e781e479794586d661700670d65564001f23024971d1f2fa126",
            size: 816_730,
        },
        ModelFile {
            name: VAD_MODEL,
            url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/silero_vad.onnx",
            sha256: "9e2449e1087496d8d4caba907f23e0bd3f78d91fa552479bb9c23ac09cbb1fd6",
            size: 643_854,
        },
    ],
};

pub fn models_root() -> Option<PathBuf> {
    Some(
        dirs::data_local_dir()?
            .join("Agentory")
            .join("models")
            .join("speech"),
    )
}

#[derive(Debug, Clone)]
pub struct PipelineConfig {
    pub model_dir: PathBuf,
    pub language: Option<String>,
    pub device: String,
    pub silence: Duration,
}

impl PipelineConfig {
    pub fn from_prefs(prefs: &Prefs, model_dir: PathBuf) -> Self {
        Self {
            model_dir,
            language: prefs.speech_language_code().map(str::to_string),
            device: prefs.mic_device.clone(),
            silence: Duration::from_millis(u64::from(prefs.vad_silence_ms)),
        }
    }
}
