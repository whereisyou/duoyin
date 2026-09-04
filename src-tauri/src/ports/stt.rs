use std::future::Future;
use std::path::Path;
use std::pin::Pin;

use crate::pipeline::runner::CancelToken;
use crate::types::Segment;

pub type SttFuture<'a> = Pin<Box<dyn Future<Output = Result<Vec<Segment>, SttError>> + Send + 'a>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SttError {
    Canceled,
    /// 输入校验错误（引擎 preflight 用，sensevoice/whisper 当前直接报 String 透传，保留）
    #[allow(dead_code)]
    InvalidInput(String),
    Engine(String),
    EmptyResult,
}

pub trait SttEngine: Send + Sync {
    fn version(&self) -> String;

    fn resource_cost(&self) -> crate::scheduler::ResourceCost {
        crate::scheduler::ResourceCost::default()
    }

    fn transcribe<'a>(
        &'a self,
        audio: &'a Path,
        source_language: Option<&'a str>,
        cancel: &'a CancelToken,
    ) -> SttFuture<'a>;
}

pub fn sanitize_segments(segments: Vec<Segment>) -> Result<Vec<Segment>, SttError> {
    let sanitized = crate::segments::sanitize(segments);
    validate_segments(&sanitized)?;
    Ok(sanitized)
}

pub fn validate_segments(segments: &[Segment]) -> Result<(), SttError> {
    if segments.is_empty() {
        return Err(SttError::EmptyResult);
    }
    if segments.iter().any(|segment| {
        !segment.start.is_finite()
            || !segment.end.is_finite()
            || segment.start < 0.0
            || segment.end <= segment.start
            || segment.text.trim().is_empty()
    }) {
        return Err(SttError::Engine("STT 返回了无效字幕段".into()));
    }
    Ok(())
}

#[cfg(test)]
pub(crate) async fn assert_stt_contract(engine: &dyn SttEngine, audio: &Path) {
    let segments = engine
        .transcribe(audio, None, &CancelToken::default())
        .await
        .expect("STT should succeed");
    validate_segments(&segments).expect("STT output should satisfy contract");
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeStt;

    impl SttEngine for FakeStt {
        fn version(&self) -> String {
            "fake-v1".into()
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
                    end: 1.0,
                    text: "hello".into(),
                    translated: String::new(),
                }])
            })
        }
    }

    #[tokio::test]
    async fn fake_satisfies_stt_contract() {
        assert_stt_contract(&FakeStt, Path::new("fake.wav")).await;
    }

    #[test]
    fn sanitizes_bad_segments_without_discarding_valid_transcript() {
        let sanitized = sanitize_segments(vec![
            Segment {
                idx: 8,
                start: 1.0,
                end: 1.0,
                text: "。".into(),
                translated: String::new(),
            },
            Segment {
                idx: 9,
                start: 2.0,
                end: 3.0,
                text: "  valid text  ".into(),
                translated: String::new(),
            },
        ])
        .unwrap();
        assert_eq!(sanitized.len(), 1);
        assert_eq!(sanitized[0].idx, 0);
        assert_eq!(sanitized[0].text, "valid text");
    }

    #[test]
    fn all_invalid_segments_become_empty_result() {
        assert!(matches!(
            sanitize_segments(vec![Segment {
                idx: 0,
                start: 1.0,
                end: 1.0,
                text: "。".into(),
                translated: String::new(),
            }]),
            Err(SttError::EmptyResult)
        ));
    }

    #[test]
    fn rejects_empty_and_invalid_segments() {
        assert_eq!(validate_segments(&[]), Err(SttError::EmptyResult));
        assert!(validate_segments(&[Segment {
            idx: 0,
            start: 1.0,
            end: 1.0,
            text: "x".into(),
            translated: String::new(),
        }])
        .is_err());
    }
}
