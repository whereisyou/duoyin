use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use crate::application::checkpoint::recover_task;
use crate::domain::config::PipelineConfig;
use crate::domain::ids::{ChildTaskId, TaskId, VariantId};
use crate::domain::manifest::TaskManifest;
use crate::domain::media::{SourceFingerprint, SourceVideo};
use crate::domain::task::{ChildStatus, ChildTask, ParentStatus, ParentTask};
use crate::infra::task_store::{LoadedTask, TaskDocument, TaskStore};

#[derive(Debug, Clone)]
pub struct CreatedTask {
    pub task_id: TaskId,
    /// 创建后的完整文档/manifest（供调用方复查；当前命令层只取 task_id），保留
    #[allow(dead_code)]
    pub document: TaskDocument,
    #[allow(dead_code)]
    pub manifest: TaskManifest,
    pub task_root: std::path::PathBuf,
}

pub struct TaskService {
    store: Arc<TaskStore>,
}

impl TaskService {
    pub fn from_local_app_data() -> Result<Self, String> {
        let root = dirs_next::data_local_dir()
            .ok_or("无法获取本地应用数据目录")?
            .join("videotrans");
        Ok(Self::new(root))
    }

    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            store: Arc::new(TaskStore::new(root)),
        }
    }

    pub fn store(&self) -> Arc<TaskStore> {
        self.store.clone()
    }

    pub fn create_task(
        &self,
        source_path: &Path,
        config: PipelineConfig,
        now_ms: i64,
    ) -> Result<CreatedTask, String> {
        config.validate()?;
        let canonical_source = fs::canonicalize(source_path)
            .map_err(|error| format!("源视频不可访问 {}: {error}", source_path.display()))?;
        if !canonical_source.is_file() {
            return Err(format!("源视频不是文件: {}", canonical_source.display()));
        }
        let fingerprint = fingerprint(&canonical_source)?;
        fs::create_dir_all(self.store.root()).map_err(|error| error.to_string())?;
        let estimated = crate::infra::diskcheck::estimate_task_bytes(
            fingerprint.size,
            config.targets.len(),
            config.separation.enabled,
            config.output.generate_final_videos,
        );
        crate::infra::diskcheck::ensure_capacity(self.store.root(), estimated)?;
        let task_id = TaskId(uuid::Uuid::new_v4().to_string());
        let task_root = self
            .store
            .task_dir(&task_id)
            .map_err(|error| error.to_string())?;
        let mut children = Vec::with_capacity(config.targets.len());
        let mut child_ids = Vec::with_capacity(config.targets.len());
        for variant in &config.targets {
            let child_id = ChildTaskId(format!("{}--{}", task_id.0, variant.id.0));
            child_ids.push(child_id.clone());
            children.push(ChildTask {
                id: child_id,
                parent_id: task_id.clone(),
                variant: variant.clone(),
                status: ChildStatus::Pending,
                created_at: now_ms,
                updated_at: now_ms,
            });
        }
        let parent = ParentTask {
            id: task_id.clone(),
            source: SourceVideo {
                path: canonical_source,
                fingerprint: fingerprint.clone(),
            },
            status: ParentStatus::Pending,
            children: child_ids,
            created_at: now_ms,
            updated_at: now_ms,
        };
        let mut document = TaskDocument::new(parent, children, config);
        let manifest = TaskManifest::new(task_id.clone(), fingerprint);
        self.store
            .save_bundle(&mut document, &manifest)
            .map_err(|error| error.to_string())?;
        Ok(CreatedTask {
            task_id,
            task_root,
            document,
            manifest,
        })
    }

    pub fn recover(&self, task_id: &TaskId) -> Result<LoadedTask, String> {
        recover_task(&self.store, task_id)
    }

    /// 删除持久化任务（目录 + 索引）。运行中任务的校验由命令层负责（TASKS 表）。
    pub fn delete_task(&self, task_id: &TaskId) -> Result<(), String> {
        self.store
            .delete_task(task_id)
            .map_err(|error| error.to_string())
    }

    pub fn mark_started(&self, task_id: &TaskId, now_ms: i64) -> Result<(), String> {
        self.update_status(task_id, ParentStatus::Running, ChildStatus::Running, now_ms)
    }

    pub fn mark_targets_finished(
        &self,
        task_id: &TaskId,
        results: &BTreeMap<VariantId, Result<(), String>>,
        now_ms: i64,
    ) -> Result<ParentStatus, String> {
        let mut loaded = self
            .store
            .load_bundle(task_id)
            .map_err(|error| error.to_string())?;
        for child in &mut loaded.task.children {
            child.status = if child.status == ChildStatus::Canceled {
                ChildStatus::Canceled
            } else {
                match results.get(&child.variant.id) {
                    Some(Ok(())) => ChildStatus::Completed,
                    Some(Err(error)) if error.contains("Canceled") => ChildStatus::Canceled,
                    Some(Err(_)) => ChildStatus::Failed,
                    None => ChildStatus::Canceled,
                }
            };
            child.updated_at = now_ms;
        }
        let statuses: Vec<_> = loaded
            .task
            .children
            .iter()
            .map(|child| child.status.clone())
            .collect();
        let parent_status = crate::domain::task::aggregate_child_statuses(&statuses);
        loaded.task.parent.status = parent_status.clone();
        loaded.task.parent.updated_at = now_ms;
        self.store
            .save_bundle(&mut loaded.task, &loaded.manifest)
            .map_err(|error| error.to_string())?;
        Ok(parent_status)
    }

    pub fn mark_child_canceled(
        &self,
        task_id: &TaskId,
        variant_id: &VariantId,
        now_ms: i64,
    ) -> Result<(), String> {
        let mut loaded = self
            .store
            .load_bundle(task_id)
            .map_err(|error| error.to_string())?;
        let child = loaded
            .task
            .children
            .iter_mut()
            .find(|child| &child.variant.id == variant_id)
            .ok_or_else(|| format!("目标版本不存在: {}", variant_id.0))?;
        child.status = ChildStatus::Canceled;
        child.updated_at = now_ms;
        loaded.task.parent.updated_at = now_ms;
        self.store
            .save_bundle(&mut loaded.task, &loaded.manifest)
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    pub fn mark_canceled(&self, task_id: &TaskId, now_ms: i64) -> Result<(), String> {
        self.update_status(
            task_id,
            ParentStatus::Canceled,
            ChildStatus::Canceled,
            now_ms,
        )
    }

    pub fn mark_finished(
        &self,
        task_id: &TaskId,
        succeeded: bool,
        now_ms: i64,
    ) -> Result<(), String> {
        self.update_status(
            task_id,
            if succeeded {
                ParentStatus::Completed
            } else {
                ParentStatus::Failed
            },
            if succeeded {
                ChildStatus::Completed
            } else {
                ChildStatus::Failed
            },
            now_ms,
        )
    }

    fn update_status(
        &self,
        task_id: &TaskId,
        parent_status: ParentStatus,
        child_status: ChildStatus,
        now_ms: i64,
    ) -> Result<(), String> {
        let mut loaded = self
            .store
            .load_bundle(task_id)
            .map_err(|error| error.to_string())?;
        loaded.task.parent.status = parent_status;
        loaded.task.parent.updated_at = now_ms;
        for child in &mut loaded.task.children {
            child.status = child_status.clone();
            child.updated_at = now_ms;
        }
        self.store
            .save_bundle(&mut loaded.task, &loaded.manifest)
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

fn fingerprint(path: &Path) -> Result<SourceFingerprint, String> {
    let metadata = fs::metadata(path).map_err(|error| error.to_string())?;
    let modified = metadata
        .modified()
        .map_err(|error| error.to_string())?
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_millis();
    Ok(SourceFingerprint {
        size: metadata.len(),
        modified: i64::try_from(modified).map_err(|_| "源视频修改时间超出范围")?,
        // 大视频创建任务时不全量读取；ArtifactStore 在元数据变化后再做严格 hash。
        content_hash: None,
        hash_algo_version: 1,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::config::{EngineSelection, OutputConfig, SeparationConfig};
    use crate::domain::variant::TargetVariant;

    fn config() -> PipelineConfig {
        PipelineConfig {
            source_language: None,
            targets: vec![
                TargetVariant::zh_mandarin(),
                TargetVariant::zh_dialect("yue", "粤语", "请用广东话表达。"),
            ],
            engines: EngineSelection {
                stt: "sensevoice".into(),
                translator: "openai-compatible".into(),
                tts: "cosyvoice3".into(),
                separator: None,
            },
            separation: SeparationConfig::default(),
            output: OutputConfig::default(),
        }
    }

    #[test]
    fn creates_stable_parent_and_variant_children() {
        let root = std::env::temp_dir().join(format!("task-service-{}", uuid::Uuid::new_v4()));
        let source = root.join("input.mp4");
        fs::create_dir_all(&root).unwrap();
        fs::write(&source, b"fake-video").unwrap();
        let service = TaskService::new(root.join("data"));

        let created = service.create_task(&source, config(), 100).unwrap();

        assert_eq!(created.document.children.len(), 2);
        assert_eq!(created.document.children[0].variant.id.0, "zh-CN");
        assert_eq!(created.document.children[1].variant.id.0, "zh-yue");
        assert!(created.task_root.join("task.json").is_file());
        assert!(created.task_root.join("manifest.json").is_file());
        assert_eq!(created.document.parent.source.fingerprint.size, 10);
        assert!(created.document.parent.source.path.is_absolute());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn lifecycle_statuses_are_persisted_for_parent_and_child() {
        let root = std::env::temp_dir().join(format!("task-service-{}", uuid::Uuid::new_v4()));
        let source = root.join("input.mp4");
        fs::create_dir_all(&root).unwrap();
        fs::write(&source, b"fake-video").unwrap();
        let service = TaskService::new(root.join("data"));
        let created = service.create_task(&source, config(), 100).unwrap();

        service.mark_started(&created.task_id, 200).unwrap();
        let running = service.store().load_bundle(&created.task_id).unwrap();
        assert_eq!(running.task.revision, 2);
        assert_eq!(running.task.parent.status, ParentStatus::Running);
        assert!(running
            .task
            .children
            .iter()
            .all(|child| child.status == ChildStatus::Running));

        service.mark_finished(&created.task_id, true, 300).unwrap();
        let completed = service.store().load_bundle(&created.task_id).unwrap();
        assert_eq!(completed.task.revision, 3);
        assert_eq!(completed.task.parent.status, ParentStatus::Completed);
        assert!(completed
            .task
            .children
            .iter()
            .all(|child| child.status == ChildStatus::Completed));

        service.mark_canceled(&created.task_id, 400).unwrap();
        let canceled = service.store().load_bundle(&created.task_id).unwrap();
        assert_eq!(canceled.task.parent.status, ParentStatus::Canceled);
        assert!(canceled
            .task
            .children
            .iter()
            .all(|child| child.status == ChildStatus::Canceled));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_source_is_rejected_without_creating_task() {
        let root = std::env::temp_dir().join(format!("task-service-{}", uuid::Uuid::new_v4()));
        let service = TaskService::new(&root);
        assert!(service
            .create_task(Path::new("definitely-missing-video.mp4"), config(), 100)
            .is_err());
        assert!(!root.exists());
    }
}
