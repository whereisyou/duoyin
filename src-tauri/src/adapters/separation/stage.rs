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
use crate::ports::separator::{validate_separation_output, AudioSeparator, SeparatorError};

pub struct SeparationStageExecutor<S: AudioSeparator> {
    separator: Arc<S>,
    denoise: bool,
    normalize: bool,
}

impl<S: AudioSeparator> SeparationStageExecutor<S> {
    pub fn new(separator: Arc<S>) -> Self {
        Self {
            separator,
            denoise: false,
            normalize: false,
        }
    }

    pub fn with_postprocess(mut self, denoise: bool, normalize: bool) -> Self {
        self.denoise = denoise;
        self.normalize = normalize;
        self
    }
}

impl<S: AudioSeparator + 'static> StageExecutor for SeparationStageExecutor<S> {
    fn version(&self, _stage: &StageId) -> String {
        self.separator.version()
    }

    fn execute<'a>(
        &'a self,
        request: StageRequest,
        context: ExecutionContext,
    ) -> ExecuteFuture<'a> {
        Box::pin(async move {
            if request.node.id.0 != "separation" {
                return Err(ExecuteError::Failed("分离执行器收到错误节点".into()));
            }
            let audio = request
                .input(ArtifactKind::SourceVideo)
                .ok_or_else(|| ExecuteError::Failed("背景分离缺少源视频".into()))?;
            let staging = context.work_dir("parent:separation");
            if staging.exists() {
                fs::remove_dir_all(&staging)
                    .map_err(|error| ExecuteError::Failed(error.to_string()))?;
            }
            fs::create_dir_all(&staging)
                .map_err(|error| ExecuteError::Failed(error.to_string()))?;
            let _lease = crate::scheduler::admit_resources(self.separator.resource_cost()).await;
            let separated = self
                .separator
                .separate(&audio.path, &staging, &context.cancel)
                .await
                .map_err(map_error)?;
            validate_separation_output(&separated, &staging).map_err(map_error)?;

            let vocals_relative = "shared/vocals.raw.wav";
            let background_relative = "shared/bgm.raw.wav";
            let vocals_target = context.task_root.join(vocals_relative);
            let background_target = context.task_root.join(background_relative);
            commit_file(&separated.vocals, &vocals_target).map_err(ExecuteError::Failed)?;
            commit_file(&separated.background, &background_target).map_err(ExecuteError::Failed)?;
            let mut outputs = vec![
                output(
                    "parent:separation:vocals",
                    ArtifactKind::VocalsRaw,
                    vocals_relative,
                    &vocals_target,
                    self.separator.version(),
                )?,
                output(
                    "parent:separation:background",
                    ArtifactKind::BackgroundRaw,
                    background_relative,
                    &background_target,
                    self.separator.version(),
                )?,
            ];
            if self.denoise || self.normalize {
                let vocals_normalized = context.task_root.join("shared/vocals.normalized.wav");
                let background_normalized = context.task_root.join("shared/bgm.normalized.wav");
                postprocess_audio(
                    &vocals_target,
                    &vocals_normalized,
                    self.denoise,
                    self.normalize,
                    &context.cancel,
                )
                .await?;
                postprocess_audio(
                    &background_target,
                    &background_normalized,
                    self.denoise,
                    self.normalize,
                    &context.cancel,
                )
                .await?;
                outputs.push(output(
                    "parent:separation:vocals-normalized",
                    ArtifactKind::VocalsNormalized,
                    "shared/vocals.normalized.wav",
                    &vocals_normalized,
                    self.separator.version(),
                )?);
                outputs.push(output(
                    "parent:separation:background-normalized",
                    ArtifactKind::BackgroundNormalized,
                    "shared/bgm.normalized.wav",
                    &background_normalized,
                    self.separator.version(),
                )?);
            }
            Ok(ExecutionOutcome::Done(outputs))
        })
    }
}

async fn postprocess_audio(
    input: &Path,
    output: &Path,
    denoise: bool,
    normalize: bool,
    cancel: &crate::pipeline::runner::CancelToken,
) -> Result<(), ExecuteError> {
    let mut filters = Vec::new();
    if denoise {
        filters.push("afftdn=nf=-25");
    }
    if normalize {
        filters.push("loudnorm=I=-16:TP=-1.5:LRA=11");
    }
    let mut command = tokio::process::Command::new("ffmpeg");
    command
        .kill_on_drop(true)
        .args(["-v", "error", "-i"])
        .arg(input)
        .arg("-af")
        .arg(filters.join(","))
        .args(["-c:a", "pcm_s16le", "-y"])
        .arg(output);
    let mut child = command
        .spawn()
        .map_err(|error| ExecuteError::Failed(error.to_string()))?;
    loop {
        if cancel.is_canceled() {
            child
                .kill()
                .await
                .map_err(|error| ExecuteError::Failed(error.to_string()))?;
            return Err(ExecuteError::Canceled);
        }
        match child
            .try_wait()
            .map_err(|error| ExecuteError::Failed(error.to_string()))?
        {
            Some(status) if status.success() => return Ok(()),
            Some(status) => {
                return Err(ExecuteError::Failed(format!("背景音后处理失败: {status}")))
            }
            None => tokio::time::sleep(std::time::Duration::from_millis(20)).await,
        }
    }
}

fn map_error(error: SeparatorError) -> ExecuteError {
    match error {
        SeparatorError::Canceled => ExecuteError::Canceled,
        other => ExecuteError::Failed(format!("背景音分离失败: {other:?}")),
    }
}

fn commit_file(source: &Path, target: &Path) -> Result<(), String> {
    let parent = target.parent().ok_or("分离目标没有父目录")?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    if target.exists() {
        fs::remove_file(target).map_err(|error| error.to_string())?;
    }
    fs::rename(source, target).map_err(|error| error.to_string())
}

fn output(
    id: &str,
    kind: ArtifactKind,
    relative: &str,
    target: &Path,
    engine_version: String,
) -> Result<ArtifactOutput, ExecuteError> {
    let metadata = fs::metadata(target).map_err(|error| ExecuteError::Failed(error.to_string()))?;
    let modified = metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .and_then(|value| i64::try_from(value.as_millis()).ok())
        .ok_or_else(|| ExecuteError::Failed("无法读取分离产物修改时间".into()))?;
    Ok(ArtifactOutput {
        id: ArtifactId(id.into()),
        kind,
        relative_path: relative.into(),
        size: metadata.len(),
        modified,
        content_hash: format!("{engine_version}:{}:{modified}", metadata.len()),
        media_type: Some("audio/wav".into()),
        retention: RetentionPolicy::RequiredForResume,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::media::output_stages::FfmpegOutputStages;
    use crate::pipeline::graph::{NodeScope, StageNode};
    use crate::pipeline::runner::{ArtifactInput, CancelToken, ExecutionContext, RunScope};
    use crate::ports::separator::{SeparationOutput, SeparatorFuture};

    struct FakeSeparator {
        fail_background: bool,
    }

    impl AudioSeparator for FakeSeparator {
        fn version(&self) -> String {
            "fake-separator-v1".into()
        }

        fn separate<'a>(
            &'a self,
            _input: &'a Path,
            staging_dir: &'a Path,
            _cancel: &'a CancelToken,
        ) -> SeparatorFuture<'a> {
            Box::pin(async move {
                let vocals = staging_dir.join("vocals.wav");
                let background = staging_dir.join("background.wav");
                fs::write(&vocals, vec![0u8; 64]).unwrap();
                if !self.fail_background {
                    fs::write(&background, vec![0u8; 64]).unwrap();
                }
                Ok(SeparationOutput { vocals, background })
            })
        }
    }

    fn request(input: &Path) -> StageRequest {
        StageRequest {
            node: StageNode::new(
                "separation",
                NodeScope::Parent,
                &["media_probe"],
                vec![ArtifactKind::VocalsRaw, ArtifactKind::BackgroundRaw],
            ),
            scope: RunScope::Parent,
            inputs: vec![ArtifactInput {
                id: ArtifactId("audio".into()),
                kind: ArtifactKind::SourceVideo,
                path: input.to_owned(),
                content_hash: Some("h".into()),
            }],
        }
    }

    #[tokio::test]
    async fn commits_both_outputs_only_after_validation() {
        let root = std::env::temp_dir().join(format!("separation-stage-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let input = root.join("audio.wav");
        fs::write(&input, vec![0u8; 64]).unwrap();
        let executor = SeparationStageExecutor::new(Arc::new(FakeSeparator {
            fail_background: false,
        }));

        let result = executor
            .execute(
                request(&input),
                ExecutionContext {
                    task_root: root.clone(),
                    cancel: CancelToken::default(),
                },
            )
            .await
            .unwrap();

        assert!(matches!(result, ExecutionOutcome::Done(ref outputs) if outputs.len() == 2));
        assert!(root.join("shared/vocals.raw.wav").is_file());
        assert!(root.join("shared/bgm.raw.wav").is_file());
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn separated_background_can_be_mixed_while_vocals_remain_preserved() {
        let root = std::env::temp_dir().join(format!("separation-mix-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let input = root.join("audio.wav");
        let dub = root.join("dub.wav");
        fs::write(&input, vec![0u8; 64]).unwrap();
        let status = tokio::process::Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=880:duration=0.2",
                "-acodec",
                "pcm_s16le",
                "-y",
            ])
            .arg(&dub)
            .status()
            .await
            .unwrap();
        assert!(status.success());

        struct WavSeparator;
        impl AudioSeparator for WavSeparator {
            fn version(&self) -> String {
                "fake-wav-v1".into()
            }
            fn separate<'a>(
                &'a self,
                _input: &'a Path,
                staging_dir: &'a Path,
                _cancel: &'a CancelToken,
            ) -> SeparatorFuture<'a> {
                Box::pin(async move {
                    fs::create_dir_all(staging_dir).unwrap();
                    let vocals = staging_dir.join("vocals.wav");
                    let background = staging_dir.join("background.wav");
                    for (path, frequency) in [(&vocals, "440"), (&background, "220")] {
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
                            .arg(path)
                            .status()
                            .await
                            .unwrap();
                        assert!(status.success());
                    }
                    Ok(SeparationOutput { vocals, background })
                })
            }
        }

        let separator = SeparationStageExecutor::new(Arc::new(WavSeparator));
        let separation = separator
            .execute(
                request(&input),
                ExecutionContext {
                    task_root: root.clone(),
                    cancel: CancelToken::default(),
                },
            )
            .await
            .unwrap();
        let ExecutionOutcome::Done(outputs) = separation else {
            panic!("expected outputs")
        };
        let background = root.join(
            &outputs
                .iter()
                .find(|o| o.kind == ArtifactKind::BackgroundRaw)
                .unwrap()
                .relative_path,
        );
        let output_stages = FfmpegOutputStages::default();
        let result = output_stages
            .execute(
                StageRequest {
                    node: StageNode::new(
                        "mix",
                        NodeScope::Target,
                        &["tts", "separation"],
                        vec![ArtifactKind::MixedAudio],
                    ),
                    scope: RunScope::Target(crate::domain::ids::VariantId("zh-CN".into())),
                    inputs: vec![
                        ArtifactInput {
                            id: ArtifactId("dub".into()),
                            kind: ArtifactKind::DubAudio,
                            path: dub,
                            content_hash: Some("dub".into()),
                        },
                        ArtifactInput {
                            id: ArtifactId("bgm".into()),
                            kind: ArtifactKind::BackgroundRaw,
                            path: background,
                            content_hash: Some("bgm".into()),
                        },
                    ],
                },
                ExecutionContext {
                    task_root: root.clone(),
                    cancel: CancelToken::default(),
                },
            )
            .await;

        assert!(result.is_ok());
        assert!(root.join("shared/vocals.raw.wav").is_file());
        assert!(root.join("shared/bgm.raw.wav").is_file());
        assert!(root.join("targets/zh-CN/mixed.wav").is_file());
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn missing_second_output_commits_neither_file() {
        let root = std::env::temp_dir().join(format!("separation-stage-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let input = root.join("audio.wav");
        fs::write(&input, vec![0u8; 64]).unwrap();
        let executor = SeparationStageExecutor::new(Arc::new(FakeSeparator {
            fail_background: true,
        }));

        assert!(executor
            .execute(
                request(&input),
                ExecutionContext {
                    task_root: root.clone(),
                    cancel: CancelToken::default(),
                },
            )
            .await
            .is_err());
        assert!(!root.join("shared/vocals.raw.wav").exists());
        assert!(!root.join("shared/bgm.raw.wav").exists());
        fs::remove_dir_all(root).unwrap();
    }
}
