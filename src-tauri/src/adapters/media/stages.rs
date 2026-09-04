use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use crate::domain::artifact::{ArtifactKind, RetentionPolicy};
use crate::domain::ids::ArtifactId;
use crate::pipeline::runner::{
    ArtifactOutput, ExecuteError, ExecuteFuture, ExecutionContext, ExecutionOutcome, StageExecutor,
    StageRequest,
};
use crate::ports::media_tool::MediaTool;

pub struct MediaStageExecutor<M: MediaTool> {
    media: Arc<M>,
}

impl<M: MediaTool> MediaStageExecutor<M> {
    pub fn new(media: Arc<M>) -> Self {
        Self { media }
    }
}

impl<M: MediaTool + 'static> StageExecutor for MediaStageExecutor<M> {
    fn version(&self, _stage: &crate::domain::ids::StageId) -> String {
        "ffmpeg-media-v1".into()
    }

    fn execute<'a>(
        &'a self,
        request: StageRequest,
        context: ExecutionContext,
    ) -> ExecuteFuture<'a> {
        Box::pin(async move {
            match request.node.id.0.as_str() {
                "media_probe" => {
                    let source = request
                        .input(ArtifactKind::SourceVideo)
                        .ok_or_else(|| ExecuteError::Failed("media_probe 缺少源视频".into()))?;
                    let info = self
                        .media
                        .probe(&source.path, &context.cancel)
                        .await
                        .map_err(|error| {
                            ExecuteError::Failed(format!("ffprobe 失败: {error:?}"))
                        })?;
                    let relative = "shared/media-info.json";
                    let target = context.task_root.join(relative);
                    write_json_atomic(&target, &info).map_err(ExecuteError::Failed)?;
                    Ok(ExecutionOutcome::Done(vec![file_output(
                        "parent:media_probe:0",
                        ArtifactKind::MediaInfo,
                        relative,
                        &target,
                        "application/json",
                    )?]))
                }
                "extract_audio" => {
                    let source = request
                        .input(ArtifactKind::SourceVideo)
                        .ok_or_else(|| ExecuteError::Failed("extract_audio 缺少源视频".into()))?;
                    let relative = "shared/audio.wav";
                    let target = context.task_root.join(relative);
                    let temp = context.work_dir("parent:extract_audio").join("audio.wav");
                    self.media
                        .extract_stt_audio(&source.path, &temp, &context.cancel)
                        .await
                        .map_err(|error| {
                            ExecuteError::Failed(format!("提取音频失败: {error:?}"))
                        })?;
                    commit_file(&temp, &target).map_err(ExecuteError::Failed)?;
                    Ok(ExecutionOutcome::Done(vec![file_output(
                        "parent:extract_audio:0",
                        ArtifactKind::ExtractedAudio,
                        relative,
                        &target,
                        "audio/wav",
                    )?]))
                }
                other => Err(ExecuteError::Failed(format!(
                    "MediaStageExecutor 不支持节点 {other}"
                ))),
            }
        })
    }
}

fn write_json_atomic<T: serde::Serialize>(target: &Path, value: &T) -> Result<(), String> {
    let parent = target.parent().ok_or("输出文件没有父目录")?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let temp = target.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    fs::write(&temp, bytes).map_err(|error| error.to_string())?;
    commit_file(&temp, target)
}

fn commit_file(temp: &Path, target: &Path) -> Result<(), String> {
    let parent = target.parent().ok_or("输出文件没有父目录")?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    if target.exists() {
        fs::remove_file(target).map_err(|error| error.to_string())?;
    }
    fs::rename(temp, target).map_err(|error| error.to_string())
}

fn file_output(
    id: &str,
    kind: ArtifactKind,
    relative_path: &str,
    target: &Path,
    media_type: &str,
) -> Result<ArtifactOutput, ExecuteError> {
    let metadata = fs::metadata(target).map_err(|error| ExecuteError::Failed(error.to_string()))?;
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .ok_or_else(|| ExecuteError::Failed("无法读取产物修改时间".into()))?;
    Ok(ArtifactOutput {
        id: ArtifactId(id.into()),
        kind,
        relative_path: relative_path.into(),
        size: metadata.len(),
        modified,
        content_hash: format!("metadata:{}:{modified}", metadata.len()),
        media_type: Some(media_type.into()),
        retention: RetentionPolicy::RequiredForResume,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::media::ffmpeg::FfmpegMediaTool;
    use crate::domain::config::{EngineSelection, OutputConfig, PipelineConfig, SeparationConfig};
    use crate::domain::ids::{StageId, TaskId};
    use crate::domain::manifest::TaskManifest;
    use crate::domain::media::SourceFingerprint;
    use crate::domain::variant::TargetVariant;
    use crate::pipeline::graph::PipelineGraph;
    use crate::pipeline::runner::{CancelToken, PipelineRunner, RunScope};

    fn config() -> PipelineConfig {
        PipelineConfig {
            source_language: None,
            targets: vec![TargetVariant::zh_mandarin()],
            engines: EngineSelection {
                stt: "unused".into(),
                translator: "unused".into(),
                tts: "unused".into(),
                separator: None,
            },
            separation: SeparationConfig::default(),
            output: OutputConfig::default(),
        }
    }

    #[tokio::test]
    async fn real_media_stages_write_committed_artifacts() {
        let root = std::env::temp_dir().join(format!("media-stages-{}", uuid::Uuid::new_v4()));
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
        let source_meta = fs::metadata(&source).unwrap();
        let manifest = TaskManifest::new(
            TaskId("p1".into()),
            SourceFingerprint {
                size: source_meta.len(),
                modified: 1,
                content_hash: Some("source-hash".into()),
                hash_algo_version: 1,
            },
        );
        let runner = PipelineRunner::new(
            PipelineGraph::video_translation(),
            config(),
            manifest,
            Arc::new(MediaStageExecutor::new(
                Arc::new(FfmpegMediaTool::default()),
            )),
        )
        .with_environment(&root, &source);
        let cancel = CancelToken::default();

        runner
            .run_named(RunScope::Parent, "media_probe", &cancel)
            .await
            .unwrap();
        runner
            .run_named(RunScope::Parent, "extract_audio", &cancel)
            .await
            .unwrap();

        assert!(root.join("shared/media-info.json").is_file());
        assert!(root.join("shared/audio.wav").is_file());
        let snapshot = runner.manifest_snapshot().await;
        assert_eq!(
            snapshot.stages[&StageId("extract_audio".into())].artifact_ids,
            vec![ArtifactId("parent:extract_audio:0".into())]
        );
        assert!(snapshot.artifacts[&ArtifactId("parent:extract_audio:0".into())].size > 44);

        fs::remove_dir_all(root).unwrap();
    }
}
