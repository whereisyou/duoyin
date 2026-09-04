use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::ids::{ArtifactId, StageId};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ArtifactKind {
    SourceVideo,
    MediaInfo,
    ExtractedAudio,
    VocalsRaw,
    VocalsNormalized,
    BackgroundRaw,
    BackgroundNormalized,
    Segments,
    TranslatedSegments,
    SubtitleSrt,
    DubAudio,
    MixedAudio,
    FinalVideo,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ArtifactStatus {
    Valid,
    Stale,
    Missing,
    Invalidated,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RetentionPolicy {
    /// 恢复流程必须保留，例如 audio.wav / segments.json / dub.wav
    RequiredForResume,
    /// 用户最终产物，例如 final.mp4 / translated.srt
    FinalOutput,
    /// 临时或可清理产物
    Temporary,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactRecord {
    pub id: ArtifactId,
    pub kind: ArtifactKind,
    pub producer_stage_id: StageId,
    pub status: ArtifactStatus,
    pub retention: RetentionPolicy,
    /// manifest 只存相对路径；绝对路径由 ArtifactStore 运行时解析。
    pub relative_path: PathBuf,
    pub size: u64,
    pub modified: i64,
    pub content_hash: Option<String>,
    pub media_type: Option<String>,
    pub schema_version: Option<u32>,
}

impl ArtifactRecord {
    /// 测试辅助构造（manifest/artifact/integration 测试都在用；生产走 executor 产物登记）
    #[allow(dead_code)]
    pub fn valid_required(id: &str, kind: ArtifactKind, stage: &str, path: &str) -> Self {
        Self {
            id: ArtifactId(id.into()),
            kind,
            producer_stage_id: StageId(stage.into()),
            status: ArtifactStatus::Valid,
            retention: RetentionPolicy::RequiredForResume,
            relative_path: PathBuf::from(path),
            size: 1,
            modified: 1,
            content_hash: Some("h".into()),
            media_type: None,
            schema_version: None,
        }
    }

    /// 测试辅助断言（仅测试使用）
    #[allow(dead_code)]
    pub fn is_valid(&self) -> bool {
        self.status == ArtifactStatus::Valid
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artifact_records_are_relative_and_validatable() {
        let a = ArtifactRecord::valid_required(
            "audio",
            ArtifactKind::ExtractedAudio,
            "extract_audio",
            "audio.wav",
        );
        assert!(a.is_valid());
        assert_eq!(a.relative_path, PathBuf::from("audio.wav"));
        assert_eq!(a.retention, RetentionPolicy::RequiredForResume);
    }
}
