use std::fs;
use std::path::Path;

use crate::domain::artifact::{ArtifactKind, ArtifactRecord, ArtifactStatus, RetentionPolicy};
use crate::domain::ids::{ArtifactId, StageId, TaskId, VariantId};
use crate::domain::manifest::{StageRecord, StageStatus};
use crate::infra::artifact_store::ArtifactStore;
use crate::infra::task_store::TaskStore;
use crate::types::Segment;

pub fn import_target_srt(
    store: &TaskStore,
    task_id: &TaskId,
    variant_id: &VariantId,
    source_srt: &Path,
) -> Result<(), String> {
    let mut loaded = store
        .load_bundle(task_id)
        .map_err(|error| error.to_string())?;
    if !loaded
        .task
        .config
        .targets
        .iter()
        .any(|target| &target.id == variant_id)
    {
        return Err(format!("目标版本不存在: {}", variant_id.0));
    }
    let content = fs::read_to_string(source_srt).map_err(|error| error.to_string())?;
    let mut imported = crate::subtitle_parse::parse_srt(&content)?;
    let task_root = store.task_dir(task_id).map_err(|error| error.to_string())?;
    let parent_segments = task_root.join("shared/segments.json");
    if let Ok(bytes) = fs::read(parent_segments) {
        if let Ok(source) = serde_json::from_slice::<Vec<Segment>>(&bytes) {
            for (segment, original) in imported.iter_mut().zip(source) {
                segment.text = original.text;
            }
        }
    }
    let target_dir = task_root.join("targets").join(&variant_id.0);
    fs::create_dir_all(&target_dir).map_err(|error| error.to_string())?;
    let srt_relative = format!("targets/{}/translated.srt", variant_id.0);
    let json_relative = format!("targets/{}/translated.json", variant_id.0);
    write_atomic(&task_root.join(&srt_relative), content.as_bytes())?;
    write_atomic(
        &task_root.join(&json_relative),
        &serde_json::to_vec_pretty(&imported).map_err(|error| error.to_string())?,
    )?;

    let artifact_store = ArtifactStore::new(&task_root);
    let json_id = ArtifactId(format!("target:{}:translate:external", variant_id.0));
    let srt_id = ArtifactId(format!("target:{}:srt:external", variant_id.0));
    for (id, kind, relative, media_type) in [
        (
            json_id.clone(),
            ArtifactKind::TranslatedSegments,
            json_relative,
            "application/json",
        ),
        (
            srt_id.clone(),
            ArtifactKind::SubtitleSrt,
            srt_relative,
            "application/x-subrip",
        ),
    ] {
        let path = task_root.join(&relative);
        let metadata = fs::metadata(&path).map_err(|error| error.to_string())?;
        loaded.manifest.artifacts.insert(
            id.clone(),
            ArtifactRecord {
                id,
                kind,
                producer_stage_id: StageId("translate".into()),
                status: ArtifactStatus::Valid,
                retention: RetentionPolicy::RequiredForResume,
                relative_path: relative.into(),
                size: metadata.len(),
                modified: metadata
                    .modified()
                    .ok()
                    .and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok())
                    .and_then(|value| i64::try_from(value.as_millis()).ok())
                    .unwrap_or(0),
                content_hash: Some(
                    artifact_store
                        .hash_file(&path.strip_prefix(&task_root).unwrap_or(&path))
                        .map_err(|error| error.to_string())?,
                ),
                media_type: Some(media_type.into()),
                schema_version: Some(1),
            },
        );
    }
    let stages = loaded
        .manifest
        .target_stages
        .entry(variant_id.clone())
        .or_default();
    let mut translate = StageRecord::done("translate", "external-srt", vec![json_id]);
    translate.node_key = format!("target:{}:translate", variant_id.0);
    translate.engine_version = "external-srt-v1".into();
    translate.external_override = true;
    stages.insert(StageId("translate".into()), translate);
    let mut srt = StageRecord::done("srt", "external-srt", vec![srt_id]);
    srt.node_key = format!("target:{}:srt", variant_id.0);
    srt.engine_version = "external-srt-v1".into();
    srt.external_override = true;
    stages.insert(StageId("srt".into()), srt);
    for stage in ["tts", "mix", "final_video"] {
        if let Some(record) = stages.get_mut(&StageId(stage.into())) {
            record.status = StageStatus::Invalidated;
        }
    }
    store
        .save_bundle(&mut loaded.task, &loaded.manifest)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let temp = path.with_extension("tmp");
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

    #[test]
    fn imported_srt_becomes_external_translate_override() {
        let root = std::env::temp_dir().join(format!("srt-import-{}", uuid::Uuid::new_v4()));
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
        let srt = root.join("import.srt");
        fs::write(&srt, "1\n00:00:00,000 --> 00:00:01,000\nこんにちは\n").unwrap();

        import_target_srt(
            &service.store(),
            &created.task_id,
            &VariantId("ja".into()),
            &srt,
        )
        .unwrap();

        let loaded = service.store().load_bundle(&created.task_id).unwrap();
        let translate =
            &loaded.manifest.target_stages[&VariantId("ja".into())][&StageId("translate".into())];
        assert!(translate.external_override);
        assert_eq!(translate.status, StageStatus::Done);
        assert!(created
            .task_root
            .join("targets/ja/translated.json")
            .is_file());
        fs::remove_dir_all(root).unwrap();
    }
}
