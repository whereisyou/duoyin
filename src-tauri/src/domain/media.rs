use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// 源文件身份。恢复任务时先比较廉价元数据，必要时再比较内容哈希。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceFingerprint {
    pub size: u64,
    pub modified: i64,
    pub content_hash: Option<String>,
    pub hash_algo_version: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceVideo {
    /// 源视频位于任务目录之外，因此这里允许绝对路径；产物路径仍只能是相对路径。
    pub path: PathBuf,
    pub fingerprint: SourceFingerprint,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaInfo {
    pub duration_ms: u64,
    pub width: u32,
    pub height: u32,
    pub video_codec: String,
    #[serde(default)]
    pub frame_rate_milli: Option<u32>,
    #[serde(default)]
    pub source_size: u64,
    #[serde(default)]
    pub audio_track_count: u16,
    pub audio_codec: Option<String>,
    pub audio_sample_rate: Option<u32>,
    pub audio_channels: Option<u16>,
}

impl SourceFingerprint {
    /// 恢复时比较源文件身份；生产恢复路径当前未接线这两条（先比 metadata 再比 hash），保留
    #[allow(dead_code)]
    pub fn matches_metadata(&self, other: &Self) -> bool {
        self.size == other.size && self.modified == other.modified
    }

    #[allow(dead_code)]
    pub fn matches_content(&self, other: &Self) -> bool {
        self.hash_algo_version == other.hash_algo_version
            && self.content_hash.is_some()
            && self.content_hash == other.content_hash
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fingerprint(hash: Option<&str>) -> SourceFingerprint {
        SourceFingerprint {
            size: 100,
            modified: 20,
            content_hash: hash.map(str::to_owned),
            hash_algo_version: 1,
        }
    }

    #[test]
    fn metadata_match_is_a_cheap_first_pass() {
        let mut changed = fingerprint(Some("new"));
        changed.modified = 21;

        assert!(fingerprint(Some("old")).matches_metadata(&fingerprint(Some("new"))));
        assert!(!fingerprint(Some("old")).matches_metadata(&changed));
    }

    #[test]
    fn content_match_requires_hash_and_same_algorithm() {
        assert!(fingerprint(Some("same")).matches_content(&fingerprint(Some("same"))));
        assert!(!fingerprint(None).matches_content(&fingerprint(None)));

        let mut newer_algorithm = fingerprint(Some("same"));
        newer_algorithm.hash_algo_version = 2;
        assert!(!fingerprint(Some("same")).matches_content(&newer_algorithm));
    }
}
