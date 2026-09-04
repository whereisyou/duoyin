use serde::{Deserialize, Serialize};

use crate::application::dialects::{default_dialect_config_path, load_dialects};
use crate::application::task_service::TaskService;
use crate::domain::config::PipelineConfig;
use crate::domain::dialect::LanguageDialectSpec;
use crate::domain::ids::TaskId;
use crate::domain::manifest::TaskManifest;
use crate::domain::task::ParentStatus;
use crate::infra::task_store::TaskDocument;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreatePersistentTaskRequest {
    pub source_video: String,
    pub config: PipelineConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreatePersistentTaskResponse {
    pub task_id: String,
    pub task_root: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistentStageSummary {
    pub stage: String,
    pub status: crate::domain::manifest::StageStatus,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistentChildSummary {
    pub variant_id: String,
    pub status: crate::domain::task::ChildStatus,
    pub output_dir: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistentTaskSummary {
    pub task_id: String,
    pub status: ParentStatus,
    pub updated_at: i64,
    pub revision: u64,
    pub source_video: String,
    pub source_language: Option<String>,
    pub targets: Vec<crate::domain::variant::TargetVariant>,
    pub shared_stages: Vec<PersistentStageSummary>,
    pub shared_bytes: u64,
    pub task_root: String,
    pub variant_bytes: std::collections::BTreeMap<String, u64>,
    pub children: Vec<PersistentChildSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistentTaskDetail {
    pub task_root: String,
    pub task: TaskDocument,
    pub manifest: TaskManifest,
    pub recovered_from_backup: bool,
}

const UVR_MODEL_URL: &str = "https://www.modelscope.cn/models/himyworld/videotrans/resolve/master/onnx/UVR-MDX-NET-Inst_HQ_4.onnx";
const UVR_MODEL_FILE: &str = "UVR-MDX-NET-Inst_HQ_4.onnx";

#[tauri::command]
pub async fn ensure_uvr_model() -> Result<String, String> {
    let model_dir = dirs_next::data_local_dir()
        .ok_or("无法获取本地应用数据目录")?
        .join("videotrans")
        .join("models");
    tokio::fs::create_dir_all(&model_dir)
        .await
        .map_err(|error| error.to_string())?;
    let target = model_dir.join(UVR_MODEL_FILE);
    if target
        .metadata()
        .map(|metadata| metadata.len() > 50_000_000)
        .unwrap_or(false)
    {
        return Ok(target.to_string_lossy().into_owned());
    }
    let temp = target.with_extension("onnx.tmp");
    let mut response = reqwest::Client::new()
        .get(UVR_MODEL_URL)
        .timeout(std::time::Duration::from_secs(600))
        .send()
        .await
        .map_err(|error| format!("下载 UVR 模型失败: {error}"))?;
    if !response.status().is_success() {
        return Err(format!("下载 UVR 模型失败: HTTP {}", response.status()));
    }
    let mut file = tokio::fs::File::create(&temp)
        .await
        .map_err(|error| error.to_string())?;
    use tokio::io::AsyncWriteExt;
    while let Some(chunk) = response.chunk().await.map_err(|error| error.to_string())? {
        file.write_all(&chunk)
            .await
            .map_err(|error| error.to_string())?;
    }
    file.flush().await.map_err(|error| error.to_string())?;
    drop(file);
    let size = tokio::fs::metadata(&temp)
        .await
        .map_err(|error| error.to_string())?
        .len();
    if size < 50_000_000 {
        let _ = tokio::fs::remove_file(&temp).await;
        return Err(format!("下载的 UVR 模型不完整: {size} bytes"));
    }
    if target.exists() {
        tokio::fs::remove_file(&target)
            .await
            .map_err(|error| error.to_string())?;
    }
    tokio::fs::rename(&temp, &target)
        .await
        .map_err(|error| error.to_string())?;
    Ok(target.to_string_lossy().into_owned())
}

#[tauri::command]
pub fn load_dialect_specs() -> Result<Vec<LanguageDialectSpec>, String> {
    load_dialects(&default_dialect_config_path()?)
}

#[tauri::command]
pub fn create_persistent_task(
    request: CreatePersistentTaskRequest,
) -> Result<CreatePersistentTaskResponse, String> {
    let service = TaskService::from_local_app_data()?;
    let created = service.create_task(
        std::path::Path::new(&request.source_video),
        request.config,
        now_millis()?,
    )?;
    Ok(CreatePersistentTaskResponse {
        task_id: created.task_id.0,
        task_root: created.task_root.to_string_lossy().into_owned(),
    })
}

#[tauri::command]
pub fn list_persistent_tasks() -> Result<Vec<PersistentTaskSummary>, String> {
    let service = TaskService::from_local_app_data()?;
    let index = service
        .store()
        .load_index_or_rebuild()
        .map_err(|error| error.to_string())?;
    Ok(index
        .tasks
        .into_values()
        .filter_map(|entry| {
            let loaded = service.recover(&entry.task_id).ok()?;
            let root = service.store().task_dir(&entry.task_id).ok()?;
            let shared_bytes = directory_size(&root.join("shared"));
            let variant_bytes: std::collections::BTreeMap<String, u64> = loaded
                .task
                .config
                .targets
                .iter()
                .map(|target| {
                    (
                        target.id.0.clone(),
                        directory_size(&root.join("targets").join(&target.id.0)),
                    )
                })
                .collect();
            if matches!(
                loaded.task.parent.status,
                ParentStatus::Failed | ParentStatus::PartiallyFailed
            ) {
                let mut failures = Vec::new();
                for record in loaded.manifest.stages.values() {
                    if matches!(
                        record.status,
                        crate::domain::manifest::StageStatus::Failed
                            | crate::domain::manifest::StageStatus::Interrupted
                    ) {
                        failures.push(format!(
                            "parent:{}={}",
                            record.stage_id.0,
                            record.error.as_deref().unwrap_or("unknown")
                        ));
                    }
                }
                for (variant, stages) in &loaded.manifest.target_stages {
                    for record in stages.values() {
                        if matches!(
                            record.status,
                            crate::domain::manifest::StageStatus::Failed
                                | crate::domain::manifest::StageStatus::Interrupted
                        ) {
                            failures.push(format!(
                                "target:{}:{}={}",
                                variant.0,
                                record.stage_id.0,
                                record.error.as_deref().unwrap_or("unknown")
                            ));
                        }
                    }
                }
                log::error!(
                    "historical task failed task_id={} status={:?} source={} task_dir={} failures=[{}]",
                    entry.task_id.0,
                    loaded.task.parent.status,
                    loaded.task.parent.source.path.display(),
                    root.display(),
                    failures.join(" | ")
                );
            }
            let children = loaded
                .task
                .children
                .iter()
                .map(|child| {
                    let bytes = *variant_bytes.get(&child.variant.id.0).unwrap_or(&0);
                    PersistentChildSummary {
                        variant_id: child.variant.id.0.clone(),
                        status: child.status.clone(),
                        output_dir: root
                            .join("targets")
                            .join(&child.variant.id.0)
                            .to_string_lossy()
                            .into_owned(),
                        bytes,
                    }
                })
                .collect();
            let shared_stages = ["media_probe", "extract_audio", "separation", "stt"]
                .into_iter()
                .map(|stage| {
                    let record = loaded
                        .manifest
                        .stages
                        .get(&crate::domain::ids::StageId(stage.into()));
                    PersistentStageSummary {
                        stage: stage.into(),
                        status: record
                            .map(|item| item.status.clone())
                            .unwrap_or(crate::domain::manifest::StageStatus::Pending),
                        error: record.and_then(|item| item.error.clone()),
                    }
                })
                .collect();
            Some(PersistentTaskSummary {
                task_id: entry.task_id.0,
                status: entry.status,
                updated_at: entry.updated_at,
                revision: entry.revision,
                source_video: loaded
                    .task
                    .parent
                    .source
                    .path
                    .to_string_lossy()
                    .into_owned(),
                source_language: loaded.task.config.source_language,
                targets: loaded.task.config.targets,
                shared_stages,
                shared_bytes,
                task_root: root.to_string_lossy().into_owned(),
                variant_bytes,
                children,
            })
        })
        .collect())
}

#[tauri::command]
pub fn delete_persistent_task(task_id: String) -> Result<(), String> {
    let task_id = TaskId(task_id);
    {
        let running = crate::state::TASKS
            .lock()
            .map_err(|error| error.to_string())?;
        if running.contains_key(&task_id.0) {
            return Err("任务正在运行，请先取消再删除".into());
        }
    }
    let service = TaskService::from_local_app_data()?;
    service
        .delete_task(&task_id)
        .map_err(|error| error.to_string())?;
    log::info!("task deleted task_id={}", task_id.0);
    Ok(())
}

#[tauri::command]
pub fn load_task_segments(
    task_id: String,
    variant_id: Option<String>,
) -> Result<Vec<crate::types::Segment>, String> {
    let service = TaskService::from_local_app_data()?;
    let variant = variant_id.map(crate::domain::ids::VariantId);
    crate::application::subtitle_edit::load_segments(
        &service.store(),
        &TaskId(task_id),
        variant.as_ref(),
    )
}

#[tauri::command]
pub fn save_task_segments(
    task_id: String,
    variant_id: Option<String>,
    segments: Vec<crate::types::Segment>,
) -> Result<(), String> {
    let service = TaskService::from_local_app_data()?;
    let variant = variant_id.map(crate::domain::ids::VariantId);
    crate::application::subtitle_edit::save_segments(
        &service.store(),
        &TaskId(task_id),
        variant.as_ref(),
        &segments,
    )
}

#[tauri::command]
pub fn import_target_srt(task_id: String, variant_id: String) -> Result<(), String> {
    let path = rfd::FileDialog::new()
        .add_filter("SubRip Subtitle", &["srt"])
        .pick_file()
        .ok_or("未选择 SRT 文件")?;
    let service = TaskService::from_local_app_data()?;
    crate::application::subtitle_import::import_target_srt(
        &service.store(),
        &TaskId(task_id),
        &crate::domain::ids::VariantId(variant_id),
        &path,
    )
}

#[tauri::command]
pub fn load_persistent_task(task_id: String) -> Result<PersistentTaskDetail, String> {
    let service = TaskService::from_local_app_data()?;
    let task_id = TaskId(task_id);
    let loaded = service
        .store()
        .load_bundle(&task_id)
        .map_err(|error| error.to_string())?;
    let task_root = service
        .store()
        .task_dir(&task_id)
        .map_err(|error| error.to_string())?;
    Ok(PersistentTaskDetail {
        task_root: task_root.to_string_lossy().into_owned(),
        task: loaded.task,
        manifest: loaded.manifest,
        recovered_from_backup: loaded.recovered_from_backup,
    })
}

fn directory_size(path: &std::path::Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    entries
        .filter_map(Result::ok)
        .map(|entry| {
            entry
                .metadata()
                .map(|metadata| {
                    if metadata.is_dir() {
                        directory_size(&entry.path())
                    } else {
                        metadata.len()
                    }
                })
                .unwrap_or(0)
        })
        .sum()
}

fn now_millis() -> Result<i64, String> {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| error.to_string())?
            .as_millis(),
    )
    .map_err(|_| "系统时间超出范围".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn response_and_detail_are_json_serializable() {
        let response = CreatePersistentTaskResponse {
            task_id: "p1".into(),
            task_root: "tasks/p1".into(),
        };
        assert!(serde_json::to_string(&response).is_ok());
        assert!(now_millis().unwrap() > 0);
    }
}
