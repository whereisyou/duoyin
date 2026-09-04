//! 任务生命周期命令：启动（多目标/legacy 单目标）、取消（整任务/单目标）。
//!
//! 运行中任务的句柄与取消令牌集中登记在 crate::state（TASKS 全局表）；
//! 持久化状态走 application::task_service。

use std::sync::Mutex;

use tauri::{Emitter, Manager};

use crate::state::{RunningTask, TASKS};
use crate::types::{AppConfig, ProgressEvent};
use crate::{application, domain, logger, pipeline};

// legacy::command::start_task 也复用这两个 helper（共享配置装配/时间戳，不重复实现）。
pub(crate) fn pipeline_config_from_app(
    source_language: Option<String>,
    targets: Vec<domain::variant::TargetVariant>,
    config: &AppConfig,
) -> Result<domain::config::PipelineConfig, String> {
    let subtitle = match config.subtitle_mode.as_str() {
        "none" => domain::config::SubtitleMode::None,
        "external_srt" => domain::config::SubtitleMode::ExternalSrt,
        "hard_subtitle_planned" => domain::config::SubtitleMode::HardSubtitlePlanned,
        other => return Err(format!("未知字幕模式: {other}")),
    };
    let naming = match config.output_naming.as_str() {
        "source_variant" => domain::config::OutputNaming::SourceVariant,
        "final" => domain::config::OutputNaming::Final,
        other => return Err(format!("未知输出命名规则: {other}")),
    };
    Ok(domain::config::PipelineConfig {
        source_language,
        targets,
        engines: domain::config::EngineSelection {
            stt: config.stt_engine.clone(),
            translator: "openai-compatible".into(),
            tts: config.tts_engine.clone(),
            separator: config
                .separation_enabled
                .then(|| "uvr-mdx-net-inst-292".into()),
        },
        separation: domain::config::SeparationConfig {
            enabled: config.separation_enabled,
            denoise: config.separation_denoise,
            normalize: config.separation_normalize,
            allow_no_bgm_fallback: config.separation_fallback_no_bgm,
        },
        output: domain::config::OutputConfig {
            generate_final_videos: config.generate_final_videos,
            naming,
            keep_original_audio_track: config.keep_original_audio_track,
            min_speed_percent: config.min_speed_percent,
            max_speed_percent: config.max_speed_percent,
            subtitle,
        },
    })
}

pub(crate) fn now_millis() -> Result<i64, String> {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| error.to_string())?
            .as_millis(),
    )
    .map_err(|_| "系统时间超出范围".into())
}

/// 多目标任务：共享父级 STT，各目标版本独立翻译/TTS/合成。
#[tauri::command]
pub async fn start_multi_target_task(
    video: String,
    source_language: String,
    targets: Vec<domain::variant::TargetVariant>,
    existing_task_id: Option<String>,
    app: tauri::AppHandle,
) -> Result<String, String> {
    let cfg = app
        .state::<Mutex<AppConfig>>()
        .lock()
        .map_err(|error| error.to_string())?
        .clone();
    let service = application::task_service::TaskService::from_local_app_data()?;
    let task_id = if let Some(existing) = existing_task_id {
        domain::ids::TaskId(existing)
    } else {
        let pipeline_config = pipeline_config_from_app(
            if source_language.trim().is_empty() || source_language == "auto" {
                None
            } else {
                Some(source_language)
            },
            targets,
            &cfg,
        )?;
        application::pipeline_service::validate_pipeline_configuration(&cfg, &pipeline_config)
            .map_err(|error| {
                logger::record_failure(&video, "preflight", "configuration", &error);
                log::error!("new task preflight failed video={}: {}", video, error);
                error
            })?;
        service
            .create_task(std::path::Path::new(&video), pipeline_config, now_millis()?)?
            .task_id
    };
    let id = task_id.0.clone();
    {
        let mut tasks = TASKS.lock().map_err(|error| error.to_string())?;
        tasks.retain(|_, running| !running.handle.is_finished());
        if tasks.contains_key(&id) {
            return Err("该任务正在运行，请勿重复启动".into());
        }
    }
    let store = service.store();
    let started = store
        .load_bundle(&task_id)
        .map_err(|error| error.to_string())?;
    if let Err(error) = application::pipeline_service::validate_pipeline_configuration(
        &cfg,
        &started.task.config,
    ) {
        logger::record_failure(&id, "preflight", "configuration", &error);
        log::error!(
            "task {} preflight failed task_dir={}: {}",
            id,
            store
                .task_dir(&task_id)
                .map(|path| path.to_string_lossy().into_owned())
                .unwrap_or_else(|_| "unknown".into()),
            error
        );
        return Err(error);
    }
    service.mark_started(&task_id, now_millis()?)?;
    let started = store
        .load_bundle(&task_id)
        .map_err(|error| error.to_string())?;
    let task = started.task;
    let manifest = started.manifest;
    let app_clone = app.clone();
    let id_clone = id.clone();
    let cancel = pipeline::runner::CancelToken::default();
    let run_cancel = cancel.clone();
    let child_cancels: std::collections::BTreeMap<_, _> = task
        .config
        .targets
        .iter()
        .map(|target| (target.id.clone(), pipeline::runner::CancelToken::default()))
        .collect();
    let run_child_cancels = child_cancels.clone();
    let running_task_id = task_id.clone();
    let handle = tokio::spawn(async move {
        let event_name = format!("task:{}", id_clone);
        struct TauriPipelineObserver {
            app: tauri::AppHandle,
            event_name: String,
        }
        impl pipeline::runner::PipelineObserver for TauriPipelineObserver {
            fn on_stage_update(&self, update: pipeline::runner::StageUpdate) {
                let (status, progress) = match update.status {
                    domain::manifest::StageStatus::Done
                    | domain::manifest::StageStatus::Skipped
                    | domain::manifest::StageStatus::Degraded => ("done", 100),
                    domain::manifest::StageStatus::Failed
                    | domain::manifest::StageStatus::Canceled
                    | domain::manifest::StageStatus::Interrupted => ("error", 0),
                    _ => ("running", 0),
                };
                let (scope, variant_id) = match update.scope {
                    pipeline::runner::RunScope::Parent => ("parent", None),
                    pipeline::runner::RunScope::Target(variant) => ("target", Some(variant.0)),
                };
                let _ = self.app.emit(
                    &self.event_name,
                    ProgressEvent {
                        step: update.stage_id.0,
                        progress,
                        status: status.into(),
                        error: update.error,
                        segments: None,
                        output_dir: None,
                        scope: Some(scope.into()),
                        variant_id,
                        parent_status: Some("running".into()),
                    },
                );
            }
        }
        let observer = std::sync::Arc::new(TauriPipelineObserver {
            app: app_clone.clone(),
            event_name: event_name.clone(),
        });
        let target_app = app_clone.clone();
        let target_event_name = event_name.clone();
        let task_root = store.task_dir(&task_id).ok();
        let on_target = std::sync::Arc::new(
            move |variant: &domain::ids::VariantId, result: &Result<(), String>| {
                let output_dir = task_root.as_ref().map(|root| {
                    root.join("targets")
                        .join(&variant.0)
                        .to_string_lossy()
                        .into_owned()
                });
                let _ = target_app.emit(
                    &target_event_name,
                    ProgressEvent {
                        step: if result.is_ok() { "done" } else { "error" }.into(),
                        progress: if result.is_ok() { 100 } else { 0 },
                        status: if result.is_ok() { "done" } else { "error" }.into(),
                        error: result.as_ref().err().cloned(),
                        segments: None,
                        output_dir,
                        scope: Some("target".into()),
                        variant_id: Some(variant.0.clone()),
                        parent_status: None,
                    },
                );
            },
        );
        let result = application::pipeline_service::run_configured_pipeline(
            cfg,
            task,
            manifest,
            store.clone(),
            run_cancel,
            run_child_cancels,
            observer,
            on_target,
        )
        .await;
        match result {
            Ok(result) => {
                let string_results: std::collections::BTreeMap<_, _> =
                    result.targets.into_iter().collect();
                match service.mark_targets_finished(
                    &task_id,
                    &string_results,
                    now_millis().unwrap_or_default(),
                ) {
                    Ok(status) => {
                        let failed = string_results.values().filter(|item| item.is_err()).count();
                        for (variant, target_result) in &string_results {
                            if let Err(error) = target_result {
                                log::error!(
                                    "task {} target {} failed: {}",
                                    id_clone,
                                    variant.0,
                                    error
                                );
                            }
                        }
                        let all_failed = failed == string_results.len();
                        let parent_status = match status {
                            domain::task::ParentStatus::PartiallyFailed => "partially_failed",
                            domain::task::ParentStatus::Completed => "completed",
                            _ => "failed",
                        };
                        let _ = app_clone.emit(
                            &event_name,
                            ProgressEvent {
                                step: "done".into(),
                                progress: 100,
                                status: if all_failed { "error" } else { "done" }.into(),
                                error: if failed > 0 {
                                    Some(format!("{failed} 个目标版本失败"))
                                } else {
                                    None
                                },
                                segments: None,
                                output_dir: store
                                    .task_dir(&task_id)
                                    .ok()
                                    .map(|path| path.to_string_lossy().into_owned()),
                                scope: Some("parent".into()),
                                variant_id: None,
                                parent_status: Some(parent_status.into()),
                            },
                        );
                    }
                    Err(error) => log::error!("task {} final state failed: {}", id_clone, error),
                }
            }
            Err(error) => {
                logger::record_failure(&id_clone, "parent:pipeline", "pipeline", &error);
                log::error!(
                    "task {} pipeline failed task_dir={}: {}",
                    id_clone,
                    store
                        .task_dir(&task_id)
                        .map(|path| path.to_string_lossy().into_owned())
                        .unwrap_or_else(|_| "unknown".into()),
                    error
                );
                let _ = service.mark_finished(&task_id, false, now_millis().unwrap_or_default());
                let _ = app_clone.emit(
                    &event_name,
                    ProgressEvent {
                        step: "pipeline".into(),
                        progress: 0,
                        status: "error".into(),
                        error: Some(error),
                        segments: None,
                        output_dir: None,
                        scope: Some("parent".into()),
                        variant_id: None,
                        parent_status: Some("failed".into()),
                    },
                );
            }
        }
    });
    TASKS.lock().map_err(|error| error.to_string())?.insert(
        id.clone(),
        RunningTask {
            handle,
            cancel: Some(cancel),
            task_id: running_task_id,
            child_cancels,
        },
    );
    Ok(id)
}

/// 取消任务
#[tauri::command]
pub fn cancel_child_task(id: String, variant_id: String) -> Result<(), String> {
    let (task_id, cancel) = {
        let tasks = TASKS.lock().map_err(|error| error.to_string())?;
        let running = tasks.get(&id).ok_or("父任务未运行")?;
        let variant = domain::ids::VariantId(variant_id);
        let cancel = running
            .child_cancels
            .get(&variant)
            .cloned()
            .ok_or("目标版本未运行")?;
        (running.task_id.clone(), (variant, cancel))
    };
    cancel.1.cancel();
    application::task_service::TaskService::from_local_app_data()?.mark_child_canceled(
        &task_id,
        &cancel.0,
        now_millis()?,
    )?;
    log::info!("task {} child {} cancelled", id, cancel.0 .0);
    Ok(())
}

#[tauri::command]
pub async fn cancel_task(id: String) -> Result<(), String> {
    let running = TASKS.lock().map_err(|e| e.to_string())?.remove(&id);
    if let Some(mut running) = running {
        if let Some(cancel) = &running.cancel {
            cancel.cancel();
            tokio::select! {
                _ = &mut running.handle => {}
                _ = tokio::time::sleep(std::time::Duration::from_millis(500)) => {
                    running.handle.abort();
                }
            }
        } else {
            running.handle.abort();
        }
        application::task_service::TaskService::from_local_app_data()?
            .mark_canceled(&running.task_id, now_millis()?)?;
        log::info!("task {} cancelled", id);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zh() -> Vec<domain::variant::TargetVariant> {
        vec![domain::variant::TargetVariant::language("zh-CN").unwrap()]
    }

    /// 配置 → PipelineConfig 字段映射（用户点「开始」后的第一道关卡）：
    /// 引擎选择/分离开关/输出命名/字幕模式逐项透传，防「新配置项加了但没接上」
    /// （历史教训：final_video 漏接，正是这类接线问题）。
    #[test]
    fn pipeline_config_maps_engine_selection_and_output() {
        let mut cfg = AppConfig::default();
        cfg.stt_engine = "sensevoice".into();
        cfg.tts_engine = "supertonic".into();
        cfg.separation_enabled = true;
        cfg.separation_denoise = true;
        cfg.separation_normalize = false;
        cfg.subtitle_mode = "external_srt".into();
        cfg.output_naming = "final".into();
        cfg.generate_final_videos = true;
        cfg.min_speed_percent = 90;
        cfg.max_speed_percent = 120;

        let p = pipeline_config_from_app(Some("zh".into()), zh(), &cfg).unwrap();
        assert_eq!(p.source_language.as_deref(), Some("zh"));
        assert_eq!(p.engines.stt, "sensevoice");
        assert_eq!(p.engines.translator, "openai-compatible");
        assert_eq!(p.engines.tts, "supertonic");
        // 分离开启 → 引擎名固定为 UVR 模型标识；关闭 → None
        assert_eq!(p.engines.separator.as_deref(), Some("uvr-mdx-net-inst-292"));
        assert!(p.separation.enabled);
        assert!(p.separation.denoise);
        assert!(!p.separation.normalize);
        assert!(p.output.generate_final_videos);
        assert_eq!(p.output.min_speed_percent, 90);
        assert_eq!(p.output.max_speed_percent, 120);
        assert_eq!(p.output.naming, domain::config::OutputNaming::Final);
    }

    #[test]
    fn pipeline_config_separation_off_yields_no_separator() {
        let mut cfg = AppConfig::default();
        cfg.separation_enabled = false;
        let p = pipeline_config_from_app(None, zh(), &cfg).unwrap();
        assert_eq!(p.source_language, None);
        assert_eq!(p.engines.separator, None);
        assert!(!p.separation.enabled);
    }

    #[test]
    fn pipeline_config_rejects_unknown_enum_values() {
        let mut cfg = AppConfig::default();
        cfg.subtitle_mode = "burn_in".into();
        let err = pipeline_config_from_app(None, zh(), &cfg).unwrap_err();
        assert!(err.contains("未知字幕模式"), "{err}");

        let mut cfg = AppConfig::default();
        cfg.output_naming = "fancy".into();
        let err = pipeline_config_from_app(None, zh(), &cfg).unwrap_err();
        assert!(err.contains("未知输出命名规则"), "{err}");
    }
}
