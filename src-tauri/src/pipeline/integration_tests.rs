#![cfg(test)]

use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use crate::adapters::media::output_stages::FfmpegOutputStages;
use crate::adapters::translate::stage::TranslateStageExecutor;
use crate::adapters::tts::stage::TtsStageExecutor;
use crate::domain::artifact::{ArtifactKind, ArtifactRecord, RetentionPolicy};
use crate::domain::config::{EngineSelection, OutputConfig, PipelineConfig, SeparationConfig};
use crate::domain::ids::{ArtifactId, StageId, TaskId, VariantId};
use crate::domain::manifest::{StageRecord, StageStatus, TaskManifest};
use crate::domain::media::SourceFingerprint;
use crate::domain::variant::TargetVariant;
use crate::pipeline::graph::PipelineGraph;
use crate::pipeline::registry::StageRegistry;
use crate::pipeline::runner::{CancelToken, PipelineRunner};
use crate::ports::translator::{TranslateFuture, Translator};
use crate::ports::tts::{TtsAlignment, TtsEngine, TtsFuture, TtsOutput};
use crate::types::Segment;

struct FakeTranslator;

impl Translator for FakeTranslator {
    fn version(&self) -> String {
        "fake-translator-v1".into()
    }

    fn translate<'a>(
        &'a self,
        segments: &'a [Segment],
        _source_language: Option<&'a str>,
        target: &'a TargetVariant,
        _cancel: &'a CancelToken,
    ) -> TranslateFuture<'a> {
        Box::pin(async move {
            let mut output = segments.to_vec();
            for segment in &mut output {
                segment.translated = format!("{}:{}", target.id.0, segment.text);
            }
            Ok(output)
        })
    }
}

struct FfmpegFakeTts;

impl TtsEngine for FfmpegFakeTts {
    fn version(&self) -> String {
        "ffmpeg-fake-tts-v1".into()
    }

    fn synthesize<'a>(
        &'a self,
        _segments: &'a [Segment],
        target: &'a TargetVariant,
        output_dir: &'a Path,
        _alignment: TtsAlignment,
        cancel: &'a CancelToken,
    ) -> TtsFuture<'a> {
        Box::pin(async move {
            if cancel.is_canceled() {
                return Err(crate::ports::tts::TtsError::Canceled);
            }
            tokio::fs::create_dir_all(output_dir).await.unwrap();
            let output = output_dir.join("dub.wav");
            let frequency = if target.id.0 == "zh-yue" {
                "660"
            } else {
                "880"
            };
            let status = tokio::process::Command::new("ffmpeg")
                .args([
                    "-v",
                    "error",
                    "-f",
                    "lavfi",
                    "-i",
                    &format!("sine=frequency={frequency}:duration=0.2"),
                    "-acodec",
                    "pcm_s16le",
                    "-y",
                ])
                .arg(&output)
                .status()
                .await
                .unwrap();
            assert!(status.success());
            Ok(TtsOutput {
                dub_audio: output,
                segment_dir: None,
            })
        })
    }
}

fn modified(path: &Path) -> i64 {
    fs::metadata(path)
        .unwrap()
        .modified()
        .unwrap()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

fn parent_manifest(root: &Path, source: &Path) -> TaskManifest {
    let segments_path = root.join("shared/segments.json");
    fs::create_dir_all(segments_path.parent().unwrap()).unwrap();
    fs::write(
        &segments_path,
        serde_json::to_vec(&vec![Segment {
            idx: 0,
            start: 0.0,
            end: 0.2,
            text: "hello".into(),
            translated: String::new(),
        }])
        .unwrap(),
    )
    .unwrap();
    let source_metadata = fs::metadata(source).unwrap();
    let mut manifest = TaskManifest::new(
        TaskId("p1".into()),
        SourceFingerprint {
            size: source_metadata.len(),
            modified: modified(source),
            content_hash: Some("source-hash".into()),
            hash_algo_version: 1,
        },
    );
    let artifact = ArtifactRecord {
        id: ArtifactId("parent:stt:0".into()),
        kind: ArtifactKind::Segments,
        producer_stage_id: StageId("stt".into()),
        status: crate::domain::artifact::ArtifactStatus::Valid,
        retention: RetentionPolicy::RequiredForResume,
        relative_path: "shared/segments.json".into(),
        size: fs::metadata(&segments_path).unwrap().len(),
        modified: modified(&segments_path),
        content_hash: Some("segments-hash".into()),
        media_type: Some("application/json".into()),
        schema_version: Some(1),
    };
    manifest.add_artifact(artifact);
    manifest.add_stage(StageRecord::done(
        "stt",
        "stt-committed",
        vec![ArtifactId("parent:stt:0".into())],
    ));
    let mut separation = StageRecord::done("separation", "separation-disabled", vec![]);
    separation.status = StageStatus::Skipped;
    manifest.add_stage(separation);
    manifest
}

fn config(targets: Vec<TargetVariant>) -> PipelineConfig {
    PipelineConfig {
        source_language: Some("en".into()),
        targets,
        engines: EngineSelection {
            stt: "precomputed".into(),
            translator: "fake".into(),
            tts: "fake".into(),
            separator: None,
        },
        separation: SeparationConfig::default(),
        output: OutputConfig {
            generate_final_videos: true,
            ..OutputConfig::default()
        },
    }
}

#[tokio::test]
async fn target_pipeline_produces_independent_complete_versions() {
    let root = std::env::temp_dir().join(format!("target-pipeline-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    let source = root.join("source.mp4");
    let status = tokio::process::Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=c=black:s=160x90:d=0.2",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=0.2",
            "-shortest",
            "-c:v",
            "libx264",
            "-c:a",
            "aac",
            "-y",
        ])
        .arg(&source)
        .status()
        .await
        .unwrap();
    assert!(status.success());

    let mandarin = TargetVariant::zh_mandarin();
    let yue = TargetVariant::zh_dialect("yue", "粤语", "请用广东话表达。");
    let targets = vec![mandarin.clone(), yue.clone()];
    let mut registry = StageRegistry::new();
    registry
        .register(
            "translate",
            Arc::new(TranslateStageExecutor::new(
                Arc::new(FakeTranslator),
                Some("en".into()),
                targets.clone(),
            )),
        )
        .unwrap();
    registry
        .register(
            "tts",
            Arc::new(TtsStageExecutor::new(
                Arc::new(FfmpegFakeTts),
                targets.clone(),
                TtsAlignment {
                    min_speed_percent: 85,
                    max_speed_percent: 125,
                },
                false,
            )),
        )
        .unwrap();
    let outputs = Arc::new(FfmpegOutputStages::default());
    for stage in ["mix", "srt", "final_video"] {
        registry.register(stage, outputs.clone()).unwrap();
    }
    let runner = PipelineRunner::new(
        PipelineGraph::video_translation(),
        config(targets),
        parent_manifest(&root, &source),
        Arc::new(registry),
    )
    .with_environment(&root, &source);
    let cancel = CancelToken::default();

    let results = runner
        .run_targets(
            &[VariantId("zh-CN".into()), VariantId("zh-yue".into())],
            &cancel,
        )
        .await;

    assert!(results.values().all(Result::is_ok));
    for variant in ["zh-CN", "zh-yue"] {
        let directory = root.join("targets").join(variant);
        for file in [
            "translated.json".to_string(),
            "dub.wav".to_string(),
            "mixed.wav".to_string(),
            "translated.srt".to_string(),
            format!("source.{variant}.mp4"),
        ] {
            assert!(directory.join(&file).is_file(), "missing {variant}/{file}");
        }
    }
    let snapshot = runner.manifest_snapshot().await;
    assert_eq!(snapshot.target_stages.len(), 2);
    assert_eq!(
        snapshot.target_stages[&VariantId("zh-yue".into())][&StageId("final_video".into())].status,
        StageStatus::Done
    );
    fs::remove_dir_all(root).unwrap();
}
