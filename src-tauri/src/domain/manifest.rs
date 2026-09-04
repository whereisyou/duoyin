use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::artifact::{ArtifactRecord, ArtifactStatus};
use super::ids::{ArtifactId, StageId, TaskId, VariantId};
use super::media::SourceFingerprint;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StageStatus {
    Pending,
    Running,
    Interrupted,
    Done,
    Failed,
    Skipped,
    Invalidated,
    Canceled,
    Degraded,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttemptRecord {
    pub attempt_no: u32,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FallbackRecord {
    pub trigger_error: String,
    pub from: String,
    pub to: String,
    pub degraded_quality: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StageRecord {
    pub stage_id: StageId,
    /// 产物级 DAG 节点键：stage + parent/variant scope + upstream outputs。
    pub node_key: String,
    pub status: StageStatus,
    pub dependency_hash: String,
    pub input_hash: String,
    pub param_hash: String,
    pub engine_version: String,
    pub stage_schema_version: u32,
    pub hash_algo_version: u32,
    /// 只引用 ArtifactId，避免内联冗余。
    pub artifact_ids: Vec<ArtifactId>,
    pub started_at: Option<i64>,
    pub completed_at: Option<i64>,
    pub error: Option<String>,
    pub attempts: Vec<AttemptRecord>,
    pub fallback: Option<FallbackRecord>,
    #[serde(default)]
    pub external_override: bool,
}

impl StageRecord {
    pub fn done(stage: &str, dependency_hash: &str, artifact_ids: Vec<ArtifactId>) -> Self {
        Self {
            stage_id: StageId(stage.into()),
            node_key: stage.into(),
            status: StageStatus::Done,
            dependency_hash: dependency_hash.into(),
            input_hash: "input".into(),
            param_hash: "param".into(),
            engine_version: "engine".into(),
            stage_schema_version: 1,
            hash_algo_version: 1,
            artifact_ids,
            started_at: Some(1),
            completed_at: Some(2),
            error: None,
            attempts: vec![],
            fallback: None,
            external_override: false,
        }
    }

    /// 降级构造：仅测试使用（degraded 恢复场景）
    #[allow(dead_code)]
    pub fn degraded(stage: &str, dependency_hash: &str, reason: &str) -> Self {
        let mut s = Self::done(stage, dependency_hash, vec![]);
        s.status = StageStatus::Degraded;
        s.fallback = Some(FallbackRecord {
            trigger_error: reason.into(),
            from: stage.into(),
            to: "no_bgm".into(),
            degraded_quality: true,
        });
        s
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskManifest {
    pub schema_version: u32,
    pub parent_task_id: TaskId,
    pub source_fingerprint: SourceFingerprint,
    pub artifacts: BTreeMap<ArtifactId, ArtifactRecord>,
    pub stages: BTreeMap<StageId, StageRecord>,
    pub target_stages: BTreeMap<VariantId, BTreeMap<StageId, StageRecord>>,
}

impl TaskManifest {
    pub fn new(parent_task_id: impl Into<TaskId>, source_fingerprint: SourceFingerprint) -> Self {
        Self {
            schema_version: 1,
            parent_task_id: parent_task_id.into(),
            source_fingerprint,
            artifacts: BTreeMap::new(),
            stages: BTreeMap::new(),
            target_stages: BTreeMap::new(),
        }
    }

    /// 可变构造只被测试/恢复路径使用（生产跑 run 时 executor 用不可变查询）
    #[allow(dead_code)]
    pub fn add_artifact(&mut self, artifact: ArtifactRecord) {
        self.artifacts.insert(artifact.id.clone(), artifact);
    }

    #[allow(dead_code)]
    pub fn add_stage(&mut self, stage: StageRecord) {
        self.stages.insert(stage.stage_id.clone(), stage);
    }

    /// 应用重启时，进程内 Running 已不可能继续，统一标记 Interrupted。
    pub fn recover_interrupted(&mut self) -> usize {
        let mut changed = 0;
        for stage in self.stages.values_mut() {
            if stage.status == StageStatus::Running {
                stage.status = StageStatus::Interrupted;
                stage.error = Some("应用退出时阶段仍在运行，需要重新执行".into());
                changed += 1;
            }
        }
        for stages in self.target_stages.values_mut() {
            for stage in stages.values_mut() {
                if stage.status == StageStatus::Running {
                    stage.status = StageStatus::Interrupted;
                    stage.error = Some("应用退出时阶段仍在运行，需要重新执行".into());
                    changed += 1;
                }
            }
        }
        changed
    }

    /// 恢复/测试路径的复用判据（生产在 checkpoint 前置判断，当前未接线，保留）
    #[allow(dead_code)]
    pub fn can_reuse_stage(&self, stage_id: &StageId, expected_dependency_hash: &str) -> bool {
        let Some(stage) = self.stages.get(stage_id) else {
            return false;
        };
        self.can_reuse_record(stage, expected_dependency_hash)
    }

    pub fn can_reuse_record(&self, stage: &StageRecord, expected_dependency_hash: &str) -> bool {
        if !stage.external_override && stage.dependency_hash != expected_dependency_hash {
            return false;
        }
        match stage.status {
            StageStatus::Done | StageStatus::Skipped => {}
            StageStatus::Degraded => {
                // 降级态必须显式记录 fallback，且可没有产物（如 no_bgm）
                return stage.fallback.is_some();
            }
            _ => return false,
        }

        stage.artifact_ids.iter().all(|id| {
            self.artifacts
                .get(id)
                .map(|a| a.status == ArtifactStatus::Valid)
                .unwrap_or(false)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::artifact::{ArtifactKind, ArtifactRecord};

    fn source_fingerprint() -> SourceFingerprint {
        SourceFingerprint {
            size: 100,
            modified: 1,
            content_hash: Some("src".into()),
            hash_algo_version: 1,
        }
    }

    fn manifest() -> TaskManifest {
        TaskManifest::new("p1", source_fingerprint())
    }

    #[test]
    fn done_stage_with_valid_artifact_can_reuse() {
        let mut m = manifest();
        let audio = ArtifactRecord::valid_required(
            "a1",
            ArtifactKind::ExtractedAudio,
            "extract_audio",
            "audio.wav",
        );
        m.add_artifact(audio);
        m.add_stage(StageRecord::done(
            "extract_audio",
            "h1",
            vec![ArtifactId("a1".into())],
        ));

        assert!(m.can_reuse_stage(&StageId("extract_audio".into()), "h1"));
    }

    #[test]
    fn hash_mismatch_cannot_reuse() {
        let mut m = manifest();
        m.add_stage(StageRecord::done("stt", "old", vec![]));
        assert!(!m.can_reuse_stage(&StageId("stt".into()), "new"));
    }

    #[test]
    fn missing_artifact_cannot_reuse() {
        let mut m = manifest();
        m.add_stage(StageRecord::done(
            "stt",
            "h",
            vec![ArtifactId("missing".into())],
        ));
        assert!(!m.can_reuse_stage(&StageId("stt".into()), "h"));
    }

    #[test]
    fn invalid_artifact_cannot_reuse() {
        let mut m = manifest();
        let mut segs = ArtifactRecord::valid_required(
            "s1",
            ArtifactKind::Segments,
            "stt",
            "stt/segments.json",
        );
        segs.status = ArtifactStatus::Invalidated;
        m.add_artifact(segs);
        m.add_stage(StageRecord::done("stt", "h", vec![ArtifactId("s1".into())]));
        assert!(!m.can_reuse_stage(&StageId("stt".into()), "h"));
    }

    #[test]
    fn degraded_stage_with_fallback_can_reuse_without_artifacts() {
        let mut m = manifest();
        m.add_stage(StageRecord::degraded("separation", "h", "model failed"));
        assert!(m.can_reuse_stage(&StageId("separation".into()), "h"));
    }

    #[test]
    fn running_stage_cannot_reuse() {
        let mut m = manifest();
        let mut s = StageRecord::done("stt", "h", vec![]);
        s.status = StageStatus::Running;
        m.add_stage(s);
        assert!(!m.can_reuse_stage(&StageId("stt".into()), "h"));
    }

    #[test]
    fn startup_marks_running_parent_and_target_stages_interrupted() {
        let mut m = manifest();
        let mut parent = StageRecord::done("stt", "h", vec![]);
        parent.status = StageStatus::Running;
        m.add_stage(parent);
        let mut child = StageRecord::done("tts", "h", vec![]);
        child.status = StageStatus::Running;
        m.target_stages
            .entry(VariantId("zh-CN".into()))
            .or_default()
            .insert(StageId("tts".into()), child);

        assert_eq!(m.recover_interrupted(), 2);
        assert_eq!(
            m.stages[&StageId("stt".into())].status,
            StageStatus::Interrupted
        );
        assert_eq!(
            m.target_stages[&VariantId("zh-CN".into())][&StageId("tts".into())].status,
            StageStatus::Interrupted
        );
        assert!(!m.can_reuse_stage(&StageId("stt".into()), "h"));
    }

    #[test]
    fn serde_roundtrip_preserves_manifest() {
        let mut m = manifest();
        m.add_stage(StageRecord::degraded("separation", "h", "x"));
        let json = serde_json::to_string(&m).unwrap();
        let loaded: TaskManifest = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.schema_version, 1);
        assert!(loaded.can_reuse_stage(&StageId("separation".into()), "h"));
    }
}
