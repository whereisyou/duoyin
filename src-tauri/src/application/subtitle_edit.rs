use std::fs;
use std::path::{Path, PathBuf};

use crate::domain::artifact::{ArtifactKind, ArtifactRecord, ArtifactStatus, RetentionPolicy};
use crate::domain::ids::{ArtifactId, StageId, TaskId, VariantId};
use crate::domain::manifest::{StageRecord, StageStatus};
use crate::infra::artifact_store::ArtifactStore;
use crate::infra::task_store::TaskStore;
use crate::types::Segment;

pub fn load_segments(
    store: &TaskStore,
    task_id: &TaskId,
    variant_id: Option<&VariantId>,
) -> Result<Vec<Segment>, String> {
    let root = store.task_dir(task_id).map_err(|error| error.to_string())?;
    let path = segment_path(&root, variant_id);
    let bytes =
        fs::read(&path).map_err(|error| format!("读取字幕数据失败 {}: {error}", path.display()))?;
    serde_json::from_slice(&bytes).map_err(|error| format!("字幕 JSON 无效: {error}"))
}

pub fn save_segments(
    store: &TaskStore,
    task_id: &TaskId,
    variant_id: Option<&VariantId>,
    segments: &[Segment],
) -> Result<(), String> {
    validate_segments(segments, variant_id.is_some())?;
    let mut loaded = store
        .load_bundle(task_id)
        .map_err(|error| error.to_string())?;
    if let Some(variant) = variant_id {
        if !loaded
            .task
            .config
            .targets
            .iter()
            .any(|target| &target.id == variant)
        {
            return Err(format!("目标版本不存在: {}", variant.0));
        }
    }
    let root = store.task_dir(task_id).map_err(|error| error.to_string())?;
    let path = segment_path(&root, variant_id);
    write_atomic(
        &path,
        &serde_json::to_vec_pretty(segments).map_err(|error| error.to_string())?,
    )?;
    let relative = path
        .strip_prefix(&root)
        .map_err(|_| "字幕路径逃逸任务目录")?
        .to_owned();
    let artifact_store = ArtifactStore::new(&root);
    let metadata = fs::metadata(&path).map_err(|error| error.to_string())?;
    let (artifact_id, kind, stage_id, node_key) = match variant_id {
        Some(variant) => (
            ArtifactId(format!("target:{}:translate:edited", variant.0)),
            ArtifactKind::TranslatedSegments,
            StageId("translate".into()),
            format!("target:{}:translate", variant.0),
        ),
        None => (
            ArtifactId("parent:stt:edited".into()),
            ArtifactKind::Segments,
            StageId("stt".into()),
            "parent:stt".into(),
        ),
    };
    loaded.manifest.artifacts.insert(
        artifact_id.clone(),
        ArtifactRecord {
            id: artifact_id.clone(),
            kind,
            producer_stage_id: stage_id.clone(),
            status: ArtifactStatus::Valid,
            retention: RetentionPolicy::RequiredForResume,
            relative_path: relative.clone(),
            size: metadata.len(),
            modified: metadata
                .modified()
                .ok()
                .and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok())
                .and_then(|value| i64::try_from(value.as_millis()).ok())
                .unwrap_or(0),
            content_hash: Some(
                artifact_store
                    .hash_file(&relative)
                    .map_err(|error| error.to_string())?,
            ),
            media_type: Some("application/json".into()),
            schema_version: Some(1),
        },
    );
    let mut edited = StageRecord::done(&stage_id.0, "user-edited", vec![artifact_id]);
    edited.node_key = node_key;
    edited.engine_version = "user-edit-v1".into();
    edited.external_override = true;

    match variant_id {
        Some(variant) => {
            let stages = loaded
                .manifest
                .target_stages
                .entry(variant.clone())
                .or_default();
            stages.insert(stage_id, edited);
            invalidate(stages, &["tts", "mix", "srt", "final_video"]);
        }
        None => {
            loaded.manifest.stages.insert(stage_id, edited);
            for stages in loaded.manifest.target_stages.values_mut() {
                invalidate(stages, &["translate", "tts", "mix", "srt", "final_video"]);
            }
        }
    }
    store
        .save_bundle(&mut loaded.task, &loaded.manifest)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn segment_path(root: &Path, variant_id: Option<&VariantId>) -> PathBuf {
    match variant_id {
        Some(variant) => root
            .join("targets")
            .join(&variant.0)
            .join("translated.json"),
        None => root.join("shared").join("segments.json"),
    }
}

fn invalidate(stages: &mut std::collections::BTreeMap<StageId, StageRecord>, names: &[&str]) {
    for name in names {
        if let Some(stage) = stages.get_mut(&StageId((*name).into())) {
            stage.status = StageStatus::Invalidated;
            stage.error = None;
        }
    }
}

fn validate_segments(segments: &[Segment], translated: bool) -> Result<(), String> {
    if segments.is_empty() {
        return Err("字幕不能为空".into());
    }
    for (index, segment) in segments.iter().enumerate() {
        if !segment.start.is_finite()
            || !segment.end.is_finite()
            || segment.start < 0.0
            || segment.end <= segment.start
        {
            return Err(format!("第 {} 条字幕时间范围无效", index + 1));
        }
        if segment.text.trim().is_empty() && !translated {
            return Err(format!("第 {} 条原文为空", index + 1));
        }
        if translated && segment.translated.trim().is_empty() {
            return Err(format!("第 {} 条译文为空", index + 1));
        }
    }
    Ok(())
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path.parent().ok_or("字幕文件没有父目录")?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let temp = path.with_extension("json.tmp");
    fs::write(&temp, bytes).map_err(|error| error.to_string())?;
    if path.exists() {
        fs::remove_file(path).map_err(|error| error.to_string())?;
    }
    fs::rename(temp, path).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::task_service::TaskService;
    use crate::domain::config::{EngineSelection, OutputConfig, PipelineConfig, SeparationConfig};
    use crate::domain::variant::TargetVariant;

    fn setup() -> (
        std::path::PathBuf,
        TaskService,
        crate::application::task_service::CreatedTask,
    ) {
        let root = std::env::temp_dir().join(format!("subtitle-edit-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let source = root.join("video.mp4");
        fs::write(&source, b"video").unwrap();
        let service = TaskService::new(root.join("data"));
        let created = service
            .create_task(
                &source,
                PipelineConfig {
                    source_language: Some("en".into()),
                    targets: vec![TargetVariant::language("ja").unwrap()],
                    engines: EngineSelection {
                        stt: "fake".into(),
                        translator: "fake".into(),
                        tts: "fake".into(),
                        separator: None,
                    },
                    separation: SeparationConfig::default(),
                    output: OutputConfig::default(),
                },
                1,
            )
            .unwrap();
        (root, service, created)
    }

    #[test]
    fn editing_parent_source_invalidates_all_target_translation() {
        let (root, service, created) = setup();
        let mut loaded = service.store().load_bundle(&created.task_id).unwrap();
        loaded
            .manifest
            .target_stages
            .entry(VariantId("ja".into()))
            .or_default()
            .insert(
                StageId("translate".into()),
                StageRecord::done("translate", "h", vec![]),
            );
        service
            .store()
            .save_bundle(&mut loaded.task, &loaded.manifest)
            .unwrap();
        save_segments(
            &service.store(),
            &created.task_id,
            None,
            &[Segment {
                idx: 0,
                start: 0.0,
                end: 1.0,
                text: "edited".into(),
                translated: String::new(),
            }],
        )
        .unwrap();
        let loaded = service.store().load_bundle(&created.task_id).unwrap();
        assert!(loaded.manifest.stages[&StageId("stt".into())].external_override);
        assert_eq!(
            loaded.manifest.target_stages[&VariantId("ja".into())][&StageId("translate".into())]
                .status,
            StageStatus::Invalidated
        );
        fs::remove_dir_all(root).unwrap();
    }
}
