use std::path::{Path, PathBuf};

use crate::domain::variant::TargetVariant;
use crate::pipeline::runner::CancelToken;
use crate::ports::tts::{validate_tts_input, TtsEngine, TtsError, TtsFuture, TtsOutput};
use crate::types::{AppConfig, Segment};

#[derive(Debug, Clone)]
pub struct SupertonicEngine {
    config: AppConfig,
}

impl SupertonicEngine {
    pub fn new(model_dir: impl Into<PathBuf>, voice: impl Into<String>) -> Self {
        let mut config = AppConfig::default();
        config.tts_engine = "supertonic".into();
        config.supertonic_dir = model_dir.into().to_string_lossy().into_owned();
        config.supertonic_voice = voice.into();
        Self { config }
    }
}

impl TtsEngine for SupertonicEngine {
    fn version(&self) -> String {
        "supertonic-onnx-v1".into()
    }

    fn resource_cost(&self) -> crate::scheduler::ResourceCost {
        crate::scheduler::TTS.into()
    }

    fn synthesize<'a>(
        &'a self,
        segments: &'a [Segment],
        target: &'a TargetVariant,
        output_dir: &'a Path,
        alignment: crate::ports::tts::TtsAlignment,
        cancel: &'a CancelToken,
    ) -> TtsFuture<'a> {
        Box::pin(async move {
            if cancel.is_canceled() {
                return Err(TtsError::Canceled);
            }
            validate_tts_input(segments)?;
            if target.language == "zh" && target.dialect.as_deref() != Some("mandarin") {
                return Err(TtsError::UnsupportedVariant(format!(
                    "Supertonic 不支持中文方言 {}，需使用支持 instruct 的 TTS",
                    target.display_name
                )));
            }
            let language = target.language.clone();
            crate::engines::tts::supertonic::validate_language_assets(
                &self.config.supertonic_dir,
                &language,
            )
            .map_err(TtsError::UnsupportedVariant)?;
            let segments = segments.to_vec();
            let config = self.config.clone();
            let output_dir = output_dir.to_owned();
            let blocking_output_dir = output_dir.clone();
            let cancel = cancel.clone();
            tokio::task::spawn_blocking(move || {
                crate::engines::tts::supertonic::synthesize_segments_cancelable(
                    &segments,
                    &language,
                    &config,
                    &blocking_output_dir,
                    |_| {},
                    alignment.max_speed_percent,
                    || cancel.is_canceled(),
                )
            })
            .await
            .map_err(|error| TtsError::Engine(error.to_string()))?
            .map_err(|error| {
                if error == "TTS 已取消" {
                    TtsError::Canceled
                } else {
                    TtsError::Engine(error)
                }
            })?;
            Ok(TtsOutput {
                dub_audio: output_dir.join("dub.wav"),
                segment_dir: Some(output_dir.join("audio_segments_tts")),
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn rejects_dialect_before_loading_model() {
        let engine = SupertonicEngine::new("missing", "");
        let result = engine
            .synthesize(
                &[Segment {
                    idx: 0,
                    start: 0.0,
                    end: 1.0,
                    text: "你好".into(),
                    translated: "你好".into(),
                }],
                &TargetVariant::zh_dialect("yue", "粤语", "广东话"),
                Path::new("out"),
                crate::ports::tts::TtsAlignment {
                    min_speed_percent: 85,
                    max_speed_percent: 125,
                },
                &CancelToken::default(),
            )
            .await;
        assert!(matches!(result, Err(TtsError::UnsupportedVariant(_))));
    }
}
