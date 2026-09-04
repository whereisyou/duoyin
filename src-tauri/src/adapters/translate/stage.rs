use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use crate::domain::artifact::{ArtifactKind, RetentionPolicy};
use crate::domain::ids::{ArtifactId, VariantId};
use crate::domain::manifest::FallbackRecord;
use crate::domain::variant::TargetVariant;
use crate::pipeline::runner::{
    ArtifactOutput, ExecuteError, ExecuteFuture, ExecutionContext, ExecutionOutcome, RunScope,
    StageExecutor, StageRequest,
};
use crate::ports::translator::{TranslateError, Translator};
use crate::types::Segment;

pub struct TranslateStageExecutor<T: Translator> {
    translator: Arc<T>,
    source_language: Option<String>,
    variants: BTreeMap<VariantId, TargetVariant>,
}

impl<T: Translator> TranslateStageExecutor<T> {
    pub fn new(
        translator: Arc<T>,
        source_language: Option<String>,
        variants: impl IntoIterator<Item = TargetVariant>,
    ) -> Self {
        Self {
            translator,
            source_language,
            variants: variants
                .into_iter()
                .map(|variant| (variant.id.clone(), variant))
                .collect(),
        }
    }
}

impl<T: Translator + 'static> StageExecutor for TranslateStageExecutor<T> {
    fn version(&self, _stage: &crate::domain::ids::StageId) -> String {
        self.translator.version()
    }

    fn execute<'a>(
        &'a self,
        request: StageRequest,
        context: ExecutionContext,
    ) -> ExecuteFuture<'a> {
        Box::pin(async move {
            if request.node.id.0 != "translate" {
                return Err(ExecuteError::Failed("翻译执行器收到错误节点".into()));
            }
            let RunScope::Target(variant_id) = &request.scope else {
                return Err(ExecuteError::Failed("翻译节点必须属于目标版本".into()));
            };
            let variant = self
                .variants
                .get(variant_id)
                .ok_or_else(|| ExecuteError::Failed(format!("未知目标版本 {}", variant_id.0)))?;
            let source = request
                .input(ArtifactKind::Segments)
                .ok_or_else(|| ExecuteError::Failed("翻译缺少 STT segments".into()))?;
            let segments: Vec<Segment> = serde_json::from_slice(
                &fs::read(&source.path).map_err(|error| ExecuteError::Failed(error.to_string()))?,
            )
            .map_err(|error| ExecuteError::Failed(error.to_string()))?;
            // 翻译容错：外部 API 偶发截断/超时/漏段，最多重试 3 次（间隔逐步退避）。
            // 全部失败且为 IncompleteResult 时以原文回填译文降级继续（TTS 用原文读，
            // manifest 记录 degraded，任务可重试），避免整任务中断。
            const TRANSLATE_ATTEMPTS: usize = 3;
            let mut translated: Option<Vec<Segment>> = None;
            let mut last_error: Option<TranslateError> = None;
            for attempt in 0..TRANSLATE_ATTEMPTS {
                if context.cancel.is_canceled() {
                    return Err(ExecuteError::Canceled);
                }
                match self
                    .translator
                    .translate(
                        &segments,
                        self.source_language.as_deref(),
                        variant,
                        &context.cancel,
                    )
                    .await
                {
                    Ok(result) => {
                        translated = Some(result);
                        break;
                    }
                    Err(TranslateError::Canceled) => return Err(ExecuteError::Canceled),
                    Err(error) => {
                        log::warn!(
                            "[api:translate] 第 {}/{} 次尝试失败: {:?}",
                            attempt + 1,
                            TRANSLATE_ATTEMPTS,
                            error
                        );
                        last_error = Some(error);
                        if attempt + 1 < TRANSLATE_ATTEMPTS {
                            tokio::time::sleep(std::time::Duration::from_millis(
                                800 * (attempt as u64 + 1),
                            ))
                            .await;
                        }
                    }
                }
            }
            let (translated, degraded): (Vec<Segment>, Option<String>) = match translated {
                Some(result) => (result, None),
                None => match last_error {
                    Some(TranslateError::IncompleteResult) => {
                        let mut fallback = segments.clone();
                        for seg in &mut fallback {
                            seg.translated = seg.text.clone();
                        }
                        log::error!(
                            "[api:translate] 目的语 {}：多次重试译文不完整，已回填原文降级（{} 段）",
                            variant_id.0,
                            fallback.len()
                        );
                        (fallback, Some("译文不完整，已用原文顶替（可重试）".into()))
                    }
                    Some(other) => return Err(map_error(other)),
                    None => return Err(ExecuteError::Failed("翻译无结果".into())),
                },
            };
            let relative = format!("targets/{}/translated.json", variant_id.0);
            let target = context.task_root.join(&relative);
            write_json_atomic(&target, &translated).map_err(ExecuteError::Failed)?;
            let outputs = vec![output(
                variant_id,
                &relative,
                &target,
                self.translator.version(),
                translated.len(),
            )?];
            Ok(match degraded {
                Some(trigger) => ExecutionOutcome::Degraded {
                    outputs,
                    fallback: FallbackRecord {
                        trigger_error: trigger,
                        from: "translate".into(),
                        to: "source_text".into(),
                        degraded_quality: true,
                    },
                },
                None => ExecutionOutcome::Done(outputs),
            })
        })
    }
}

fn map_error(error: TranslateError) -> ExecuteError {
    match error {
        TranslateError::Canceled => ExecuteError::Canceled,
        other => ExecuteError::Failed(format!("翻译失败: {other:?}")),
    }
}

fn write_json_atomic<T: serde::Serialize>(target: &Path, value: &T) -> Result<(), String> {
    let parent = target.parent().ok_or("翻译输出没有父目录")?;
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

fn output(
    variant: &VariantId,
    relative: &str,
    target: &Path,
    engine_version: String,
    segments: usize,
) -> Result<ArtifactOutput, ExecuteError> {
    let metadata = fs::metadata(target).map_err(|error| ExecuteError::Failed(error.to_string()))?;
    let modified = metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .and_then(|value| i64::try_from(value.as_millis()).ok())
        .ok_or_else(|| ExecuteError::Failed("无法读取译文修改时间".into()))?;
    Ok(ArtifactOutput {
        id: ArtifactId(format!("target:{}:translate:0", variant.0)),
        kind: ArtifactKind::TranslatedSegments,
        relative_path: relative.into(),
        size: metadata.len(),
        modified,
        content_hash: format!("{engine_version}:{segments}:{modified}"),
        media_type: Some("application/json".into()),
        retention: RetentionPolicy::RequiredForResume,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::graph::{NodeScope, StageNode};
    use crate::pipeline::runner::{CancelToken, ExecutionContext, StageRequest};
    use crate::ports::translator::{TranslateFuture, Translator};

    struct FakeTranslator;

    impl Translator for FakeTranslator {
        fn version(&self) -> String {
            "fake-v1".into()
        }

        fn translate<'a>(
            &'a self,
            segments: &'a [Segment],
            _source_language: Option<&'a str>,
            target: &'a TargetVariant,
            _cancel: &'a CancelToken,
        ) -> TranslateFuture<'a> {
            Box::pin(async move {
                let mut output = segments.to_vec();
                for segment in &mut output {
                    segment.translated = format!("{}:{}", target.id.0, segment.text);
                }
                Ok(output)
            })
        }
    }

    /// 前 fail_times 次返回 IncompleteResult，之后成功（模拟外部 API 偶发截断）
    struct FlakyTranslator {
        fail_times: usize,
        calls: std::sync::atomic::AtomicUsize,
    }

    impl Translator for FlakyTranslator {
        fn version(&self) -> String {
            "flaky-v1".into()
        }

        fn translate<'a>(
            &'a self,
            segments: &'a [Segment],
            _source_language: Option<&'a str>,
            target: &'a TargetVariant,
            _cancel: &'a CancelToken,
        ) -> TranslateFuture<'a> {
            let fail = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                < self.fail_times;
            Box::pin(async move {
                if fail {
                    return Err(TranslateError::IncompleteResult);
                }
                let mut output = segments.to_vec();
                for segment in &mut output {
                    segment.translated = format!("{}:{}", target.id.0, segment.text);
                }
                Ok(output)
            })
        }
    }

    fn sample_segments() -> Vec<Segment> {
        vec![
            Segment {
                idx: 0,
                start: 0.0,
                end: 1.0,
                text: "你好".into(),
                translated: String::new(),
            },
            Segment {
                idx: 1,
                start: 1.0,
                end: 2.0,
                text: "世界".into(),
                translated: String::new(),
            },
        ]
    }

    async fn run_executor<T: Translator + 'static>(
        executor: &TranslateStageExecutor<T>,
        root: &Path,
        segments: &[Segment],
        variant: &TargetVariant,
    ) -> Result<ExecutionOutcome, ExecuteError> {
        let source = root.join("shared/segments.json");
        fs::create_dir_all(root.join("shared")).unwrap();
        fs::write(&source, serde_json::to_vec(segments).unwrap()).unwrap();
        executor
            .execute(
                StageRequest {
                    node: StageNode::new(
                        "translate",
                        NodeScope::Target,
                        &["stt"],
                        vec![ArtifactKind::TranslatedSegments],
                    ),
                    scope: RunScope::Target(variant.id.clone()),
                    inputs: vec![crate::pipeline::runner::ArtifactInput {
                        id: ArtifactId("segments".into()),
                        kind: ArtifactKind::Segments,
                        path: source,
                        content_hash: Some("h".into()),
                    }],
                },
                ExecutionContext {
                    task_root: root.to_path_buf(),
                    cancel: CancelToken::default(),
                },
            )
            .await
    }

    #[tokio::test]
    async fn writes_translation_into_variant_directory() {
        let root = std::env::temp_dir().join(format!("translate-stage-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join("shared")).unwrap();
        let source = root.join("shared/segments.json");
        fs::write(
            &source,
            serde_json::to_vec(&vec![Segment {
                idx: 0,
                start: 0.0,
                end: 1.0,
                text: "hello".into(),
                translated: String::new(),
            }])
            .unwrap(),
        )
        .unwrap();
        let variant = TargetVariant::zh_dialect("yue", "粤语", "广东话");
        let executor =
            TranslateStageExecutor::new(Arc::new(FakeTranslator), None, [variant.clone()]);
        let result = executor
            .execute(
                StageRequest {
                    node: StageNode::new(
                        "translate",
                        NodeScope::Target,
                        &["stt"],
                        vec![ArtifactKind::TranslatedSegments],
                    ),
                    scope: RunScope::Target(variant.id.clone()),
                    inputs: vec![crate::pipeline::runner::ArtifactInput {
                        id: ArtifactId("segments".into()),
                        kind: ArtifactKind::Segments,
                        path: source,
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
        let output: Vec<Segment> =
            serde_json::from_slice(&fs::read(root.join("targets/zh-yue/translated.json")).unwrap())
                .unwrap();
        assert_eq!(output[0].translated, "zh-yue:hello");
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn retries_and_succeeds_after_temporary_incomplete_result() {
        let root = std::env::temp_dir().join(format!("translate-retry-{}", uuid::Uuid::new_v4()));
        let variant = TargetVariant::zh_mandarin();
        // 第 1 次失败，第 2 次成功 → 任务正常完成，不降级
        let translator = Arc::new(FlakyTranslator {
            fail_times: 1,
            calls: std::sync::atomic::AtomicUsize::new(0),
        });
        let executor = TranslateStageExecutor::new(translator.clone(), None, [variant.clone()]);
        let result = run_executor(&executor, &root, &sample_segments(), &variant)
            .await
            .unwrap();
        assert!(matches!(result, ExecutionOutcome::Done(_)));
        assert_eq!(translator.calls.load(std::sync::atomic::Ordering::SeqCst), 2);
        let output: Vec<Segment> =
            serde_json::from_slice(&fs::read(root.join("targets/zh-CN/translated.json")).unwrap())
                .unwrap();
        assert_eq!(output[0].translated, "zh-CN:你好");
        assert_eq!(output[1].translated, "zh-CN:世界");
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn degrades_with_source_text_when_retries_exhausted() {
        let root = std::env::temp_dir().join(format!("translate-degraded-{}", uuid::Uuid::new_v4()));
        let variant = TargetVariant::zh_mandarin();
        // 3 次全失败 → 原文回填译文，降级继续
        let translator = Arc::new(FlakyTranslator {
            fail_times: 10,
            calls: std::sync::atomic::AtomicUsize::new(0),
        });
        let executor = TranslateStageExecutor::new(translator.clone(), None, [variant.clone()]);
        let result = run_executor(&executor, &root, &sample_segments(), &variant)
            .await
            .unwrap();
        let ExecutionOutcome::Degraded { fallback, .. } = result else {
            panic!("期望 Degraded，实际 {:?}", result);
        };
        assert!(fallback.degraded_quality);
        assert_eq!(translator.calls.load(std::sync::atomic::Ordering::SeqCst), 3);
        let output: Vec<Segment> =
            serde_json::from_slice(&fs::read(root.join("targets/zh-CN/translated.json")).unwrap())
                .unwrap();
        // 回填原文：translated == text
        assert_eq!(output[0].translated, "你好");
        assert_eq!(output[1].translated, "世界");
        fs::remove_dir_all(root).unwrap();
    }
}
