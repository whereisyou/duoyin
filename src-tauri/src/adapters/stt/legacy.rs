use std::path::Path;

use crate::pipeline::runner::CancelToken;
use crate::ports::stt::{sanitize_segments, SttEngine, SttError, SttFuture};
use crate::types::AppConfig;

#[derive(Clone)]
pub struct ConfiguredSttEngine {
    config: AppConfig,
}

impl ConfiguredSttEngine {
    pub fn new(config: AppConfig) -> Self {
        Self { config }
    }
}

impl SttEngine for ConfiguredSttEngine {
    fn version(&self) -> String {
        format!("configured-stt:{}:v1", self.config.stt_engine)
    }

    fn resource_cost(&self) -> crate::scheduler::ResourceCost {
        match self.config.stt_engine.as_str() {
            "openai_api" => crate::scheduler::ResourceCost::default(),
            "whisper_local" => crate::scheduler::ResourceCost {
                process_slots: 1,
                ram_bytes: 512 * crate::memcheck::MB,
                ..Default::default()
            },
            engine => crate::scheduler::stt(engine).into(),
        }
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
            let config = self.config.clone();
            let audio = audio.to_owned();
            let language = source_language.unwrap_or("auto").to_owned();
            let segments = match config.stt_engine.as_str() {
                "openai_api" => {
                    crate::engines::stt::openai_api::transcribe(&audio, &language, &config.openai_key)
                        .await
                        .map_err(SttError::Engine)?
                }
                "whisper_local" => crate::engines::stt::whisper_cli::transcribe(&audio, &language, &config)
                    .await
                    .map_err(SttError::Engine)?,
                "sensevoice" => {
                    #[cfg(feature = "inference")]
                    {
                        let cancel = cancel.clone();
                        tokio::task::spawn_blocking(move || {
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
                        })?
                    }
                    #[cfg(not(feature = "inference"))]
                    return Err(SttError::Engine("推理功能未启用".into()));
                }
                _ => {
                    #[cfg(feature = "inference")]
                    {
                        let cancel = cancel.clone();
                        tokio::task::spawn_blocking(move || {
                            if cancel.is_canceled() {
                                return Err("STT 已取消".into());
                            }
                            crate::engines::stt::whisper_native::transcribe(
                                &audio,
                                &language,
                                &config,
                                |_| {},
                            )
                        })
                        .await
                        .map_err(|error| SttError::Engine(error.to_string()))?
                        .map_err(SttError::Engine)?
                    }
                    #[cfg(not(feature = "inference"))]
                    return Err(SttError::Engine("推理功能未启用".into()));
                }
            };
            sanitize_segments(segments)
        })
    }
}
