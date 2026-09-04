//! StageRecord 的读写辅助：原本是 runner.rs 尾部的私有自由函数。
//! 仅 executor 兄弟模块调用，可见性限定在 runner 子树内，不进 crate 公共面。

use std::collections::BTreeMap;

use crate::domain::ids::StageId;
use crate::domain::manifest::{StageRecord, StageStatus, TaskManifest};
use crate::pipeline::graph::{NodeScope, StageNode};

use super::types::{PipelineError, RunScope};

pub(in crate::pipeline::runner) fn validate_scope(
    node: &StageNode,
    scope: &RunScope,
) -> Result<(), PipelineError> {
    if matches!((&node.scope, scope), (NodeScope::Parent, RunScope::Parent))
        || matches!(
            (&node.scope, scope),
            (NodeScope::Target, RunScope::Target(_))
        )
    {
        Ok(())
    } else {
        Err(PipelineError::ScopeMismatch(node.id.0.clone()))
    }
}

pub(in crate::pipeline::runner) fn stage_record(
    node: &StageNode,
    scope: &RunScope,
    dependency_hash: String,
    engine_version: String,
    status: StageStatus,
) -> StageRecord {
    StageRecord {
        stage_id: node.id.clone(),
        node_key: scope.node_key(&node.id),
        status,
        dependency_hash,
        input_hash: "inputs-v1".into(),
        param_hash: "params-v1".into(),
        engine_version,
        stage_schema_version: 1,
        hash_algo_version: 1,
        artifact_ids: vec![],
        started_at: Some(1),
        completed_at: None,
        error: None,
        attempts: vec![],
        fallback: None,
        external_override: false,
    }
}

pub(in crate::pipeline::runner) fn get_record<'a>(
    manifest: &'a TaskManifest,
    scope: &RunScope,
    stage: &StageId,
) -> Option<&'a StageRecord> {
    match scope {
        RunScope::Parent => manifest.stages.get(stage),
        RunScope::Target(variant) => manifest.target_stages.get(variant)?.get(stage),
    }
}

pub(in crate::pipeline::runner) fn get_record_mut<'a>(
    manifest: &'a mut TaskManifest,
    scope: &RunScope,
    stage: &StageId,
) -> Option<&'a mut StageRecord> {
    match scope {
        RunScope::Parent => manifest.stages.get_mut(stage),
        RunScope::Target(variant) => manifest.target_stages.get_mut(variant)?.get_mut(stage),
    }
}

pub(in crate::pipeline::runner) fn insert_record(
    manifest: &mut TaskManifest,
    scope: &RunScope,
    record: StageRecord,
) {
    match scope {
        RunScope::Parent => {
            manifest.stages.insert(record.stage_id.clone(), record);
        }
        RunScope::Target(variant) => {
            manifest
                .target_stages
                .entry(variant.clone())
                .or_default()
                .insert(record.stage_id.clone(), record);
        }
    }
}

pub(in crate::pipeline::runner) fn record_for_dependency<'a>(
    manifest: &'a TaskManifest,
    dependency_node: &StageNode,
    scope: &RunScope,
    stage: &StageId,
) -> Option<&'a StageRecord> {
    match dependency_node.scope {
        NodeScope::Parent => manifest.stages.get(stage),
        NodeScope::Target => match scope {
            RunScope::Target(variant) => manifest.target_stages.get(variant)?.get(stage),
            RunScope::Parent => None,
        },
    }
}

pub(in crate::pipeline::runner) fn invalidate_record(
    records: &mut BTreeMap<StageId, StageRecord>,
    stage: &StageId,
) {
    if let Some(record) = records.get_mut(stage) {
        record.status = StageStatus::Invalidated;
        record.error = None;
    }
}
