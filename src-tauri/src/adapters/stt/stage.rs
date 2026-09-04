use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use crate::domain::artifact::{ArtifactKind, RetentionPolicy};
use crate::domain::ids::{ArtifactId, StageId};
use crate::pipeline::runner::{
    ArtifactOutput, ExecuteError, ExecuteFuture, ExecutionContext, ExecutionOutcome, StageExecutor,
    StageRequest,
};
use crate::ports::stt::{SttEngine, SttError};

pub struct SttStageExecutor<S: SttEngine> {
    engine: Arc<S>,
    source_language: Option<String>,
}

impl<S: SttEngine> SttStageExecutor<S> {
    pub fn new(engine: Arc<S>, source_language: Option<String>) -> Self {
        Self {
            engine,
            source_language,
        }
    }
}

impl<S: SttEngine + 'static> StageExecutor for SttStageExecutor<S> {
    fn version(&self, _stage: &StageId) -> String {
        self.engine.version()
    }

    fn execute<'a>(
        &'a self,
        request: StageRequest,
        context: ExecutionContext,
    ) -> ExecuteFuture<'a> {
        Box::pin(async move {
            if request.node.id.0 != "stt" {
                return Err(ExecuteError::Failed(format!(
                    "SttStageExecutor 不支持节点 {}",
                    request.node.id.0
                )));
            }
            let audio = request
                .input(ArtifactKind::ExtractedAudio)
                .ok_or_else(|| ExecuteError::Failed("STT 缺少提取后的音频".into()))?;
            let _lease = crate::scheduler::admit_resources(self.engine.resource_cost()).await;
            let segments = self
                .engine
                .transcribe(
                    &audio.path,
                    self.source_language.as_deref(),
                    &context.cancel,
                )
                .await
                .map_err(map_stt_error)?;
            let relative = "shared/segments.json";
            let target = context.task_root.join(relative);
            write_json_atomic(&target, &segments).map_err(ExecuteError::Failed)?;
            let metadata =
                fs::metadata(&target).map_err(|error| ExecuteError::Failed(error.to_string()))?;
            let modified = metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .and_then(|duration| i64::try_from(duration.as_millis()).ok())
                .ok_or_else(|| ExecuteError::Failed("无法读取 STT 产物修改时间".into()))?;
            Ok(ExecutionOutcome::Done(vec![ArtifactOutput {
                id: ArtifactId("parent:stt:0".into()),
                kind: ArtifactKind::Segments,
                relative_path: relative.into(),
                size: metadata.len(),
                modified,
                content_hash: format!("{}:{}:{}", self.engine.version(), segments.len(), modified),
                media_type: Some("application/json".into()),
                retention: RetentionPolicy::RequiredForResume,
            }]))
        })
    }
}

fn map_stt_error(error: SttError) -> ExecuteError {
    match error {
        SttError::Canceled => ExecuteError::Canceled,
        SttError::EmptyResult => ExecuteError::Failed(
            "STT 未识别到有效语音，请检查视频音轨、源语言或模型配置".into(),
        ),
        SttError::InvalidInput(message) => {
            ExecuteError::Failed(format!("STT 输入无效: {message}"))
        }
        SttError::Engine(message) => ExecuteError::Failed(format!("STT 引擎失败: {message}")),
    }
}

fn write_json_atomic<T: serde::Serialize>(target: &Path, value: &T) -> Result<(), String> {
    let parent = target.parent().ok_or("STT 输出没有父目录")?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let temp = target.with_extension("json.tmp");
    fs::write(
        &temp,
        serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    if target.exists() {
        fs::remove_file(target).map_err(|error| error.to_string())?;
    }
    fs::rename(temp, target).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_result_has_actionable_user_message() {
        assert!(matches!(
            map_stt_error(SttError::EmptyResult),
            ExecuteError::Failed(message)
                if message.contains("未识别到有效语音") && message.contains("源语言")
        ));
    }
    use crate::adapters::media::ffmpeg::FfmpegMediaTool;
    use crate::adapters::media::stages::MediaStageExecutor;
    use crate::domain::config::{EngineSelection, OutputConfig, PipelineConfig, SeparationConfig};
    use crate::domain::ids::{StageId, TaskId};
    use crate::domain::manifest::{StageStatus, TaskManifest};
    use crate::domain::media::SourceFingerprint;
    use crate::domain::variant::TargetVariant;
    use crate::pipeline::graph::PipelineGraph;
    use crate::pipeline::registry::StageRegistry;
    use crate::pipeline::runner::{CancelToken, PipelineRunner};
    use crate::ports::stt::{SttEngine, SttFuture};
    use crate::types::Segment;

    struct FakeStt;

    impl SttEngine for FakeStt {
        fn version(&self) -> String {
            "fake-stt-v1".into()
        }

        fn transcribe<'a>(
            &'a self,
            _audio: &'a Path,
            _source_language: Option<&'a str>,
            cancel: &'a CancelToken,
        ) -> SttFuture<'a> {
            Box::pin(async move {
                if cancel.is_canceled() {
                    return Err(SttError::Canceled);
                }
                Ok(vec![Segment {
                    idx: 0,
                    start: 0.0,
                    end: 0.2,
                    text: "hello".into(),
                    translated: String::new(),
                }])
            })
        }
    }

    fn config() -> PipelineConfig {
        PipelineConfig {
            source_language: None,
            targets: vec![TargetVariant::zh_mandarin()],
            engines: EngineSelection {
                stt: "fake".into(),
                translator: "unused".into(),
                tts: "unused".into(),
                separator: None,
            },
            separation: SeparationConfig::default(),
            output: OutputConfig::default(),
        }
    }

    #[tokio::test]
    async fn parent_dag_runs_real_media_and_fake_stt_through_registry() {
        let root = std::env::temp_dir().join(format!("parent-dag-{}", uuid::Uuid::new_v4()));
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

        let media = Arc::new(MediaStageExecutor::new(
            Arc::new(FfmpegMediaTool::default()),
        ));
        let mut registry = StageRegistry::new();
        registry.register("media_probe", media.clone()).unwrap();
        registry.register("extract_audio", media).unwrap();
        registry
            .register(
                "stt",
                Arc::new(SttStageExecutor::new(Arc::new(FakeStt), None)),
            )
            .unwrap();
        let manifest = TaskManifest::new(
            TaskId("p1".into()),
            SourceFingerprint {
                size: fs::metadata(&source).unwrap().len(),
                modified: 1,
                content_hash: Some("source".into()),
                hash_algo_version: 1,
            },
        );
        let runner = PipelineRunner::new(
            PipelineGraph::video_translation(),
            config(),
            manifest,
            Arc::new(registry),
        )
        .with_environment(&root, &source);

        runner.run_parent(&CancelToken::default()).await.unwrap();

        assert!(root.join("shared/media-info.json").is_file());
        assert!(root.join("shared/audio.wav").is_file());
        assert!(root.join("shared/segments.json").is_file());
        let snapshot = runner.manifest_snapshot().await;
        assert_eq!(
            snapshot.stages[&StageId("stt".into())].status,
            StageStatus::Done
        );
        assert_eq!(
            snapshot.stages[&StageId("separation".into())].status,
            StageStatus::Skipped
        );
        fs::remove_dir_all(root).unwrap();
    }
}
