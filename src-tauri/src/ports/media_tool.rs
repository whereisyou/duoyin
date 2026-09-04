use std::future::Future;
use std::path::Path;
use std::pin::Pin;

use crate::domain::media::MediaInfo;
use crate::pipeline::runner::CancelToken;

pub type MediaFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, MediaToolError>> + Send + 'a>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaToolError {
    Canceled,
    ToolUnavailable(String),
    ProcessFailed(String),
    InvalidOutput(String),
    Io(String),
}

pub trait MediaTool: Send + Sync {
    fn probe<'a>(&'a self, input: &'a Path, cancel: &'a CancelToken) -> MediaFuture<'a, MediaInfo>;

    fn extract_stt_audio<'a>(
        &'a self,
        input: &'a Path,
        output: &'a Path,
        cancel: &'a CancelToken,
    ) -> MediaFuture<'a, ()>;
}

pub fn validate_media_info(info: &MediaInfo) -> Result<(), MediaToolError> {
    if info.duration_ms == 0 {
        return Err(MediaToolError::InvalidOutput("媒体时长必须大于 0".into()));
    }
    if info.video_codec.trim().is_empty() {
        return Err(MediaToolError::InvalidOutput("视频编码不能为空".into()));
    }
    if info.width == 0 || info.height == 0 {
        return Err(MediaToolError::InvalidOutput("视频尺寸必须大于 0".into()));
    }
    Ok(())
}

#[cfg(test)]
pub(crate) async fn assert_media_tool_contract(tool: &dyn MediaTool, input: &Path) {
    let cancel = CancelToken::default();
    let info = tool
        .probe(input, &cancel)
        .await
        .expect("probe should succeed");
    validate_media_info(&info).expect("probe result should satisfy contract");

    let output = std::env::temp_dir().join(format!("media-contract-{}.wav", uuid::Uuid::new_v4()));
    tool.extract_stt_audio(input, &output, &cancel)
        .await
        .expect("extract should succeed");
    let metadata = std::fs::metadata(&output).expect("output should exist");
    assert!(metadata.is_file());
    assert!(metadata.len() > 44, "WAV should contain audio samples");
    let _ = std::fs::remove_file(output);
}
