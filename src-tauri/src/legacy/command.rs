//! 【冻结】旧单目标入口 `start_task`（IPC 名不变，仅紧急回滚使用）。
//! 新任务请走 `commands::task::start_multi_target_task`（多目标 + DAG pipeline）。

use std::sync::Mutex;

use tauri::{Emitter, Manager};

use super::process;
use crate::commands::task::{now_millis, pipeline_config_from_app};
use crate::state::{RunningTask, TASKS};
use crate::types::{AppConfig, ProgressEvent, TaskConfig};
use crate::{application, domain, logger};

/// 兼容单目标旧入口；多目标走 start_multi_target_task。
#[tauri::command]
pub async fn start_task(config: TaskConfig, app: tauri::AppHandle) -> Result<String, String> {
    // 从托管状态读取最新配置（设置页保存后立即生效）
    let cfg = app
        .state::<Mutex<AppConfig>>()
        .lock()
        .map_err(|e| e.to_string())?
        .clone();
    let variant = domain::variant::TargetVariant::language(&config.target_lang)?;
    let pipeline_config = pipeline_config_from_app(
        if config.source_lang.trim().is_empty() || config.source_lang == "auto" {
            None
        } else {
            Some(config.source_lang.clone())
        },
        vec![variant.clone()],
        &cfg,
    )?;
    application::pipeline_service::validate_pipeline_configuration(&cfg, &pipeline_config)
        .map_err(|error| {
            logger::record_failure(&config.video, "preflight", "configuration", &error);
            log::error!("legacy task preflight failed video={}: {}", config.video, error);
            error
        })?;
    let service = application::task_service::TaskService::from_local_app_data()?;
    let created = service.create_task(
        std::path::Path::new(&config.video),
        pipeline_config,
        now_millis()?,
    )?;
    let id = created.task_id.0.clone();
    TASKS
        .lock()
        .map_err(|error| error.to_string())?
        .retain(|_, running| !running.handle.is_finished());
    let task_root = created.task_root.clone();
    let task_id = created.task_id.clone();
    service.mark_started(&task_id, now_millis()?)?;

    log::info!(
        "task {} start: video={} {}->{}, stt={}, tts={}",
        id,
        config.video,
        config.source_lang,
        config.target_lang,
        cfg.stt_engine,
        cfg.tts_engine
    );
    let app_clone = app.clone();
    let id_clone = id.clone();
    let running_task_id = task_id.clone();

    let handle = tokio::spawn(async move {
        let work = task_root.join("work");
        let out = task_root.join("targets").join(&variant.id.0);

        if let Err(error) = tokio::fs::create_dir_all(&work).await {
            log::error!("task {} create work dir failed: {}", id_clone, error);
            let _ = service.mark_finished(&task_id, false, now_millis().unwrap_or_default());
            return;
        }
        if let Err(error) = tokio::fs::create_dir_all(&out).await {
            log::error!("task {} create output dir failed: {}", id_clone, error);
            let _ = service.mark_finished(&task_id, false, now_millis().unwrap_or_default());
            return;
        }
        let succeeded = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

        // Arc 共享发射器：流水线内的阻塞子任务也可克隆转发进度
        let emit: std::sync::Arc<dyn Fn(ProgressEvent) + Send + Sync> = {
            let app = app_clone.clone();
            let id = id_clone.clone();
            let succeeded = succeeded.clone();
            // 统一在这里记流水：流水线每一步进度/错误都进日志，崩溃前最后的现场可回溯
            std::sync::Arc::new(move |evt: ProgressEvent| {
                if evt.status == "done" {
                    succeeded.store(true, std::sync::atomic::Ordering::Release);
                }
                match (evt.status.as_str(), &evt.error) {
                    ("error", Some(e)) => log::error!("[task {}][{}] {}", id, evt.step, e),
                    _ => log::debug!("[task {}][{}] {}%", id, evt.step, evt.progress),
                }
                let _ = app.emit(&format!("task:{}", id), &evt);
            })
        };

        process::run(&cfg, &config, &work, &out, emit).await;
        let completed =
            succeeded.load(std::sync::atomic::Ordering::Acquire) && out.join("final.mp4").is_file();
        if let Err(error) =
            service.mark_finished(&task_id, completed, now_millis().unwrap_or_default())
        {
            log::error!("task {} persist final status failed: {}", id_clone, error);
        }
    });

    TASKS.lock().map_err(|e| e.to_string())?.insert(
        id.clone(),
        RunningTask {
            handle,
            cancel: None,
            task_id: running_task_id,
            child_cancels: std::collections::BTreeMap::new(),
        },
    );
    Ok(id)
}
