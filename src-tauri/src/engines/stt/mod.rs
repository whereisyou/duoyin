//! STT 裸引擎：SenseVoice（sherpa-onnx）、Whisper（candle 原生）、OpenAI API、whisper.cpp CLI。

pub mod openai_api;
#[cfg(feature = "inference")]
pub mod sensevoice;
pub mod whisper_cli;
#[cfg(feature = "inference")]
pub mod whisper_native;
