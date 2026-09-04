//! PipelineRunner 测试。必须是 runner 的子模块（`super::*` 拿 re-export 的 pub 类型），
//! 否则访问不到 `pub(in crate::pipeline::runner)` 级别的内部项（如 RunScope::node_key）。

use std::collections::BTreeMap;
use std::fs;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;

use super::*;
use crate::domain::artifact::{ArtifactKind, ArtifactRecord, ArtifactStatus, RetentionPolicy};
use crate::domain::config::{EngineSelection, OutputConfig, PipelineConfig, SeparationConfig};
use crate::domain::ids::{ArtifactId, ChildTaskId, StageId, TaskId, VariantId};
use crate::domain::manifest::{StageRecord, StageStatus, TaskManifest};
use crate::domain::media::{SourceFingerprint, SourceVideo};
use crate::domain::task::{ChildStatus, ParentStatus, ParentTask};
use crate::domain::variant::TargetVariant;
use crate::infra::artifact_store::ArtifactStore;
use crate::infra::task_store::{TaskDocument, TaskStore};
use crate::pipeline::graph::PipelineGraph;

#[derive(Default)]
struct FakeExecutor {
    calls: Mutex<BTreeMap<String, usize>>,
    delay_ms: u64,
    fail_stage: Option<String>,
    fail_node_key: Option<String>,
    version: Option<String>,
    active: AtomicUsize,
    max_active: AtomicUsize,
}

impl FakeExecutor {
    async fn count(&self, key: &str) -> usize {
        *self.calls.lock().await.get(key).unwrap_or(&0)
    }
}

impl StageExecutor for FakeExecutor {
    fn version(&self, _stage: &StageId) -> String {
        self.version.clone().unwrap_or_else(|| "fake-v1".into())
    }

    fn execute<'a>(
        &'a self,
        request: StageRequest,
        context: ExecutionContext,
    ) -> ExecuteFuture<'a> {
        Box::pin(async move {
            let node = request.node;
            let scope = request.scope;
            let key = scope.node_key(&node.id);
            *self.calls.lock().await.entry(key.clone()).or_default() += 1;
            let active = self.active.fetch_add(1, Ordering::AcqRel) + 1;
            self.max_active.fetch_max(active, Ordering::AcqRel);
            if self.delay_ms > 0 {
                tokio::time::sleep(Duration::from_millis(self.delay_ms)).await;
            }
            self.active.fetch_sub(1, Ordering::AcqRel);
            if context.cancel.is_canceled() {
                return Err(ExecuteError::Canceled);
            }
            if self.fail_stage.as_deref() == Some(node.id.0.as_str())
                || self.fail_node_key.as_deref() == Some(key.as_str())
            {
                return Err(ExecuteError::Failed("fake failure".into()));
            }
            let outputs = node
                .outputs
                .iter()
                .enumerate()
                .map(|(index, kind)| ArtifactOutput {
                    id: ArtifactId(format!("{key}:{index}")),
                    kind: kind.clone(),
                    relative_path: format!("{}/{}.bin", key.replace(':', "/"), index),
                    size: 1,
                    modified: 1,
                    content_hash: format!("hash:{key}:{index}"),
                    media_type: None,
                    retention: RetentionPolicy::RequiredForResume,
                })
                .collect();
            Ok(ExecutionOutcome::Done(outputs))
        })
    }
}

fn config() -> PipelineConfig {
    PipelineConfig {
        source_language: None,
        targets: vec![TargetVariant::zh_mandarin()],
        engines: EngineSelection {
            stt: "fake".into(),
            translator: "fake".into(),
            tts: "fake".into(),
            separator: Some("fake".into()),
        },
        separation: SeparationConfig::default(),
        output: OutputConfig::default(),
    }
}

#[derive(Default)]
struct RecordingCheckpoint {
    statuses: Mutex<Vec<Vec<StageStatus>>>,
    fail: bool,
}

impl PipelineCheckpoint for RecordingCheckpoint {
    fn save<'a>(&'a self, manifest: TaskManifest) -> CheckpointFuture<'a> {
        Box::pin(async move {
            if self.fail {
                return Err("disk full".into());
            }
            let mut statuses: Vec<_> = manifest
                .stages
                .values()
                .map(|stage| stage.status.clone())
                .collect();
            for stages in manifest.target_stages.values() {
                statuses.extend(stages.values().map(|stage| stage.status.clone()));
            }
            self.statuses.lock().await.push(statuses);
            Ok(())
        })
    }
}

fn manifest() -> TaskManifest {
    TaskManifest::new(
        TaskId("p1".into()),
        SourceFingerprint {
            size: 1,
            modified: 1,
            content_hash: Some("source".into()),
            hash_algo_version: 1,
        },
    )
}

#[tokio::test]
async fn first_run_executes_and_second_run_reuses() {
    let executor = Arc::new(FakeExecutor::default());
    let runner = PipelineRunner::new(
        PipelineGraph::video_translation(),
        config(),
        manifest(),
        executor.clone(),
    );
    let cancel = CancelToken::default();

    runner.run_parent(&cancel).await.unwrap();
    runner.run_parent(&cancel).await.unwrap();

    assert_eq!(executor.count("parent:media_probe").await, 1);
    assert_eq!(executor.count("parent:stt").await, 1);
    let snapshot = runner.manifest_snapshot().await;
    assert_eq!(
        snapshot.stages[&StageId("separation".into())].status,
        StageStatus::Skipped
    );
}

#[tokio::test]
async fn checkpoints_running_and_done_boundaries() {
    let checkpoint = Arc::new(RecordingCheckpoint::default());
    let runner = PipelineRunner::new(
        PipelineGraph::video_translation(),
        config(),
        manifest(),
        Arc::new(FakeExecutor::default()),
    )
    .with_checkpoint(checkpoint.clone());

    runner
        .run_named(RunScope::Parent, "media_probe", &CancelToken::default())
        .await
        .unwrap();

    let snapshots = checkpoint.statuses.lock().await;
    assert!(snapshots
        .iter()
        .any(|states| states.contains(&StageStatus::Running)));
    assert!(snapshots
        .iter()
        .any(|states| states.contains(&StageStatus::Done)));
}

#[tokio::test]
async fn checkpoint_failure_prevents_executor_start() {
    let checkpoint = Arc::new(RecordingCheckpoint {
        fail: true,
        ..Default::default()
    });
    let executor = Arc::new(FakeExecutor::default());
    let runner = PipelineRunner::new(
        PipelineGraph::video_translation(),
        config(),
        manifest(),
        executor.clone(),
    )
    .with_checkpoint(checkpoint);

    assert!(matches!(
        runner
            .run_named(RunScope::Parent, "media_probe", &CancelToken::default(),)
            .await,
        Err(PipelineError::Checkpoint(_))
    ));
    assert_eq!(executor.count("parent:media_probe").await, 0);
}

#[tokio::test]
async fn source_fingerprint_change_invalidates_source_consumers() {
    let first_executor = Arc::new(FakeExecutor::default());
    let first = PipelineRunner::new(
        PipelineGraph::video_translation(),
        config(),
        manifest(),
        first_executor,
    );
    let cancel = CancelToken::default();
    first
        .run_named(RunScope::Parent, "media_probe", &cancel)
        .await
        .unwrap();
    let mut committed = first.manifest_snapshot().await;
    committed.source_fingerprint.modified += 1;

    let restarted_executor = Arc::new(FakeExecutor::default());
    let restarted = PipelineRunner::new(
        PipelineGraph::video_translation(),
        config(),
        committed,
        restarted_executor.clone(),
    );
    restarted
        .run_named(RunScope::Parent, "media_probe", &cancel)
        .await
        .unwrap();

    assert_eq!(restarted_executor.count("parent:media_probe").await, 1);
}

#[tokio::test]
async fn engine_version_change_invalidates_reuse() {
    let first = PipelineRunner::new(
        PipelineGraph::video_translation(),
        config(),
        manifest(),
        Arc::new(FakeExecutor::default()),
    );
    let cancel = CancelToken::default();
    first
        .run_named(RunScope::Parent, "media_probe", &cancel)
        .await
        .unwrap();
    let committed = first.manifest_snapshot().await;
    assert_eq!(
        committed.stages[&StageId("media_probe".into())].engine_version,
        "fake-v1"
    );

    let upgraded_executor = Arc::new(FakeExecutor {
        version: Some("fake-v2".into()),
        ..Default::default()
    });
    let upgraded = PipelineRunner::new(
        PipelineGraph::video_translation(),
        config(),
        committed,
        upgraded_executor.clone(),
    );
    upgraded
        .run_named(RunScope::Parent, "media_probe", &cancel)
        .await
        .unwrap();

    assert_eq!(upgraded_executor.count("parent:media_probe").await, 1);
    assert_eq!(
        upgraded.manifest_snapshot().await.stages[&StageId("media_probe".into())]
            .engine_version,
        "fake-v2"
    );
}

#[tokio::test]
async fn concurrent_same_node_is_single_flight() {
    let executor = Arc::new(FakeExecutor {
        delay_ms: 30,
        ..Default::default()
    });
    let runner = Arc::new(PipelineRunner::new(
        PipelineGraph::video_translation(),
        config(),
        manifest(),
        executor.clone(),
    ));
    let cancel = CancelToken::default();
    runner
        .run_named(RunScope::Parent, "media_probe", &cancel)
        .await
        .unwrap();

    let a = {
        let runner = runner.clone();
        let cancel = cancel.clone();
        tokio::spawn(async move { runner.run_named(RunScope::Parent, "extract_audio", &cancel).await })
    };
    let b = {
        let runner = runner.clone();
        let cancel = cancel.clone();
        tokio::spawn(async move { runner.run_named(RunScope::Parent, "extract_audio", &cancel).await })
    };
    a.await.unwrap().unwrap();
    b.await.unwrap().unwrap();

    assert_eq!(executor.count("parent:extract_audio").await, 1);
}

#[tokio::test]
async fn one_target_failure_does_not_touch_other_target() {
    let executor = Arc::new(FakeExecutor {
        fail_stage: Some("translate".into()),
        ..Default::default()
    });
    let runner = PipelineRunner::new(
        PipelineGraph::video_translation(),
        config(),
        manifest(),
        executor,
    );
    let cancel = CancelToken::default();
    runner.run_parent(&cancel).await.unwrap();

    assert!(runner
        .run_target(VariantId("zh-CN".into()), &cancel)
        .await
        .is_err());
    let snapshot = runner.manifest_snapshot().await;
    assert_eq!(
        snapshot.target_stages[&VariantId("zh-CN".into())][&StageId("translate".into())].status,
        StageStatus::Failed
    );
    assert!(!snapshot
        .target_stages
        .contains_key(&VariantId("zh-yue".into())));
}

#[tokio::test]
async fn target_cancel_keeps_parent_artifacts_done() {
    let executor = Arc::new(FakeExecutor::default());
    let runner = PipelineRunner::new(
        PipelineGraph::video_translation(),
        config(),
        manifest(),
        executor,
    );
    let parent_cancel = CancelToken::default();
    runner.run_parent(&parent_cancel).await.unwrap();
    let child_cancel = CancelToken::default();
    child_cancel.cancel();

    assert!(runner
        .run_target(VariantId("zh-CN".into()), &child_cancel)
        .await
        .is_err());
    let snapshot = runner.manifest_snapshot().await;
    assert_eq!(
        snapshot.stages[&StageId("stt".into())].status,
        StageStatus::Done
    );
    assert_eq!(
        snapshot.target_stages[&VariantId("zh-CN".into())][&StageId("translate".into())].status,
        StageStatus::Canceled
    );
}

#[tokio::test]
async fn parent_stt_invalidation_propagates_to_all_targets() {
    let executor = Arc::new(FakeExecutor::default());
    let mut cfg = config();
    cfg.targets
        .push(TargetVariant::zh_dialect("yue", "粤语", "广东话"));
    let runner = PipelineRunner::new(
        PipelineGraph::video_translation(),
        cfg,
        manifest(),
        executor,
    );
    let cancel = CancelToken::default();
    runner.run_parent(&cancel).await.unwrap();
    runner
        .run_target(VariantId("zh-CN".into()), &cancel)
        .await
        .unwrap();
    runner
        .run_target(VariantId("zh-yue".into()), &cancel)
        .await
        .unwrap();

    runner
        .invalidate_from(RunScope::Parent, "stt")
        .await
        .unwrap();
    let snapshot = runner.manifest_snapshot().await;
    assert_eq!(
        snapshot.stages[&StageId("stt".into())].status,
        StageStatus::Invalidated
    );
    for variant in [VariantId("zh-CN".into()), VariantId("zh-yue".into())] {
        assert_eq!(
            snapshot.target_stages[&variant][&StageId("translate".into())].status,
            StageStatus::Invalidated
        );
        assert_eq!(
            snapshot.target_stages[&variant][&StageId("final_video".into())].status,
            StageStatus::Invalidated
        );
    }
}

#[tokio::test]
async fn restart_reuses_committed_manifest() {
    let first = PipelineRunner::new(
        PipelineGraph::video_translation(),
        config(),
        manifest(),
        Arc::new(FakeExecutor::default()),
    );
    let cancel = CancelToken::default();
    first.run_parent(&cancel).await.unwrap();
    let committed = first.manifest_snapshot().await;

    let restarted_executor = Arc::new(FakeExecutor::default());
    let restarted = PipelineRunner::new(
        PipelineGraph::video_translation(),
        config(),
        committed,
        restarted_executor.clone(),
    );
    restarted.run_parent(&cancel).await.unwrap();

    assert_eq!(restarted_executor.count("parent:media_probe").await, 0);
    assert_eq!(restarted_executor.count("parent:stt").await, 0);
}

#[tokio::test]
async fn disk_restart_reuses_task_store_manifest() {
    let root =
        std::env::temp_dir().join(format!("videotrans-pipeline-{}", uuid::Uuid::new_v4()));
    let task_store = TaskStore::new(&root);
    let variant = TargetVariant::zh_mandarin();
    let task_id = TaskId("p1".into());
    let child_id = ChildTaskId("p1-zh-CN".into());
    let parent = ParentTask {
        id: task_id.clone(),
        source: SourceVideo {
            path: "input.mp4".into(),
            fingerprint: manifest().source_fingerprint,
        },
        status: ParentStatus::Pending,
        children: vec![child_id.clone()],
        created_at: 1,
        updated_at: 1,
    };
    let child = crate::domain::task::ChildTask {
        id: child_id,
        parent_id: task_id.clone(),
        variant,
        status: ChildStatus::Pending,
        created_at: 1,
        updated_at: 1,
    };
    let first = PipelineRunner::new(
        PipelineGraph::video_translation(),
        config(),
        manifest(),
        Arc::new(FakeExecutor::default()),
    );
    let cancel = CancelToken::default();
    first.run_parent(&cancel).await.unwrap();
    let committed = first.manifest_snapshot().await;
    let mut document = TaskDocument::new(parent, vec![child], config());
    task_store.save_bundle(&mut document, &committed).unwrap();

    let loaded = task_store.load_bundle(&task_id).unwrap();
    let restarted_executor = Arc::new(FakeExecutor::default());
    let restarted = PipelineRunner::new(
        PipelineGraph::video_translation(),
        loaded.task.config,
        loaded.manifest,
        restarted_executor.clone(),
    );
    restarted.run_parent(&cancel).await.unwrap();

    assert_eq!(restarted_executor.count("parent:stt").await, 0);
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn enabled_separation_runs_in_parallel_with_stt() {
    let executor = Arc::new(FakeExecutor {
        delay_ms: 30,
        ..Default::default()
    });
    let mut cfg = config();
    cfg.separation.enabled = true;
    let runner = PipelineRunner::new(
        PipelineGraph::video_translation(),
        cfg,
        manifest(),
        executor.clone(),
    );

    runner.run_parent(&CancelToken::default()).await.unwrap();

    assert!(executor.max_active.load(Ordering::Acquire) >= 2);
    assert_eq!(executor.count("parent:stt").await, 1);
    assert_eq!(executor.count("parent:separation").await, 1);
}

#[tokio::test]
async fn separation_failure_degrades_to_no_bgm_when_enabled() {
    let executor = Arc::new(FakeExecutor {
        fail_stage: Some("separation".into()),
        ..Default::default()
    });
    let mut cfg = config();
    cfg.separation.enabled = true;
    cfg.separation.allow_no_bgm_fallback = true;
    let runner = PipelineRunner::new(
        PipelineGraph::video_translation(),
        cfg,
        manifest(),
        executor,
    );

    runner.run_parent(&CancelToken::default()).await.unwrap();
    let snapshot = runner.manifest_snapshot().await;
    let separation = &snapshot.stages[&StageId("separation".into())];
    assert_eq!(separation.status, StageStatus::Degraded);
    assert_eq!(separation.fallback.as_ref().unwrap().to, "no_bgm");
}

#[tokio::test]
async fn target_versions_are_enqueued_together_and_executor_controls_concurrency() {
    let executor = Arc::new(FakeExecutor {
        delay_ms: 20,
        ..Default::default()
    });
    let runner = PipelineRunner::new(
        PipelineGraph::video_translation(),
        config(),
        manifest(),
        executor.clone(),
    );
    let cancel = CancelToken::default();
    runner.run_parent(&cancel).await.unwrap();
    let tokens = BTreeMap::from([
        (VariantId("zh-CN".into()), CancelToken::default()),
        (VariantId("ja".into()), CancelToken::default()),
    ]);

    let results = runner.run_targets_with_tokens(&tokens).await;

    assert!(results.values().all(Result::is_ok));
    assert!(executor.max_active.load(Ordering::Acquire) >= 2);
}

#[tokio::test]
async fn batch_targets_continue_after_one_variant_fails() {
    let executor = Arc::new(FakeExecutor {
        fail_node_key: Some("target:zh-CN:translate".into()),
        ..Default::default()
    });
    let runner = PipelineRunner::new(
        PipelineGraph::video_translation(),
        config(),
        manifest(),
        executor.clone(),
    );
    let cancel = CancelToken::default();
    runner.run_parent(&cancel).await.unwrap();
    let variants = [VariantId("zh-CN".into()), VariantId("zh-yue".into())];

    let results = runner.run_targets(&variants, &cancel).await;

    assert!(results[&VariantId("zh-CN".into())].is_err());
    assert!(results[&VariantId("zh-yue".into())].is_ok());
    assert_eq!(executor.count("target:zh-yue:translate").await, 1);
    assert_eq!(executor.count("target:zh-yue:tts").await, 1);
}

#[tokio::test]
async fn child_translation_edit_invalidates_only_that_child_downstream() {
    let executor = Arc::new(FakeExecutor::default());
    let runner = PipelineRunner::new(
        PipelineGraph::video_translation(),
        config(),
        manifest(),
        executor,
    );
    let cancel = CancelToken::default();
    runner.run_parent(&cancel).await.unwrap();
    runner
        .run_target(VariantId("zh-CN".into()), &cancel)
        .await
        .unwrap();
    runner
        .run_target(VariantId("zh-yue".into()), &cancel)
        .await
        .unwrap();

    runner
        .invalidate_from(RunScope::Target(VariantId("zh-CN".into())), "translate")
        .await
        .unwrap();
    let snapshot = runner.manifest_snapshot().await;

    assert_eq!(
        snapshot.target_stages[&VariantId("zh-CN".into())][&StageId("tts".into())].status,
        StageStatus::Invalidated
    );
    assert_eq!(
        snapshot.target_stages[&VariantId("zh-yue".into())][&StageId("tts".into())].status,
        StageStatus::Done
    );
}

#[tokio::test]
async fn deleted_dub_is_detected_and_reruns_only_tts_chain() {
    let root =
        std::env::temp_dir().join(format!("videotrans-pipeline-{}", uuid::Uuid::new_v4()));
    let executor = Arc::new(FakeExecutor::default());
    let runner = PipelineRunner::new(
        PipelineGraph::video_translation(),
        config(),
        manifest(),
        executor.clone(),
    );
    let cancel = CancelToken::default();
    runner.run_parent(&cancel).await.unwrap();
    let variant = VariantId("zh-CN".into());
    runner.run_target(variant.clone(), &cancel).await.unwrap();

    let mut snapshot = runner.manifest_snapshot().await;
    for artifact in snapshot.artifacts.values_mut() {
        let path = root.join(&artifact.relative_path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"x").unwrap();
        let metadata = fs::metadata(&path).unwrap();
        artifact.size = metadata.len();
        artifact.modified = metadata
            .modified()
            .unwrap()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
    }
    let restarted = PipelineRunner::new(
        PipelineGraph::video_translation(),
        config(),
        snapshot,
        executor.clone(),
    );
    let dub_id = ArtifactId("target:zh-CN:tts:0".into());
    let dub_path = {
        let current = restarted.manifest_snapshot().await;
        root.join(&current.artifacts[&dub_id].relative_path)
    };
    fs::remove_file(dub_path).unwrap();

    assert_eq!(
        restarted
            .reconcile_artifacts(&ArtifactStore::new(&root))
            .await
            .unwrap(),
        1
    );
    restarted.run_target(variant, &cancel).await.unwrap();

    assert_eq!(executor.count("target:zh-CN:translate").await, 1);
    assert_eq!(executor.count("target:zh-CN:tts").await, 2);
    assert_eq!(executor.count("target:zh-CN:mix").await, 2);
    assert_eq!(executor.count("target:zh-CN:srt").await, 1);
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn external_translation_edit_keeps_translation_and_invalidates_its_downstream() {
    let root = std::env::temp_dir().join(format!("external-edit-{}", uuid::Uuid::new_v4()));
    let target_dir = root.join("targets/zh-CN");
    fs::create_dir_all(&target_dir).unwrap();
    let translated_path = target_dir.join("translated.json");
    fs::write(&translated_path, b"old").unwrap();
    let metadata = fs::metadata(&translated_path).unwrap();
    let mut snapshot = manifest();
    let variant = VariantId("zh-CN".into());
    let artifact_id = ArtifactId("translated".into());
    snapshot.artifacts.insert(
        artifact_id.clone(),
        ArtifactRecord {
            id: artifact_id.clone(),
            kind: ArtifactKind::TranslatedSegments,
            producer_stage_id: StageId("translate".into()),
            status: ArtifactStatus::Valid,
            retention: RetentionPolicy::RequiredForResume,
            relative_path: "targets/zh-CN/translated.json".into(),
            size: metadata.len(),
            modified: metadata
                .modified()
                .unwrap()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as i64,
            content_hash: Some("old-hash".into()),
            media_type: Some("application/json".into()),
            schema_version: Some(1),
        },
    );
    let mut stages = BTreeMap::new();
    stages.insert(
        StageId("translate".into()),
        StageRecord::done("translate", "h", vec![artifact_id]),
    );
    for stage in ["tts", "mix", "srt", "final_video"] {
        stages.insert(StageId(stage.into()), StageRecord::done(stage, "h", vec![]));
    }
    snapshot.target_stages.insert(variant.clone(), stages);
    fs::write(&translated_path, b"new translated content").unwrap();
    let runner = PipelineRunner::new(
        PipelineGraph::video_translation(),
        config(),
        snapshot,
        Arc::new(FakeExecutor::default()),
    )
    .with_environment(&root, root.join("source.mp4"));

    runner
        .reconcile_artifacts(&ArtifactStore::new(&root))
        .await
        .unwrap();
    let reconciled = runner.manifest_snapshot().await;

    assert_eq!(
        reconciled.target_stages[&variant][&StageId("translate".into())].status,
        StageStatus::Done
    );
    assert_eq!(
        reconciled.target_stages[&variant][&StageId("tts".into())].status,
        StageStatus::Invalidated
    );
    assert_eq!(
        reconciled.target_stages[&variant][&StageId("srt".into())].status,
        StageStatus::Invalidated
    );
    assert!(reconciled.artifacts[&ArtifactId("translated".into())]
        .content_hash
        .as_deref()
        .unwrap()
        .starts_with("sha256:"));
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn missing_dub_reruns_tts_chain_but_not_translation_or_srt() {
    let executor = Arc::new(FakeExecutor::default());
    let runner = PipelineRunner::new(
        PipelineGraph::video_translation(),
        config(),
        manifest(),
        executor.clone(),
    );
    let cancel = CancelToken::default();
    runner.run_parent(&cancel).await.unwrap();
    let variant = VariantId("zh-CN".into());
    runner.run_target(variant.clone(), &cancel).await.unwrap();
    runner
        .invalidate_from(RunScope::Target(variant.clone()), "tts")
        .await
        .unwrap();
    runner.run_target(variant, &cancel).await.unwrap();

    assert_eq!(executor.count("target:zh-CN:translate").await, 1);
    assert_eq!(executor.count("target:zh-CN:tts").await, 2);
    assert_eq!(executor.count("target:zh-CN:mix").await, 2);
    assert_eq!(executor.count("target:zh-CN:srt").await, 1);
}
