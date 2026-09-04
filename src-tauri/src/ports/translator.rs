use std::future::Future;
use std::pin::Pin;

use crate::domain::variant::TargetVariant;
use crate::pipeline::runner::CancelToken;
use crate::types::Segment;

pub type TranslateFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Vec<Segment>, TranslateError>> + Send + 'a>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranslateError {
    Canceled,
    InvalidInput(String),
    Engine(String),
    IncompleteResult,
}

pub trait Translator: Send + Sync {
    fn version(&self) -> String;

    fn translate<'a>(
        &'a self,
        segments: &'a [Segment],
        source_language: Option<&'a str>,
        target: &'a TargetVariant,
        cancel: &'a CancelToken,
    ) -> TranslateFuture<'a>;
}

pub fn validate_translation(
    source: &[Segment],
    translated: &[Segment],
) -> Result<(), TranslateError> {
    if source.len() != translated.len() {
        return Err(TranslateError::IncompleteResult);
    }
    for (expected, actual) in source.iter().zip(translated) {
        if expected.idx != actual.idx
            || expected.start != actual.start
            || expected.end != actual.end
            || actual.translated.trim().is_empty()
        {
            return Err(TranslateError::IncompleteResult);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validation_requires_all_segments_and_preserved_timestamps() {
        let source = vec![Segment {
            idx: 0,
            start: 0.0,
            end: 1.0,
            text: "hello".into(),
            translated: String::new(),
        }];
        let mut translated = source.clone();
        translated[0].translated = "你好".into();
        assert!(validate_translation(&source, &translated).is_ok());
        translated[0].translated.clear();
        assert_eq!(
            validate_translation(&source, &translated),
            Err(TranslateError::IncompleteResult)
        );
    }
}
