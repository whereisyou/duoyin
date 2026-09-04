use std::path::{Path, PathBuf};

use crate::pipeline::runner::CancelToken;
use crate::ports::stt::{sanitize_segments, SttEngine, SttError, SttFuture};
use crate::types::{AppConfig, Segment};

#[derive(Debug, Clone)]
/// SenseVoice 适配器包装——生产经 ConfiguredSttEngine 分发直接构造引擎，
/// 本包装保留作显式注入的入口（备用），不删
#[allow(dead_code)]
pub struct SenseVoiceEngine {
    config: AppConfig,
}

impl SenseVoiceEngine {
    #[allow(dead_code)]
    pub fn new(model_dir: impl Into<PathBuf>) -> Self {
            
        let mut config = AppConfig::default();
        config.stt_engine = "sensevoice".into();
        config.sensevoice_dir = model_dir.into().to_string_lossy().into_owned();
        Self { config }
    }
}

impl SttEngine for SenseVoiceEngine {
    fn version(&self) -> String {
        "sensevoice-sherpa-onnx-v1".into()
    }

    fn resource_cost(&self) -> crate::scheduler::ResourceCost {
        crate::scheduler::stt("sensevoice").into()
    }

    fn transcribe<'a>(
        &'a self,
        audio: &'a Path,
        source_language: Option<&'a str>,
        cancel: &'a CancelToken,
    ) -> SttFuture<'a> {
        Box::pin(async move {
            if cancel.is_canceled() {
                return Err(SttError::Canceled);
            }
            if !audio.is_file() {
                return Err(SttError::InvalidInput(format!(
                    "音频文件不存在: {}",
                    audio.display()
                )));
            }
            let audio = audio.to_owned();
            let language = source_language.unwrap_or("auto").to_owned();
            let config = self.config.clone();
            let cancel = cancel.clone();
            let segments: Vec<Segment> = tokio::task::spawn_blocking(move || {
                crate::engines::stt::sensevoice::transcribe_cancelable(
                    &audio,
                    &language,
                    &config,
                    |_| {},
                    || cancel.is_canceled(),
                )
            })
            .await
            .map_err(|error| SttError::Engine(error.to_string()))?
            .map_err(|error| {
                if error == "STT 已取消" {
                    SttError::Canceled
                } else {
                    SttError::Engine(error)
                }
            })?;
            sanitize_segments(segments)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cancellation_and_missing_input_fail_before_model_load() {
        let engine = SenseVoiceEngine::new("missing-model");
        let canceled = CancelToken::default();
        canceled.cancel();
        assert!(matches!(
            engine
                .transcribe(Path::new("missing.wav"), None, &canceled)
                .await,
            Err(SttError::Canceled)
        ));
        assert!(matches!(
            engine
                .transcribe(Path::new("missing.wav"), None, &CancelToken::default())
                .await,
            Err(SttError::InvalidInput(_))
        ));
    }
}
