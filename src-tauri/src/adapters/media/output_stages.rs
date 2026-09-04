use std::fs;
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, UNIX_EPOCH};

use tokio::process::Command;

use crate::domain::artifact::{ArtifactKind, RetentionPolicy};
use crate::domain::config::{OutputConfig, OutputNaming, SubtitleMode};
use crate::domain::ids::{ArtifactId, StageId};
use crate::pipeline::runner::{
    ArtifactOutput, CancelToken, ExecuteError, ExecuteFuture, ExecutionContext, ExecutionOutcome,
    RunScope, StageExecutor, StageRequest,
};
use crate::types::Segment;

#[derive(Debug, Clone)]
pub struct FfmpegOutputStages {
    ffmpeg: String,
    config: OutputConfig,
}

impl Default for FfmpegOutputStages {
    fn default() -> Self {
        Self {
            ffmpeg: "ffmpeg".into(),
            config: OutputConfig::default(),
        }
    }
}

impl FfmpegOutputStages {
    pub fn new(config: OutputConfig) -> Self {
        Self {
            ffmpeg: "ffmpeg".into(),
            config,
        }
    }
}

impl StageExecutor for FfmpegOutputStages {
    fn version(&self, _stage: &StageId) -> String {
        "ffmpeg-output-v1".into()
    }

    fn execute<'a>(
        &'a self,
        request: StageRequest,
        context: ExecutionContext,
    ) -> ExecuteFuture<'a> {
        Box::pin(async move {
            let RunScope::Target(variant) = &request.scope else {
                return Err(ExecuteError::Failed("输出节点必须属于目标版本".into()));
            };
            match request.node.id.0.as_str() {
                "srt" => write_srt_stage(&request, &context, &variant.0).await,
                "mix" => self.mix_stage(&request, &context, &variant.0).await,
                "final_video" => self.final_stage(&request, &context, &variant.0).await,
                other => Err(ExecuteError::Failed(format!("未知输出节点 {other}"))),
            }
        })
    }
}

impl FfmpegOutputStages {
    async fn mix_stage(
        &self,
        request: &StageRequest,
        context: &ExecutionContext,
        variant: &str,
    ) -> Result<ExecutionOutcome, ExecuteError> {
        let dub = request
            .input(ArtifactKind::DubAudio)
            .ok_or_else(|| ExecuteError::Failed("混音缺少 dub.wav".into()))?;
        let background = request
            .input(ArtifactKind::BackgroundNormalized)
            .or_else(|| request.input(ArtifactKind::BackgroundRaw));
        let relative = format!("targets/{variant}/mixed.wav");
        let target = context.task_root.join(&relative);
        let temp = context
            .work_dir(&format!("target:{variant}:mix"))
            .join("mixed.wav");
        if let Some(parent) = temp.parent() {
            fs::create_dir_all(parent).map_err(|error| ExecuteError::Failed(error.to_string()))?;
        }
        let mut command = Command::new(&self.ffmpeg);
        command
            .kill_on_drop(true)
            .arg("-v")
            .arg("error")
            .arg("-i")
            .arg(&dub.path);
        if let Some(background) = background {
            command
                .arg("-i")
                .arg(&background.path)
                .arg("-filter_complex")
                .arg("[0:a][1:a]amix=inputs=2:duration=longest:normalize=0[a]")
                .arg("-map")
                .arg("[a]");
        }
        command
            .args(["-acodec", "pcm_s16le", "-y"])
            .arg(&temp)
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        run_command(command, &context.cancel).await?;
        commit_file(&temp, &target).map_err(ExecuteError::Failed)?;
        Ok(ExecutionOutcome::Done(vec![file_output(
            &format!("target:{variant}:mix:0"),
            ArtifactKind::MixedAudio,
            &relative,
            &target,
            RetentionPolicy::RequiredForResume,
            "audio/wav",
        )?]))
    }

    async fn final_stage(
        &self,
        request: &StageRequest,
        context: &ExecutionContext,
        variant: &str,
    ) -> Result<ExecutionOutcome, ExecuteError> {
        if self.config.subtitle == SubtitleMode::HardSubtitlePlanned {
            return Err(ExecuteError::Failed(
                "硬字幕仍为后续支持占位，请选择无字幕或外挂 SRT".into(),
            ));
        }
        let source = request
            .input(ArtifactKind::SourceVideo)
            .ok_or_else(|| ExecuteError::Failed("视频合成缺少源视频".into()))?;
        let mixed = request
            .input(ArtifactKind::MixedAudio)
            .ok_or_else(|| ExecuteError::Failed("视频合成缺少 mixed.wav".into()))?;
        let file_name = match self.config.naming {
            OutputNaming::Final => "final.mp4".into(),
            OutputNaming::SourceVariant => {
                let source_name = source
                    .path
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .chars()
                    .map(|ch| {
                        if matches!(ch, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') {
                            '_'
                        } else {
                            ch
                        }
                    })
                    .collect::<String>();
                format!("{source_name}.{variant}.mp4")
            }
        };
        let relative = format!("targets/{variant}/{file_name}");
        let target = context.task_root.join(&relative);
        let temp = context
            .work_dir(&format!("target:{variant}:final_video"))
            .join(&file_name);
        if let Some(parent) = temp.parent() {
            fs::create_dir_all(parent).map_err(|error| ExecuteError::Failed(error.to_string()))?;
        }
        let mut command = Command::new(&self.ffmpeg);
        command
            .kill_on_drop(true)
            .arg("-v")
            .arg("error")
            .arg("-i")
            .arg(&source.path)
            .arg("-i")
            .arg(&mixed.path)
            .arg("-filter_complex")
            .arg("[1:a]apad[new_audio]")
            .arg("-map")
            .arg("0:v:0");
        if self.config.keep_original_audio_track {
            command.arg("-map").arg("0:a:0?");
        }
        command
            .arg("-map")
            .arg("[new_audio]")
            .args(["-c:v", "copy", "-c:a", "aac", "-shortest", "-y"])
            .arg(&temp)
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        run_command(command, &context.cancel).await?;
        commit_file(&temp, &target).map_err(ExecuteError::Failed)?;
        Ok(ExecutionOutcome::Done(vec![file_output(
            &format!("target:{variant}:final_video:0"),
            ArtifactKind::FinalVideo,
            &relative,
            &target,
            RetentionPolicy::FinalOutput,
            "video/mp4",
        )?]))
    }
}

async fn write_srt_stage(
    request: &StageRequest,
    context: &ExecutionContext,
    variant: &str,
) -> Result<ExecutionOutcome, ExecuteError> {
    let translated = request
        .input(ArtifactKind::TranslatedSegments)
        .ok_or_else(|| ExecuteError::Failed("SRT 缺少 translated segments".into()))?;
    let segments: Vec<Segment> = serde_json::from_slice(
        &fs::read(&translated.path).map_err(|error| ExecuteError::Failed(error.to_string()))?,
    )
    .map_err(|error| ExecuteError::Failed(error.to_string()))?;
    let relative = format!("targets/{variant}/translated.srt");
    let target = context.task_root.join(&relative);
    let temp = context
        .work_dir(&format!("target:{variant}:srt"))
        .join("translated.srt");
    if let Some(parent) = temp.parent() {
        fs::create_dir_all(parent).map_err(|error| ExecuteError::Failed(error.to_string()))?;
    }
    crate::subtitle::write_srt(&segments, &temp)
        .await
        .map_err(ExecuteError::Failed)?;
    commit_file(&temp, &target).map_err(ExecuteError::Failed)?;
    Ok(ExecutionOutcome::Done(vec![file_output(
        &format!("target:{variant}:srt:0"),
        ArtifactKind::SubtitleSrt,
        &relative,
        &target,
        RetentionPolicy::FinalOutput,
        "application/x-subrip",
    )?]))
}

async fn run_command(mut command: Command, cancel: &CancelToken) -> Result<(), ExecuteError> {
    let mut child = command
        .spawn()
        .map_err(|error| ExecuteError::Failed(format!("ffmpeg 启动失败: {error}")))?;
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
                return Err(ExecuteError::Failed(format!(
                    "ffmpeg 输出阶段失败: {status}"
                )))
            }
            None => tokio::time::sleep(Duration::from_millis(20)).await,
        }
    }
}

fn commit_file(source: &Path, target: &Path) -> Result<(), String> {
    let parent = target.parent().ok_or("输出目标没有父目录")?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    if target.exists() {
        fs::remove_file(target).map_err(|error| error.to_string())?;
    }
    fs::rename(source, target).map_err(|error| error.to_string())
}

fn file_output(
    id: &str,
    kind: ArtifactKind,
    relative: &str,
    target: &Path,
    retention: RetentionPolicy,
    media_type: &str,
) -> Result<ArtifactOutput, ExecuteError> {
    let metadata = fs::metadata(target).map_err(|error| ExecuteError::Failed(error.to_string()))?;
    let modified = metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .and_then(|value| i64::try_from(value.as_millis()).ok())
        .ok_or_else(|| ExecuteError::Failed("无法读取输出产物修改时间".into()))?;
    Ok(ArtifactOutput {
        id: ArtifactId(id.into()),
        kind,
        relative_path: relative.into(),
        size: metadata.len(),
        modified,
        content_hash: format!("ffmpeg-output-v1:{}:{modified}", metadata.len()),
        media_type: Some(media_type.into()),
        retention,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::graph::{NodeScope, StageNode};
    use crate::pipeline::runner::{ArtifactInput, ExecutionContext};

    fn input(id: &str, kind: ArtifactKind, path: &Path) -> ArtifactInput {
        ArtifactInput {
            id: ArtifactId(id.into()),
            kind,
            path: path.to_owned(),
            content_hash: Some("h".into()),
        }
    }

    #[tokio::test]
    async fn real_srt_mix_and_final_video_stages_complete() {
        let root = std::env::temp_dir().join(format!("output-stages-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let source = root.join("source.mp4");
        let dub = root.join("dub.wav");
        let translated = root.join("translated.json");
        let status = Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=c=black:s=160x90:d=0.6",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=0.6",
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
        let status = Command::new("ffmpeg")
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
        fs::write(
            &translated,
            serde_json::to_vec(&vec![Segment {
                idx: 0,
                start: 0.0,
                end: 0.6,
                text: "hello".into(),
                translated: "你好".into(),
            }])
            .unwrap(),
        )
        .unwrap();
        let executor = FfmpegOutputStages::default();
        let context = ExecutionContext {
            task_root: root.clone(),
            cancel: CancelToken::default(),
        };
        let scope = RunScope::Target(crate::domain::ids::VariantId("zh-CN".into()));

        executor
            .execute(
                StageRequest {
                    node: StageNode::new(
                        "srt",
                        NodeScope::Target,
                        &["translate"],
                        vec![ArtifactKind::SubtitleSrt],
                    ),
                    scope: scope.clone(),
                    inputs: vec![input(
                        "translated",
                        ArtifactKind::TranslatedSegments,
                        &translated,
                    )],
                },
                context.clone(),
            )
            .await
            .unwrap();
        executor
            .execute(
                StageRequest {
                    node: StageNode::new(
                        "mix",
                        NodeScope::Target,
                        &["tts", "separation"],
                        vec![ArtifactKind::MixedAudio],
                    ),
                    scope: scope.clone(),
                    inputs: vec![input("dub", ArtifactKind::DubAudio, &dub)],
                },
                context.clone(),
            )
            .await
            .unwrap();
        let mixed = root.join("targets/zh-CN/mixed.wav");
        executor
            .execute(
                StageRequest {
                    node: StageNode::new(
                        "final_video",
                        NodeScope::Target,
                        &["mix", "srt"],
                        vec![ArtifactKind::FinalVideo],
                    ),
                    scope,
                    inputs: vec![
                        input("source", ArtifactKind::SourceVideo, &source),
                        input("mixed", ArtifactKind::MixedAudio, &mixed),
                    ],
                },
                context,
            )
            .await
            .unwrap();

        assert!(root.join("targets/zh-CN/translated.srt").is_file());
        assert!(mixed.is_file());
        let final_video = root.join("targets/zh-CN/source.zh-CN.mp4");
        assert!(final_video.is_file());
        let probe: serde_json::Value = serde_json::from_slice(
            &Command::new("ffprobe")
                .args([
                    "-v",
                    "error",
                    "-show_entries",
                    "format=duration:stream=codec_type",
                    "-of",
                    "json",
                ])
                .arg(&final_video)
                .output()
                .await
                .unwrap()
                .stdout,
        )
        .unwrap();
        let duration: f64 = probe["format"]["duration"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        let audio_tracks = probe["streams"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|stream| stream["codec_type"] == "audio")
            .count();
        assert!(duration >= 0.5, "短配音不能截短原视频");
        assert_eq!(audio_tracks, 1, "默认只保留新配音轨");

        let keep_original = FfmpegOutputStages::new(OutputConfig {
            keep_original_audio_track: true,
            ..OutputConfig::default()
        });
        keep_original
            .execute(
                StageRequest {
                    node: StageNode::new(
                        "final_video",
                        NodeScope::Target,
                        &["mix", "srt"],
                        vec![ArtifactKind::FinalVideo],
                    ),
                    scope: RunScope::Target(crate::domain::ids::VariantId("zh-CN".into())),
                    inputs: vec![
                        input("source", ArtifactKind::SourceVideo, &source),
                        input("mixed", ArtifactKind::MixedAudio, &mixed),
                    ],
                },
                ExecutionContext {
                    task_root: root.clone(),
                    cancel: CancelToken::default(),
                },
            )
            .await
            .unwrap();
        let probe: serde_json::Value = serde_json::from_slice(
            &Command::new("ffprobe")
                .args([
                    "-v",
                    "error",
                    "-show_entries",
                    "stream=codec_type",
                    "-of",
                    "json",
                ])
                .arg(&final_video)
                .output()
                .await
                .unwrap()
                .stdout,
        )
        .unwrap();
        let audio_tracks = probe["streams"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|stream| stream["codec_type"] == "audio")
            .count();
        assert_eq!(audio_tracks, 2, "开启后应保留原音轨并添加新配音轨");
        fs::remove_dir_all(root).unwrap();
    }

    /// 真实 ffmpeg 冒烟：mix（配音+背景混音）→ final_video（合成成片）两步真实执行。
    /// 这条路径是「用户跑完整任务才炸」的高频区（ffmpeg 参数/滤镜拼接错）。
    /// ffmpeg 不在 PATH 时优雅跳过，保持 CI 友好。
    #[tokio::test]
    async fn real_ffmpeg_mix_then_final_video() {
        use crate::domain::ids::{ArtifactId, VariantId};
        use crate::pipeline::graph::{NodeScope, StageNode};
        use crate::pipeline::runner::{
            ArtifactInput, CancelToken, ExecutionContext, RunScope, StageExecutor, StageRequest,
        };

        let ffmpeg_ok = Command::new("ffmpeg")
            .arg("-version")
            .output()
            .await
            .map(|out| out.status.success())
            .unwrap_or(false);
        if !ffmpeg_ok {
            eprintln!("[output-smoke] 跳过：系统 PATH 没有 ffmpeg");
            return;
        }

        let root = std::env::temp_dir().join(format!("vt-output-smoke-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();

        // 造输入：2s 配音 + 2s 背景 + 2s 带音轨的源视频（全部真 ffmpeg 编码）
        let dub = root.join("dub.wav");
        let bg = root.join("bg.wav");
        let source = root.join("source.mp4");
        for (output, args) in [
            (
                &dub,
                &["-f", "lavfi", "-i", "sine=frequency=440:duration=2", "-ac", "1", "-ar",
                    "44100"][..],
            ),
            (
                &bg,
                &["-f", "lavfi", "-i", "sine=frequency=220:duration=2", "-ac", "1", "-ar",
                    "44100"][..],
            ),
            (
                &source,
                &["-f", "lavfi", "-i", "color=c=black:s=160x90:d=2", "-f", "lavfi", "-i",
                    "sine=frequency=330:duration=2", "-shortest", "-c:v", "libx264", "-c:a",
                    "aac"][..],
            ),
        ] {
            let status = Command::new("ffmpeg")
                .arg("-v")
                .arg("error")
                .args(args)
                .arg("-y")
                .arg(output)
                .status()
                .await
                .unwrap();
            assert!(status.success(), "生成测试媒体失败: {}", output.display());
        }

        let stages = FfmpegOutputStages::new(OutputConfig {
            naming: OutputNaming::Final,
            ..OutputConfig::default()
        });
        let variant = VariantId("zh-CN".into());
        let context = ExecutionContext {
            task_root: root.clone(),
            cancel: CancelToken::default(),
        };
        let mk_input = |kind: ArtifactKind, path: std::path::PathBuf| ArtifactInput {
            id: ArtifactId(format!("{kind:?}").to_lowercase()),
            kind,
            path,
            content_hash: None,
        };

        // mix：dub + background → mixed.wav
        let mix_request = StageRequest {
            node: StageNode::new("mix", NodeScope::Target, &[], vec![ArtifactKind::MixedAudio]),
            scope: RunScope::Target(variant.clone()),
            inputs: vec![
                mk_input(ArtifactKind::DubAudio, dub),
                mk_input(ArtifactKind::BackgroundNormalized, bg),
            ],
        };
        let mix_outcome = stages
            .execute(mix_request, context.clone())
            .await
            .unwrap_or_else(|error| panic!("mix 阶段失败: {error:?}"));
        let mixed = root.join("targets/zh-CN/mixed.wav");
        assert!(mixed.is_file(), "mixed.wav 未产出");
        assert!(
            matches!(
                mix_outcome,
                crate::pipeline::runner::ExecutionOutcome::Done(_)
            ),
            "mix 应返回 Done"
        );

        // final_video：source + mixed → final.mp4（含新音轨）
        let final_request = StageRequest {
            node: StageNode::new(
                "final_video",
                NodeScope::Target,
                &[],
                vec![ArtifactKind::FinalVideo],
            ),
            scope: RunScope::Target(variant),
            inputs: vec![
                mk_input(ArtifactKind::SourceVideo, source),
                mk_input(ArtifactKind::MixedAudio, mixed),
            ],
        };
        stages
            .execute(final_request, context)
            .await
            .unwrap_or_else(|error| panic!("final_video 阶段失败: {error:?}"));
        let final_path = root.join("targets/zh-CN/final.mp4");
        assert!(final_path.is_file(), "final.mp4 未产出");

        // ffprobe 验证成片音轨数 = 1（原视频音轨未保留，只留配音轨）
        let probe: serde_json::Value = serde_json::from_slice(
            &Command::new("ffprobe")
                .args(["-v", "error", "-show_entries", "stream=codec_type", "-of", "json"])
                .arg(&final_path)
                .output()
                .await
                .unwrap()
                .stdout,
        )
        .expect("ffprobe 输出解析失败");
        let audio_tracks = probe["streams"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|stream| stream["codec_type"] == "audio")
            .count();
        assert_eq!(audio_tracks, 1, "非保留原声时成片应只有一条新配音轨");

        std::fs::remove_dir_all(root).unwrap();
    }
}
