use std::collections::BTreeMap;
use std::sync::Arc;

use crate::adapters::media::ffmpeg::FfmpegMediaTool;
use crate::adapters::media::output_stages::FfmpegOutputStages;
use crate::adapters::media::stages::MediaStageExecutor;
#[cfg(feature = "inference")]
use crate::adapters::separation::sherpa_uvr::SherpaUvrSeparator;
#[cfg(feature = "inference")]
use crate::adapters::separation::stage::SeparationStageExecutor;
use crate::adapters::stt::legacy::ConfiguredSttEngine;
use crate::adapters::stt::stage::SttStageExecutor;
use crate::adapters::translate::openai_compatible::OpenAiCompatibleTranslator;
use crate::adapters::translate::stage::TranslateStageExecutor;
use crate::adapters::tts::cosyvoice3::CosyVoice3Engine;
use crate::adapters::tts::stage::TtsStageExecutor;
#[cfg(feature = "inference")]
use crate::adapters::tts::supertonic::SupertonicEngine;
#[cfg(feature = "inference")]
use crate::adapters::tts::zipvoice::ZipVoiceEngine;
use crate::application::checkpoint::TaskStoreCheckpoint;
use crate::domain::config::PipelineConfig;
use crate::domain::ids::VariantId;
use crate::domain::manifest::TaskManifest;
use crate::infra::artifact_store::ArtifactStore;
use crate::infra::task_store::{TaskDocument, TaskStore};
use crate::pipeline::graph::PipelineGraph;
use crate::pipeline::registry::StageRegistry;
use crate::pipeline::runner::{CancelToken, PipelineObserver, PipelineRunner, StageExecutor};
use crate::ports::tts::TtsAlignment;
use crate::types::AppConfig;

pub struct PipelineRunResult {
    /// 调度/诊断返回携带 manifest（当前调用方只取 results），保留
    #[allow(dead_code)]
    pub manifest: TaskManifest,
    pub targets: BTreeMap<VariantId, Result<(), String>>,
}

/// 在启动昂贵阶段前验证引擎与目标版本兼容性。
pub fn validate_pipeline_configuration(
    app_config: &AppConfig,
    config: &PipelineConfig,
) -> Result<(), String> {
    let mut app_config = app_config.clone();
    app_config.tts_engine = config.engines.tts.clone();
    register_tts(&mut StageRegistry::new(), &app_config, config)
}

pub async fn run_configured_pipeline(
    mut app_config: AppConfig,
    task: TaskDocument,
    manifest: TaskManifest,
    store: Arc<TaskStore>,
    cancel: CancelToken,
    child_cancels: BTreeMap<VariantId, CancelToken>,
    observer: Arc<dyn PipelineObserver>,
    on_target_done: Arc<dyn Fn(&VariantId, &Result<(), String>) + Send + Sync>,
) -> Result<PipelineRunResult, String> {
    // 引擎类型属于任务快照：重启后即使用户修改全局设置，恢复任务也必须按原 DAG 语义执行。
    // 路径、密钥和 API 地址仍从当前 AppConfig 注入，避免把敏感信息复制进任务目录。
    app_config.stt_engine = task.config.engines.stt.clone();
    app_config.tts_engine = task.config.engines.tts.clone();
    validate_pipeline_configuration(&app_config, &task.config)?;

    let task_root = store
        .task_dir(&task.parent.id)
        .map_err(|error| error.to_string())?;
    let mut registry = StageRegistry::new();
    let media = Arc::new(MediaStageExecutor::new(
        Arc::new(FfmpegMediaTool::default()),
    ));
    // 注册统一走 reg()（内部即 register + debug_error），去掉每个调用点的重复后缀
    reg(&mut registry, "media_probe", media.clone())?;
    reg(&mut registry, "extract_audio", media)?;
    reg(
        &mut registry,
        "stt",
        Arc::new(SttStageExecutor::new(
            Arc::new(ConfiguredSttEngine::new(app_config.clone())),
            task.config.source_language.clone(),
        )),
    )?;
    reg(
        &mut registry,
        "translate",
        Arc::new(TranslateStageExecutor::new(
            Arc::new(
                OpenAiCompatibleTranslator::new_with_limits_and_proxy(
                    app_config.deepseek_key.clone(),
                    app_config.deepseek_model.clone(),
                    app_config.deepseek_api_url.clone(),
                    app_config.api_max_concurrent,
                    app_config.api_interval_ms,
                    Some(&app_config.http_proxy),
                )
                .map_err(debug_error)?,
            ),
            task.config.source_language.clone(),
            task.config.targets.clone(),
        )),
    )?;

    register_tts(&mut registry, &app_config, &task.config)?;
    if task.config.separation.enabled {
        #[cfg(feature = "inference")]
        reg(
            &mut registry,
            "separation",
            Arc::new(
                SeparationStageExecutor::new(Arc::new(SherpaUvrSeparator::new(
                    app_config.separator_model_path.clone(),
                )))
                .with_postprocess(
                    task.config.separation.denoise,
                    task.config.separation.normalize,
                ),
            ),
        )?;
        #[cfg(not(feature = "inference"))]
        return Err("推理功能未启用，无法执行背景音分离".into());
    }

    let outputs = Arc::new(FfmpegOutputStages::new(task.config.output.clone()));
    for stage in ["mix", "srt", "final_video"] {
        reg(&mut registry, stage, outputs.clone())?;
    }

    let checkpoint = Arc::new(TaskStoreCheckpoint::new(store, task.clone()));
    let runner = PipelineRunner::new(
        PipelineGraph::video_translation(),
        task.config.clone(),
        manifest,
        Arc::new(registry),
    )
    .with_environment(&task_root, &task.parent.source.path)
    .with_checkpoint(checkpoint)
    .with_observer(observer);

    // 启动前校验用户是否删除/替换过必要产物。
    runner
        .reconcile_artifacts(&ArtifactStore::new(&task_root))
        .await
        .map_err(debug_error)?;
    runner.run_parent(&cancel).await.map_err(debug_error)?;

    let raw_results = runner.run_targets_with_tokens(&child_cancels).await;
    let mut targets = BTreeMap::new();
    for (variant, result) in raw_results {
        let result = result.map_err(debug_error);
        on_target_done(&variant, &result);
        targets.insert(variant, result);
    }

    Ok(PipelineRunResult {
        manifest: runner.manifest_snapshot().await,
        targets,
    })
}

fn register_tts(
    registry: &mut StageRegistry,
    app: &AppConfig,
    config: &PipelineConfig,
) -> Result<(), String> {
    match app.tts_engine.as_str() {
        "cosyvoice3" => {
            let engine = CosyVoice3Engine::new(
                app.cosyvoice_url.clone(),
                app.cosyvoice_key.clone(),
                app.cosyvoice_prompt_wav.clone(),
                app.cosyvoice_prompt_text.clone(),
                app.cosyvoice_sample_rate,
                app.api_max_concurrent,
                app.api_interval_ms,
            )
            .map_err(debug_error)?;
            registry
                .register(
                    "tts",
                    Arc::new(TtsStageExecutor::new(
                        Arc::new(engine),
                        config.targets.clone(),
                        TtsAlignment {
                            min_speed_percent: config.output.min_speed_percent,
                            max_speed_percent: config.output.max_speed_percent,
                        },
                        app.tts_use_video_prompt,
                    )),
                )
                .map_err(debug_error)
        }
        "supertonic" => {
            if config.targets.iter().any(|target| {
                target.dialect.is_some() && target.dialect.as_deref() != Some("mandarin")
            }) {
                return Err("Supertonic 不支持中文方言，请在设置中选择 CosyVoice3".into());
            }
            #[cfg(feature = "inference")]
            {
                for target in &config.targets {
                    crate::engines::tts::supertonic::validate_language_assets(
                        &app.supertonic_dir,
                        &target.language,
                    )?;
                }
                registry
                    .register(
                        "tts",
                        Arc::new(TtsStageExecutor::new(
                            Arc::new(SupertonicEngine::new(
                                app.supertonic_dir.clone(),
                                app.supertonic_voice.clone(),
                            )),
                            config.targets.clone(),
                            TtsAlignment {
                                min_speed_percent: config.output.min_speed_percent,
                                max_speed_percent: config.output.max_speed_percent,
                            },
                            app.tts_use_video_prompt,
                        )),
                    )
                    .map_err(debug_error)
        }
        #[cfg(not(feature = "inference"))]
        {
            Err("推理功能未启用，无法执行 Supertonic TTS".into())
        }
    }
    "zipvoice" => {
            #[cfg(feature = "inference")]
            {
                // preflight：模型目录不齐在任务启动前报错（不走昂贵 STT）
                crate::adapters::tts::zipvoice::validate(&app.zipvoice_dir)?;
                let engine = ZipVoiceEngine::new(
                    app.zipvoice_dir.clone(),
                    app.zipvoice_prompt_wav.clone(),
                    app.zipvoice_prompt_text.clone(),
                    app.zipvoice_num_threads,
                )
                .map_err(debug_error)?;
                registry
                    .register(
                        "tts",
                        Arc::new(TtsStageExecutor::new(
                            Arc::new(engine),
                            config.targets.clone(),
                            TtsAlignment {
                                min_speed_percent: config.output.min_speed_percent,
                                max_speed_percent: config.output.max_speed_percent,
                            },
                            app.tts_use_video_prompt,
                        )),
                    )
                    .map_err(debug_error)
                }
                #[cfg(not(feature = "inference"))]
                {
                    Err("推理功能未启用，无法执行 ZipVoice TTS".into())
                }
        }
        other => Err(format!("新流水线不支持 TTS 引擎 {other}")),
    }
}

fn debug_error(error: impl std::fmt::Debug) -> String {
    format!("{error:?}")
}

/// registry.register + debug_error 的统一封装，去掉每个注册点的重复 `.map_err(debug_error)?`。
fn reg(
    registry: &mut StageRegistry,
    stage: &str,
    executor: Arc<dyn StageExecutor>,
) -> Result<(), String> {
    registry.register(stage, executor).map_err(debug_error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::variant::TargetVariant;

    #[test]
    fn supertonic_rejects_dialect_before_pipeline_runs() {
        let mut app = AppConfig::default();
        app.tts_engine = "supertonic".into();
        let config = PipelineConfig {
            source_language: None,
            targets: vec![TargetVariant::zh_dialect("yue", "粤语", "广东话")],
            engines: crate::domain::config::EngineSelection {
                stt: "sensevoice".into(),
                translator: "openai-compatible".into(),
                tts: "supertonic".into(),
                separator: None,
            },
            separation: crate::domain::config::SeparationConfig::default(),
            output: crate::domain::config::OutputConfig::default(),
        };
        assert!(register_tts(&mut StageRegistry::new(), &app, &config).is_err());
    }

    #[cfg(feature = "inference")]
    #[test]
    fn supertonic_rejects_missing_chinese_extension_before_pipeline_runs() {
        let root = std::env::temp_dir().join(format!("missing-supertonic-zh-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("onnx")).unwrap();
        let mut app = AppConfig::default();
        app.tts_engine = "supertonic".into();
        app.supertonic_dir = root.to_string_lossy().into_owned();
        let config = PipelineConfig {
            source_language: Some("en".into()),
            targets: vec![TargetVariant::zh_mandarin()],
            engines: crate::domain::config::EngineSelection {
                stt: "whisper_native".into(),
                translator: "openai-compatible".into(),
                tts: "supertonic".into(),
                separator: None,
            },
            separation: crate::domain::config::SeparationConfig::default(),
            output: crate::domain::config::OutputConfig::default(),
        };
        let error = register_tts(&mut StageRegistry::new(), &app, &config).unwrap_err();
        assert!(error.contains("duration_predictor_zh.onnx"));
        assert!(error.contains("CosyVoice3"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(feature = "inference")]
    #[tokio::test]
    #[ignore = "requires local Supertonic and SenseVoice models"]
    async fn real_application_pipeline_runs_two_targets_and_persists_outputs() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let supertonic =
            std::path::PathBuf::from(std::env::var("VT_SUPERTONIC_DIR").unwrap_or_else(|_| {
                r"E:\projects\pyvideotrans-3.98\Supertone\supertonic-3".into()
            }));
        let sensevoice = std::path::PathBuf::from(
            std::env::var("VT_SENSEVOICE_DIR").unwrap_or_else(|_| {
                r"E:\projects\test2voices_backup\sense-voice-int8".into()
            }),
        );
        if !supertonic.is_dir() || !sensevoice.is_dir() {
            eprintln!("skip application e2e: local models missing");
            return;
        }
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut bytes = Vec::new();
                let mut buffer = [0u8; 8192];
                loop {
                    let read = stream.read(&mut buffer).unwrap();
                    if read == 0 {
                        break;
                    }
                    bytes.extend_from_slice(&buffer[..read]);
                    if let Some(header_end) =
                        bytes.windows(4).position(|window| window == b"\r\n\r\n")
                    {
                        let headers = String::from_utf8_lossy(&bytes[..header_end]);
                        let length = headers
                            .lines()
                            .find_map(|line| {
                                line.to_ascii_lowercase()
                                    .strip_prefix("content-length: ")
                                    .and_then(|value| value.parse::<usize>().ok())
                            })
                            .unwrap_or(0);
                        if bytes.len() >= header_end + 4 + length {
                            break;
                        }
                    }
                }
                let body_start = bytes
                    .windows(4)
                    .position(|window| window == b"\r\n\r\n")
                    .unwrap()
                    + 4;
                let request: serde_json::Value =
                    serde_json::from_slice(&bytes[body_start..]).unwrap();
                let user = request["messages"][1]["content"].as_str().unwrap_or("");
                let translations: Vec<_> = user
                    .lines()
                    .filter_map(|line| {
                        let end = line.find(']')?;
                        let idx = line.get(1..end)?.parse::<usize>().ok()?;
                        Some(
                            serde_json::json!({"idx":idx,"translated":format!("translated-{idx}")}),
                        )
                    })
                    .collect();
                let content = serde_json::json!({"translations":translations}).to_string();
                let response_body = serde_json::json!({
                    "choices":[{"message":{"content":content}}]
                })
                .to_string();
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    response_body.len(),
                    response_body
                )
                .unwrap();
            }
        });

        let root = std::env::temp_dir().join(format!("application-e2e-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let speech_dir = root.join("speech");
        std::fs::create_dir_all(&speech_dir).unwrap();
        let mut synthesis = AppConfig::default();
        synthesis.supertonic_dir = supertonic.to_string_lossy().into_owned();
        synthesis.supertonic_voice = "M1".into();
        crate::engines::tts::supertonic::synthesize_segments(
            &[crate::types::Segment {
                idx: 0,
                start: 0.0,
                end: 12.0,
                text: String::new(),
                translated: "The watcher stands on the bridge and watches the ships pass under it, while the city lights flicker in the cold night air."
                    .into(),
            }],
            "en",
            &synthesis,
            &speech_dir,
            |_| {},
        )
        .unwrap();
        let source_video = root.join("source.mp4");
        let status = tokio::process::Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=c=black:s=160x90:d=12",
                "-i",
            ])
            .arg(speech_dir.join("dub.wav"))
            .args(["-shortest", "-c:v", "libx264", "-c:a", "aac", "-y"])
            .arg(&source_video)
            .status()
            .await
            .unwrap();
        assert!(status.success());

        let service = crate::application::task_service::TaskService::new(root.join("data"));
        let targets = vec![
            crate::domain::variant::TargetVariant::language("en").unwrap(),
            crate::domain::variant::TargetVariant::language("fr").unwrap(),
        ];
        let config = PipelineConfig {
            source_language: Some("en".into()),
            targets: targets.clone(),
            engines: crate::domain::config::EngineSelection {
                stt: "sensevoice".into(),
                translator: "openai-compatible".into(),
                tts: "supertonic".into(),
                separator: None,
            },
            separation: crate::domain::config::SeparationConfig::default(),
            output: crate::domain::config::OutputConfig {
                generate_final_videos: true,
                ..Default::default()
            },
        };
        let created = service.create_task(&source_video, config, 1).unwrap();
        let mut app = AppConfig::default();
        app.stt_engine = "sensevoice".into();
        app.sensevoice_dir = sensevoice.to_string_lossy().into_owned();
        app.tts_engine = "supertonic".into();
        app.supertonic_dir = supertonic.to_string_lossy().into_owned();
        app.supertonic_voice = "M1".into();
        app.deepseek_api_url = format!("http://{address}/chat");
        app.deepseek_model = "mock".into();
        app.api_interval_ms = 0;
        let child_tokens = targets
            .iter()
            .map(|target| (target.id.clone(), CancelToken::default()))
            .collect();
        struct NoopObserver;
        impl PipelineObserver for NoopObserver {
            fn on_stage_update(&self, _update: crate::pipeline::runner::StageUpdate) {}
        }
        let result = run_configured_pipeline(
            app,
            created.document,
            created.manifest,
            service.store(),
            CancelToken::default(),
            child_tokens,
            Arc::new(NoopObserver),
            Arc::new(|_, _| {}),
        )
        .await
        .unwrap();
        assert!(result.targets.values().all(Result::is_ok));
        for target in targets {
            let dir = created.task_root.join("targets").join(&target.id.0);
            assert!(dir.join("dub.wav").is_file());
            assert!(dir.join("translated.srt").is_file());
            assert!(dir.join(format!("source.{}.mp4", target.id.0)).is_file());
        }
        let _ = std::fs::remove_dir_all(root);
    }
}
