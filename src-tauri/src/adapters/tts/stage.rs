use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use crate::domain::artifact::{ArtifactKind, RetentionPolicy};
use crate::domain::ids::{ArtifactId, VariantId};
use crate::domain::variant::TargetVariant;
use crate::pipeline::runner::{
    ArtifactOutput, ExecuteError, ExecuteFuture, ExecutionContext, ExecutionOutcome, RunScope,
    StageExecutor, StageRequest,
};
use crate::ports::tts::{TtsAlignment, TtsEngine, TtsError};
use crate::types::Segment;

pub struct TtsStageExecutor<T: TtsEngine> {
    engine: Arc<T>,
    variants: BTreeMap<VariantId, TargetVariant>,
    alignment: TtsAlignment,
    /// 配音克隆原视频音色：执行前从 shared 产物自动提取参考段并注入引擎
    use_video_prompt: bool,
}

impl<T: TtsEngine> TtsStageExecutor<T> {
    pub fn new(
        engine: Arc<T>,
        variants: impl IntoIterator<Item = TargetVariant>,
        alignment: TtsAlignment,
        use_video_prompt: bool,
    ) -> Self {
        Self {
            engine,
            alignment,
            use_video_prompt,
            variants: variants
                .into_iter()
                .map(|variant| (variant.id.clone(), variant))
                .collect(),
        }
    }
}

impl<T: TtsEngine + 'static> StageExecutor for TtsStageExecutor<T> {
    fn version(&self, _stage: &crate::domain::ids::StageId) -> String {
        self.engine.version()
    }

    fn execute<'a>(
        &'a self,
        request: StageRequest,
        context: ExecutionContext,
    ) -> ExecuteFuture<'a> {
        Box::pin(async move {
            if request.node.id.0 != "tts" {
                return Err(ExecuteError::Failed("TTS 执行器收到错误节点".into()));
            }
            let RunScope::Target(variant_id) = &request.scope else {
                return Err(ExecuteError::Failed("TTS 节点必须属于目标版本".into()));
            };
            let variant = self
                .variants
                .get(variant_id)
                .ok_or_else(|| ExecuteError::Failed(format!("未知目标版本 {}", variant_id.0)))?;
            let translated = request
                .input(ArtifactKind::TranslatedSegments)
                .ok_or_else(|| ExecuteError::Failed("TTS 缺少 translated segments".into()))?;
            let segments: Vec<Segment> = serde_json::from_slice(
                &fs::read(&translated.path)
                    .map_err(|error| ExecuteError::Failed(error.to_string()))?,
            )
            .map_err(|error| ExecuteError::Failed(error.to_string()))?;
            let work_dir = context.work_dir(&format!("target:{}:tts", variant_id.0));
            fs::create_dir_all(&work_dir)
                .map_err(|error| ExecuteError::Failed(error.to_string()))?;
            // 任务级参考音色：从原视频原声自动提取并注入零样本引擎（失败回退全局参考，不中断任务）
            if self.use_video_prompt {
                apply_task_reference(&self.engine, &context.task_root);
            }
            let _lease = crate::scheduler::admit_resources(self.engine.resource_cost()).await;
            let synthesized = self
                .engine
                .synthesize(
                    &segments,
                    variant,
                    &work_dir,
                    self.alignment,
                    &context.cancel,
                )
                .await
                .map_err(map_error)?;
            let relative = format!("targets/{}/dub.wav", variant_id.0);
            let target = context.task_root.join(&relative);
            commit_file(&synthesized.dub_audio, &target).map_err(ExecuteError::Failed)?;
            Ok(ExecutionOutcome::Done(vec![output(
                variant_id,
                &relative,
                &target,
                self.engine.version(),
            )?]))
        })
    }
}

fn map_error(error: TtsError) -> ExecuteError {
    match error {
        TtsError::Canceled => ExecuteError::Canceled,
        other => ExecuteError::Failed(format!("TTS 失败: {other:?}")),
    }
}

/// 从任务 shared 产物提取参考音色并注入引擎（voice_ref）。
/// 这是轻量 I/O + ffmpeg 截取，失败仅告警并沿用全局参考，不阻塞任务。
fn apply_task_reference(engine: &Arc<impl TtsEngine>, task_root: &Path) {
    let segments_path = task_root.join("shared/segments.json");
    let segments: Vec<Segment> = match std::fs::read(&segments_path)
        .map_err(|error| error.to_string())
        .and_then(|bytes| serde_json::from_slice::<Vec<Segment>>(&bytes).map_err(|error| error.to_string()))
    {
        Ok(value) if !value.is_empty() => value,
        Ok(_) => {
            log::warn!("[tts] 无 STT 源文段，跳过参考音色提取");
            return;
        }
        Err(error) => {
            log::warn!("[tts] 读取源文段失败，跳过参考音色提取: {error}");
            return;
        }
    };
    match crate::application::voice_ref::extract_voice_reference(task_root, &segments) {
        Ok((wav, text)) => {
            log::info!("[tts] 已注入任务级参考音色: {}（{}）", wav.display(), text);
            engine.with_task_reference(&wav, &text);
        }
        Err(error) => log::warn!("[tts] 自动提取参考音色失败，回退全局参考: {error}"),
    }
}

fn commit_file(temp: &Path, target: &Path) -> Result<(), String> {
    let parent = target.parent().ok_or("TTS 目标没有父目录")?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    if target.exists() {
        fs::remove_file(target).map_err(|error| error.to_string())?;
    }
    fs::rename(temp, target).map_err(|error| error.to_string())
}

fn output(
    variant: &VariantId,
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
        .ok_or_else(|| ExecuteError::Failed("无法读取配音修改时间".into()))?;
    Ok(ArtifactOutput {
        id: ArtifactId(format!("target:{}:tts:0", variant.0)),
        kind: ArtifactKind::DubAudio,
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
    use crate::pipeline::graph::{NodeScope, StageNode};
    use crate::pipeline::runner::{ArtifactInput, CancelToken, ExecutionContext, StageRequest};
    use crate::ports::tts::{TtsFuture, TtsOutput};

    struct FakeTts {
        /// with_task_reference 注入的 (wav, text)
        references: std::sync::Mutex<Option<(String, String)>>,
    }

    impl FakeTts {
        fn new() -> Self {
            Self {
                references: std::sync::Mutex::new(None),
            }
        }
    }

    impl TtsEngine for FakeTts {
        fn version(&self) -> String {
            "fake-v1".into()
        }

        fn with_task_reference(&self, wav: &Path, text: &str) {
            *self.references.lock().unwrap() =
                Some((wav.to_string_lossy().into_owned(), text.into()));
        }

        fn synthesize<'a>(
            &'a self,
            _segments: &'a [Segment],
            _target: &'a TargetVariant,
            output_dir: &'a Path,
            _alignment: TtsAlignment,
            _cancel: &'a CancelToken,
        ) -> TtsFuture<'a> {
            Box::pin(async move {
                fs::create_dir_all(output_dir).unwrap();
                let path = output_dir.join("dub.wav");
                fs::write(&path, b"RIFFfake-wav-audio").unwrap();
                Ok(TtsOutput {
                    dub_audio: path,
                    segment_dir: None,
                })
            })
        }
    }

    #[tokio::test]
    async fn writes_dub_into_variant_directory() {
        let root = std::env::temp_dir().join(format!("tts-stage-{}", uuid::Uuid::new_v4()));
        let translated = root.join("targets/zh-CN/translated.json");
        fs::create_dir_all(translated.parent().unwrap()).unwrap();
        fs::write(
            &translated,
            serde_json::to_vec(&vec![Segment {
                idx: 0,
                start: 0.0,
                end: 1.0,
                text: "hello".into(),
                translated: "你好".into(),
            }])
            .unwrap(),
        )
        .unwrap();
        let variant = TargetVariant::zh_mandarin();
        let executor = TtsStageExecutor::new(
            Arc::new(FakeTts::new()),
            [variant.clone()],
            TtsAlignment {
                min_speed_percent: 85,
                max_speed_percent: 125,
            },
            false,
        );
        let result = executor
            .execute(
                StageRequest {
                    node: StageNode::new(
                        "tts",
                        NodeScope::Target,
                        &["translate"],
                        vec![ArtifactKind::DubAudio],
                    ),
                    scope: RunScope::Target(variant.id.clone()),
                    inputs: vec![ArtifactInput {
                        id: ArtifactId("translated".into()),
                        kind: ArtifactKind::TranslatedSegments,
                        path: translated,
                        content_hash: Some("h".into()),
                    }],
                },
                ExecutionContext {
                    task_root: root.clone(),
                    cancel: CancelToken::default(),
                },
            )
            .await
            .unwrap();
        assert!(matches!(result, ExecutionOutcome::Done(_)));
        assert!(root.join("targets/zh-CN/dub.wav").is_file());
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn injects_task_reference_when_video_prompt_enabled() {
        let root = std::env::temp_dir().join(format!("tts-ref-{}", uuid::Uuid::new_v4()));
        // 预置 STT 源文段与已提取的参考音频（voice_ref 幂等快路径，无需 ffmpeg）
        fs::create_dir_all(root.join("shared")).unwrap();
        fs::write(
            root.join("shared/segments.json"),
            serde_json::to_vec(&vec![Segment {
                idx: 0,
                start: 0.0,
                end: 6.0,
                text: "原视频里的一句人声".into(),
                translated: String::new(),
            }])
            .unwrap(),
        )
        .unwrap();
        fs::write(root.join("shared/ref_voice.wav"), b"RIFF-existing-wav").unwrap();
        fs::write(root.join("shared/ref_voice.txt"), "原视频里的一句人声").unwrap();
        let translated = root.join("targets/zh-CN/translated.json");
        fs::create_dir_all(translated.parent().unwrap()).unwrap();
        fs::write(
            &translated,
            serde_json::to_vec(&vec![Segment {
                idx: 0,
                start: 0.0,
                end: 1.0,
                text: "hello".into(),
                translated: "你好".into(),
            }])
            .unwrap(),
        )
        .unwrap();
        let variant = TargetVariant::zh_mandarin();
        let engine = Arc::new(FakeTts::new());
        let executor = TtsStageExecutor::new(
            engine.clone(),
            [variant.clone()],
            TtsAlignment {
                min_speed_percent: 85,
                max_speed_percent: 125,
            },
            true,
        );
        executor
            .execute(
                StageRequest {
                    node: StageNode::new(
                        "tts",
                        NodeScope::Target,
                        &["translate"],
                        vec![ArtifactKind::DubAudio],
                    ),
                    scope: RunScope::Target(variant.id.clone()),
                    inputs: vec![ArtifactInput {
                        id: ArtifactId("translated".into()),
                        kind: ArtifactKind::TranslatedSegments,
                        path: translated,
                        content_hash: Some("h".into()),
                    }],
                },
                ExecutionContext {
                    task_root: root.clone(),
                    cancel: CancelToken::default(),
                },
            )
            .await
            .unwrap();
        let injected = engine.references.lock().unwrap().clone();
        let (wav, text) = injected.expect("应注入任务级参考音色");
        assert_eq!(Path::new(&wav), root.join("shared/ref_voice.wav").as_path());
        assert_eq!(text, "原视频里的一句人声");
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn skips_reference_when_video_prompt_disabled() {
        let root = std::env::temp_dir().join(format!("tts-noref-{}", uuid::Uuid::new_v4()));
        let translated = root.join("targets/zh-CN/translated.json");
        fs::create_dir_all(translated.parent().unwrap()).unwrap();
        fs::write(
            &translated,
            serde_json::to_vec(&vec![Segment {
                idx: 0,
                start: 0.0,
                end: 1.0,
                text: "hello".into(),
                translated: "你好".into(),
            }])
            .unwrap(),
        )
        .unwrap();
        let variant = TargetVariant::zh_mandarin();
        let engine = Arc::new(FakeTts::new());
        let executor = TtsStageExecutor::new(
            engine.clone(),
            [variant.clone()],
            TtsAlignment {
                min_speed_percent: 85,
                max_speed_percent: 125,
            },
            false,
        );
        executor
            .execute(
                StageRequest {
                    node: StageNode::new(
                        "tts",
                        NodeScope::Target,
                        &["translate"],
                        vec![ArtifactKind::DubAudio],
                    ),
                    scope: RunScope::Target(variant.id.clone()),
                    inputs: vec![ArtifactInput {
                        id: ArtifactId("translated".into()),
                        kind: ArtifactKind::TranslatedSegments,
                        path: translated,
                        content_hash: Some("h".into()),
                    }],
                },
                ExecutionContext {
                    task_root: root.clone(),
                    cancel: CancelToken::default(),
                },
            )
            .await
            .unwrap();
        assert!(engine.references.lock().unwrap().is_none());
        fs::remove_dir_all(root).unwrap();
    }
}
