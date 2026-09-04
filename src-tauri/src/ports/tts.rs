use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;

use crate::domain::variant::TargetVariant;
use crate::pipeline::runner::CancelToken;
use crate::types::Segment;

pub type TtsFuture<'a> = Pin<Box<dyn Future<Output = Result<TtsOutput, TtsError>> + Send + 'a>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TtsAlignment {
    pub min_speed_percent: u16,
    pub max_speed_percent: u16,
}

#[derive(Debug)]
pub struct TtsOutput {
    pub dub_audio: PathBuf,
    /// 逐段 wav 输出（supertonic 契约用；zipvoice/cosyvoice 只返回 dub_audio，当前未反读此字段）
    #[allow(dead_code)]
    pub segment_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TtsError {
    Canceled,
    InvalidInput(String),
    UnsupportedVariant(String),
    Engine(String),
}

impl std::fmt::Display for TtsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TtsError::Canceled => write!(f, "TTS 已取消"),
            TtsError::InvalidInput(msg) => write!(f, "TTS 输入错误: {msg}"),
            TtsError::UnsupportedVariant(msg) => write!(f, "TTS 不支持: {msg}"),
            TtsError::Engine(msg) => write!(f, "TTS 引擎错误: {msg}"),
        }
    }
}

pub trait TtsEngine: Send + Sync {
    fn version(&self) -> String;

    fn resource_cost(&self) -> crate::scheduler::ResourceCost {
        crate::scheduler::ResourceCost::default()
    }

    fn synthesize<'a>(
        &'a self,
        segments: &'a [Segment],
        target: &'a TargetVariant,
        output_dir: &'a Path,
        alignment: TtsAlignment,
        cancel: &'a CancelToken,
    ) -> TtsFuture<'a>;

    /// 任务级参考音色覆盖（零样本克隆引擎实现；其余引擎默认忽略）。
    /// 调用点在 TTS stage 执行前，由从原视频提取的参考段驱动。
    fn with_task_reference(&self, _wav: &Path, _text: &str) {}
}

pub fn validate_tts_input(segments: &[Segment]) -> Result<(), TtsError> {
    if segments.is_empty() {
        return Err(TtsError::InvalidInput("没有可配音字幕段".into()));
    }
    if segments
        .iter()
        .any(|segment| segment.translated.trim().is_empty())
    {
        return Err(TtsError::InvalidInput("存在空译文，不能进入 TTS".into()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty_translation_before_expensive_tts() {
        assert!(validate_tts_input(&[]).is_err());
        assert!(validate_tts_input(&[Segment {
            idx: 0,
            start: 0.0,
            end: 1.0,
            text: "hello".into(),
            translated: String::new(),
        }])
        .is_err());
    }
}
