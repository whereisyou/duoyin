use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::{Mutex, OwnedMutexGuard};

use crate::domain::artifact::{ArtifactKind, ArtifactRecord, ArtifactStatus};
use crate::domain::config::PipelineConfig;
use crate::domain::ids::{ArtifactId, StageId, VariantId};
use crate::domain::manifest::{FallbackRecord, StageRecord, StageStatus, TaskManifest};
use crate::infra::artifact_store::ArtifactStore;
use crate::pipeline::graph::{NodeScope, PipelineGraph, StageNode};

use super::records::{
    get_record, get_record_mut, insert_record, invalidate_record, record_for_dependency,
    stage_record, validate_scope,
};
use super::types::{
    ArtifactInput, ArtifactOutput, CancelToken, ExecuteError, ExecutionContext, ExecutionOutcome,
    PipelineCheckpoint, PipelineError, PipelineObserver, RunScope, StageExecutor, StageRequest,
    StageRunResult, StageUpdate,
};

pub struct PipelineRunner<E: StageExecutor> {
    graph: PipelineGraph,
    config: PipelineConfig,
    manifest: Arc<Mutex<TaskManifest>>,
    executor: Arc<E>,
    task_root: PathBuf,
    source_video: PathBuf,
    checkpoint: Option<Arc<dyn PipelineCheckpoint>>,
    observer: Option<Arc<dyn PipelineObserver>>,
    flights: Mutex<BTreeMap<String, Arc<Mutex<()>>>>,
}

impl<E: StageExecutor> PipelineRunner<E> {
    pub fn new(
        graph: PipelineGraph,
        config: PipelineConfig,
        manifest: TaskManifest,
        executor: Arc<E>,
    ) -> Self {
        Self {
            graph,
            config,
            manifest: Arc::new(Mutex::new(manifest)),
            executor,
            task_root: PathBuf::from("."),
            source_video: PathBuf::from("source.mp4"),
            checkpoint: None,
            observer: None,
            flights: Mutex::new(BTreeMap::new()),
        }
    }

    pub fn with_environment(
        mut self,
        task_root: impl Into<PathBuf>,
        source_video: impl Into<PathBuf>,
    ) -> Self {
        self.task_root = task_root.into();
        self.source_video = source_video.into();
        self
    }

    pub fn with_checkpoint(mut self, checkpoint: Arc<dyn PipelineCheckpoint>) -> Self {
        self.checkpoint = Some(checkpoint);
        self
    }

    pub fn with_observer(mut self, observer: Arc<dyn PipelineObserver>) -> Self {
        self.observer = Some(observer);
        self
    }

    pub async fn manifest_snapshot(&self) -> TaskManifest {
        self.manifest.lock().await.clone()
    }

    pub async fn run_parent(&self, cancel: &CancelToken) -> Result<(), PipelineError> {
        self.run_named(RunScope::Parent, "media_probe", cancel)
            .await?;
        if self.config.separation.enabled {
            let stt_branch = async {
                self.run_named(RunScope::Parent, "extract_audio", cancel)
                    .await?;
                self.run_named(RunScope::Parent, "stt", cancel).await
            };
            let (stt, separation) = tokio::join!(
                stt_branch,
                self.run_named(RunScope::Parent, "separation", cancel),
            );
            stt?;
            separation?;
        } else {
            self.run_named(RunScope::Parent, "extract_audio", cancel)
                .await?;
            self.run_named(RunScope::Parent, "stt", cancel).await?;
            self.run_named(RunScope::Parent, "separation", cancel)
                .await?;
        }
        Ok(())
    }

    pub async fn run_target(
        &self,
        variant: VariantId,
        cancel: &CancelToken,
    ) -> Result<(), PipelineError> {
        let scope = RunScope::Target(variant);
        for stage in ["translate", "tts", "mix", "srt", "final_video"] {
            self.run_named(scope.clone(), stage, cancel).await?;
        }
        Ok(())
    }

    pub async fn run_targets_with_tokens(
        &self,
        variants: &BTreeMap<VariantId, CancelToken>,
    ) -> BTreeMap<VariantId, Result<(), PipelineError>> {
        let futures = variants.iter().map(|(variant, cancel)| async move {
            (
                variant.clone(),
                self.run_target(variant.clone(), cancel).await,
            )
        });
        futures::future::join_all(futures)
            .await
            .into_iter()
            .collect()
    }

    /// 每个目标独立返回结果；失败不会短路尚未运行的其他语言版本。
    /// 并行驱动多目标（旧单目标入口遗留；当前生产走 run_parent + run_targets_with_tokens），
    /// 保留——若未来恢复并行调度可直接接回（FUNCTION_CHECKLIST 已登记）。
    #[allow(dead_code)]
    pub async fn run_targets(
        &self,
        variants: &[VariantId],
        cancel: &CancelToken,
    ) -> BTreeMap<VariantId, Result<(), PipelineError>> {
        let mut results = BTreeMap::new();
        for variant in variants {
            results.insert(
                variant.clone(),
                self.run_target(variant.clone(), cancel).await,
            );
        }
        results
    }

    pub async fn run_named(
        &self,
        scope: RunScope,
        stage: &str,
        cancel: &CancelToken,
    ) -> Result<StageRunResult, PipelineError> {
        let stage_id = StageId(stage.into());
        let node = self.graph.node(&stage_id)?.clone();
        validate_scope(&node, &scope)?;
        let node_key = scope.node_key(&stage_id);
        let _flight = self.acquire_flight(&node_key).await;

        if cancel.is_canceled() {
            let mut record = stage_record(
                &node,
                &scope,
                "canceled-before-start".into(),
                self.executor.version(&node.id),
                StageStatus::Canceled,
            );
            record.error = Some("canceled".into());
            record.completed_at = Some(2);
            self.insert_record(&scope, record).await?;
            return Err(PipelineError::Canceled(node_key));
        }

        let engine_version = self.executor.version(&node.id);
        let dependency_hash = self.dependency_hash(&node, &scope, &engine_version).await?;
        if self.can_reuse(&node, &scope, &dependency_hash).await {
            return Ok(StageRunResult::Reused);
        }

        if !self.is_enabled(&node) {
            let record = stage_record(
                &node,
                &scope,
                dependency_hash,
                engine_version.clone(),
                StageStatus::Skipped,
            );
            self.insert_record(&scope, record).await?;
            return Ok(StageRunResult::Skipped);
        }

        let running = stage_record(
            &node,
            &scope,
            dependency_hash.clone(),
            engine_version.clone(),
            StageStatus::Running,
        );
        self.insert_record(&scope, running).await?;

        let request = self.stage_request(&node, &scope).await?;
        let context = ExecutionContext {
            task_root: self.task_root.clone(),
            cancel: cancel.clone(),
        };
        match self.executor.execute(request, context).await {
            Ok(ExecutionOutcome::Done(outputs)) => {
                self.commit_outputs(
                    &node,
                    &scope,
                    dependency_hash,
                    engine_version,
                    outputs,
                    None,
                )
                .await?;
                Ok(StageRunResult::Executed)
            }
            Ok(ExecutionOutcome::Degraded { outputs, fallback }) => {
                self.commit_outputs(
                    &node,
                    &scope,
                    dependency_hash,
                    engine_version,
                    outputs,
                    Some(fallback),
                )
                .await?;
                Ok(StageRunResult::Degraded)
            }
            Err(ExecuteError::Canceled) => {
                self.record_terminal(
                    &node,
                    &scope,
                    StageStatus::Canceled,
                    Some("canceled".into()),
                )
                .await?;
                Err(PipelineError::Canceled(node_key))
            }
            Err(ExecuteError::Failed(message))
                if node.id.0 == "separation" && self.config.separation.allow_no_bgm_fallback =>
            {
                log::warn!(
                    "pipeline stage degraded task_root={} node={} from=separation to=no_bgm error={}",
                    self.task_root.display(),
                    node_key,
                    message
                );
                let fallback = FallbackRecord {
                    trigger_error: message,
                    from: "separation".into(),
                    to: "no_bgm".into(),
                    degraded_quality: true,
                };
                self.commit_outputs(
                    &node,
                    &scope,
                    dependency_hash,
                    engine_version,
                    vec![],
                    Some(fallback),
                )
                .await?;
                Ok(StageRunResult::Degraded)
            }
            Err(ExecuteError::Failed(message)) => {
                self.record_terminal(&node, &scope, StageStatus::Failed, Some(message.clone()))
                    .await?;
                Err(PipelineError::StageFailed { node_key, message })
            }
        }
    }

    /// 启动/恢复前做廉价文件校验，并根据 artifact 引用反查作用域后传播失效。
    pub async fn reconcile_artifacts(&self, store: &ArtifactStore) -> Result<usize, PipelineError> {
        let (invalid_sources, edited_sources) = {
            let mut manifest = self.manifest.lock().await;
            let changed = store.refresh_statuses(manifest.artifacts.values_mut());
            if changed == 0 {
                return Ok(0);
            }

            let stale_ids: Vec<_> = manifest
                .artifacts
                .iter()
                .filter(|(_, artifact)| artifact.status == ArtifactStatus::Stale)
                .map(|(id, _)| id.clone())
                .collect();
            let mut edited_ids = Vec::new();
            for id in stale_ids {
                if let Some(artifact) = manifest.artifacts.get_mut(&id) {
                    if store.accept_external_edit(artifact) {
                        edited_ids.push(id);
                    }
                }
            }
            let invalid_ids: Vec<_> = manifest
                .artifacts
                .iter()
                .filter(|(_, artifact)| artifact.status != ArtifactStatus::Valid)
                .map(|(id, _)| id.clone())
                .collect();
            let mut sources = Vec::new();
            let mut edited_sources = Vec::new();
            for artifact_id in invalid_ids {
                for record in manifest.stages.values() {
                    if record.artifact_ids.contains(&artifact_id) {
                        sources.push((RunScope::Parent, record.stage_id.clone()));
                    }
                }
                for (variant, records) in &manifest.target_stages {
                    for record in records.values() {
                        if record.artifact_ids.contains(&artifact_id) {
                            sources
                                .push((RunScope::Target(variant.clone()), record.stage_id.clone()));
                        }
                    }
                }
            }
            for artifact_id in edited_ids {
                for record in manifest.stages.values() {
                    if record.artifact_ids.contains(&artifact_id) {
                        edited_sources.push((RunScope::Parent, record.stage_id.clone()));
                    }
                }
                for (variant, records) in &manifest.target_stages {
                    for record in records.values() {
                        if record.artifact_ids.contains(&artifact_id) {
                            edited_sources
                                .push((RunScope::Target(variant.clone()), record.stage_id.clone()));
                        }
                    }
                }
            }
            (sources, edited_sources)
        };

        for (scope, stage) in &invalid_sources {
            self.invalidate_from(scope.clone(), &stage.0).await?;
        }
        for (scope, stage) in &edited_sources {
            self.invalidate_descendants(scope.clone(), stage).await?;
        }
        Ok(invalid_sources.len() + edited_sources.len())
    }

    async fn invalidate_descendants(
        &self,
        scope: RunScope,
        stage: &StageId,
    ) -> Result<Vec<StageId>, PipelineError> {
        let affected = self.graph.descendants(stage)?;
        let mut manifest = self.manifest.lock().await;
        let mut updates = Vec::new();
        for id in &affected {
            let node = self.graph.node(id)?;
            match (&scope, &node.scope) {
                (RunScope::Parent, NodeScope::Target) => {
                    for (variant, stages) in manifest.target_stages.iter_mut() {
                        invalidate_record(stages, id);
                        updates.push(StageUpdate {
                            scope: RunScope::Target(variant.clone()),
                            stage_id: id.clone(),
                            status: StageStatus::Invalidated,
                            error: None,
                        });
                    }
                }
                (RunScope::Target(variant), NodeScope::Target) => {
                    if let Some(stages) = manifest.target_stages.get_mut(variant) {
                        invalidate_record(stages, id);
                        updates.push(StageUpdate {
                            scope: scope.clone(),
                            stage_id: id.clone(),
                            status: StageStatus::Invalidated,
                            error: None,
                        });
                    }
                }
                _ => {}
            }
        }
        drop(manifest);
        for update in updates {
            self.notify(update);
        }
        self.checkpoint().await?;
        Ok(affected)
    }

    pub async fn invalidate_from(
        &self,
        scope: RunScope,
        stage: &str,
    ) -> Result<Vec<StageId>, PipelineError> {
        let stage_id = StageId(stage.into());
        let mut affected = vec![stage_id.clone()];
        affected.extend(self.graph.descendants(&stage_id)?);
        let mut manifest = self.manifest.lock().await;

        let mut updates = Vec::new();
        for id in &affected {
            let Ok(node) = self.graph.node(id) else {
                continue;
            };
            match (&scope, &node.scope) {
                (RunScope::Parent, NodeScope::Parent) => {
                    invalidate_record(&mut manifest.stages, id);
                    updates.push(StageUpdate {
                        scope: RunScope::Parent,
                        stage_id: id.clone(),
                        status: StageStatus::Invalidated,
                        error: None,
                    });
                }
                (RunScope::Parent, NodeScope::Target) => {
                    for (variant, stages) in manifest.target_stages.iter_mut() {
                        invalidate_record(stages, id);
                        updates.push(StageUpdate {
                            scope: RunScope::Target(variant.clone()),
                            stage_id: id.clone(),
                            status: StageStatus::Invalidated,
                            error: None,
                        });
                    }
                }
                (RunScope::Target(variant), NodeScope::Target) => {
                    if let Some(stages) = manifest.target_stages.get_mut(variant) {
                        invalidate_record(stages, id);
                        updates.push(StageUpdate {
                            scope: scope.clone(),
                            stage_id: id.clone(),
                            status: StageStatus::Invalidated,
                            error: None,
                        });
                    }
                }
                (RunScope::Target(_), NodeScope::Parent) => {}
            }
        }
        drop(manifest);
        for update in updates {
            self.notify(update);
        }
        self.checkpoint().await?;
        Ok(affected)
    }

    async fn acquire_flight(&self, key: &str) -> OwnedMutexGuard<()> {
        let lock = {
            let mut flights = self.flights.lock().await;
            flights
                .entry(key.to_owned())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        lock.lock_owned().await
    }

    fn is_enabled(&self, node: &StageNode) -> bool {
        match node.id.0.as_str() {
            "separation" => self.config.separation.enabled,
            "final_video" => self.config.output.generate_final_videos,
            _ => true,
        }
    }

    async fn stage_request(
        &self,
        node: &StageNode,
        scope: &RunScope,
    ) -> Result<StageRequest, PipelineError> {
        let manifest = self.manifest.lock().await;
        let mut inputs = Vec::new();
        if matches!(
            node.id.0.as_str(),
            "media_probe" | "extract_audio" | "separation" | "final_video"
        ) {
            inputs.push(ArtifactInput {
                id: ArtifactId("source-video".into()),
                kind: ArtifactKind::SourceVideo,
                path: self.source_video.clone(),
                content_hash: manifest.source_fingerprint.content_hash.clone(),
            });
        }
        for dependency in &node.depends_on {
            let dependency_node = self.graph.node(dependency)?;
            let record = record_for_dependency(&manifest, dependency_node, scope, dependency)
                .ok_or_else(|| PipelineError::DependencyNotReady(dependency.0.clone()))?;
            for artifact_id in &record.artifact_ids {
                let artifact = manifest
                    .artifacts
                    .get(artifact_id)
                    .ok_or_else(|| PipelineError::DependencyNotReady(dependency.0.clone()))?;
                inputs.push(ArtifactInput {
                    id: artifact.id.clone(),
                    kind: artifact.kind.clone(),
                    path: self.task_root.join(&artifact.relative_path),
                    content_hash: artifact.content_hash.clone(),
                });
            }
        }
        Ok(StageRequest {
            node: node.clone(),
            scope: scope.clone(),
            inputs,
        })
    }

    async fn dependency_hash(
        &self,
        node: &StageNode,
        scope: &RunScope,
        engine_version: &str,
    ) -> Result<String, PipelineError> {
        let manifest = self.manifest.lock().await;
        let mut parts = vec![format!("stage={}", node.id.0), format!("scope={scope:?}")];
        if matches!(
            node.id.0.as_str(),
            "media_probe" | "extract_audio" | "separation" | "final_video"
        ) {
            let source = &manifest.source_fingerprint;
            parts.push(format!(
                "source={}:{}:{:?}:{}",
                source.size, source.modified, source.content_hash, source.hash_algo_version
            ));
        }
        for dependency in &node.depends_on {
            let dependency_node = self.graph.node(dependency)?;
            let record = record_for_dependency(&manifest, dependency_node, scope, dependency)
                .ok_or_else(|| PipelineError::DependencyNotReady(dependency.0.clone()))?;
            if !matches!(
                record.status,
                StageStatus::Done | StageStatus::Skipped | StageStatus::Degraded
            ) {
                return Err(PipelineError::DependencyNotReady(dependency.0.clone()));
            }
            parts.push(format!("{}={}", dependency.0, record.dependency_hash));
            for artifact in &record.artifact_ids {
                let value = manifest
                    .artifacts
                    .get(artifact)
                    .and_then(|item| item.content_hash.as_deref())
                    .ok_or_else(|| PipelineError::DependencyNotReady(dependency.0.clone()))?;
                parts.push(format!("{}:{value}", artifact.0));
            }
        }
        parts.push(format!("engine_version={engine_version}"));
        parts.push(format!("enabled={}", self.is_enabled(node)));
        parts.push(format!("config={}", self.stage_config_key(node)));
        Ok(parts.join("|"))
    }

    fn stage_config_key(&self, node: &StageNode) -> String {
        match node.id.0.as_str() {
            "stt" => format!("engine={}", self.config.engines.stt),
            "separation" => format!(
                "engine={:?};denoise={};normalize={};fallback={}",
                self.config.engines.separator,
                self.config.separation.denoise,
                self.config.separation.normalize,
                self.config.separation.allow_no_bgm_fallback
            ),
            "translate" => format!("engine={}", self.config.engines.translator),
            "tts" => format!("engine={}", self.config.engines.tts),
            "mix" => format!(
                "keep_original={};speed={}-{}",
                self.config.output.keep_original_audio_track,
                self.config.output.min_speed_percent,
                self.config.output.max_speed_percent
            ),
            "srt" => format!("subtitle={:?}", self.config.output.subtitle),
            "final_video" => format!(
                "enabled={};subtitle={:?}",
                self.config.output.generate_final_videos, self.config.output.subtitle
            ),
            _ => "v1".into(),
        }
    }

    async fn can_reuse(&self, node: &StageNode, scope: &RunScope, hash: &str) -> bool {
        let manifest = self.manifest.lock().await;
        let record = get_record(&manifest, scope, &node.id);
        record
            .map(|value| manifest.can_reuse_record(value, hash))
            .unwrap_or(false)
    }

    async fn insert_record(
        &self,
        scope: &RunScope,
        record: StageRecord,
    ) -> Result<(), PipelineError> {
        let update = StageUpdate {
            scope: scope.clone(),
            stage_id: record.stage_id.clone(),
            status: record.status.clone(),
            error: record.error.clone(),
        };
        let mut manifest = self.manifest.lock().await;
        insert_record(&mut manifest, scope, record);
        drop(manifest);
        self.notify(update);
        self.checkpoint().await
    }

    async fn record_terminal(
        &self,
        node: &StageNode,
        scope: &RunScope,
        status: StageStatus,
        error: Option<String>,
    ) -> Result<(), PipelineError> {
        let node_key = scope.node_key(&node.id);
        if matches!(status, StageStatus::Failed | StageStatus::Interrupted) {
            crate::logger::record_failure(
                &self.task_root.to_string_lossy(),
                &node_key,
                &node.id.0,
                error.as_deref().unwrap_or("unknown"),
            );
        }
        match status {
            StageStatus::Failed | StageStatus::Interrupted => log::error!(
                "pipeline stage failed task_root={} node={} stage={} status={:?} error={}",
                self.task_root.display(),
                node_key,
                node.id.0,
                status,
                error.as_deref().unwrap_or("unknown")
            ),
            StageStatus::Canceled => log::warn!(
                "pipeline stage canceled task_root={} node={} stage={}",
                self.task_root.display(),
                node_key,
                node.id.0
            ),
            _ => {}
        }
        let mut manifest = self.manifest.lock().await;
        if let Some(record) = get_record_mut(&mut manifest, scope, &node.id) {
            record.status = status.clone();
            record.error = error.clone();
            record.completed_at = Some(2);
        }
        drop(manifest);
        self.notify(StageUpdate {
            scope: scope.clone(),
            stage_id: node.id.clone(),
            status,
            error,
        });
        self.checkpoint().await
    }

    async fn commit_outputs(
        &self,
        node: &StageNode,
        scope: &RunScope,
        dependency_hash: String,
        engine_version: String,
        outputs: Vec<ArtifactOutput>,
        fallback: Option<FallbackRecord>,
    ) -> Result<(), PipelineError> {
        let mut manifest = self.manifest.lock().await;
        let mut artifact_ids = Vec::with_capacity(outputs.len());
        for output in outputs {
            artifact_ids.push(output.id.clone());
            let relative_path: std::path::PathBuf = output.relative_path.clone().into();
            let content_hash = ArtifactStore::new(&self.task_root)
                .hash_file(&relative_path)
                .unwrap_or(output.content_hash);
            manifest.artifacts.insert(
                output.id.clone(),
                ArtifactRecord {
                    id: output.id,
                    kind: output.kind,
                    producer_stage_id: node.id.clone(),
                    status: ArtifactStatus::Valid,
                    retention: output.retention,
                    relative_path,
                    size: output.size,
                    modified: output.modified,
                    content_hash: Some(content_hash),
                    media_type: output.media_type,
                    schema_version: Some(1),
                },
            );
        }
        let mut record = stage_record(
            node,
            scope,
            dependency_hash,
            engine_version,
            if fallback.is_some() {
                StageStatus::Degraded
            } else {
                StageStatus::Done
            },
        );
        record.artifact_ids = artifact_ids;
        record.completed_at = Some(2);
        record.fallback = fallback;
        let update = StageUpdate {
            scope: scope.clone(),
            stage_id: record.stage_id.clone(),
            status: record.status.clone(),
            error: None,
        };
        insert_record(&mut manifest, scope, record);
        drop(manifest);
        self.notify(update);
        self.checkpoint().await
    }

    fn notify(&self, update: StageUpdate) {
        if let Some(observer) = &self.observer {
            observer.on_stage_update(update);
        }
    }

    async fn checkpoint(&self) -> Result<(), PipelineError> {
        let Some(checkpoint) = &self.checkpoint else {
            return Ok(());
        };
        checkpoint
            .save(self.manifest_snapshot().await)
            .await
            .map_err(PipelineError::Checkpoint)
    }
}
