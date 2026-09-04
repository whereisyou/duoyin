use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use serde::Deserialize;
use tokio::process::Command;

use crate::domain::media::MediaInfo;
use crate::pipeline::runner::CancelToken;
use crate::ports::media_tool::{validate_media_info, MediaFuture, MediaTool, MediaToolError};

#[derive(Debug, Clone)]
pub struct FfmpegMediaTool {
    ffmpeg: String,
    ffprobe: String,
}

impl Default for FfmpegMediaTool {
    fn default() -> Self {
        Self {
            ffmpeg: "ffmpeg".into(),
            ffprobe: "ffprobe".into(),
        }
    }
}

impl FfmpegMediaTool {
    /// 指定可执行文件路径（测试用；生产走 Default）；确证无旁路则删
    #[allow(dead_code)]
    pub fn new(ffmpeg: impl Into<String>, ffprobe: impl Into<String>) -> Self {
        Self {
            ffmpeg: ffmpeg.into(),
            ffprobe: ffprobe.into(),
        }
    }
}

impl MediaTool for FfmpegMediaTool {
    fn probe<'a>(&'a self, input: &'a Path, cancel: &'a CancelToken) -> MediaFuture<'a, MediaInfo> {
        Box::pin(async move {
            if cancel.is_canceled() {
                return Err(MediaToolError::Canceled);
            }
            let mut command = Command::new(&self.ffprobe);
            command
                .kill_on_drop(true)
                .args([
                    "-v",
                    "error",
                    "-show_entries",
                    "format=duration,size:stream=codec_type,codec_name,width,height,r_frame_rate,sample_rate,channels",
                    "-of",
                    "json",
                ])
                .arg(input)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            let child = command
                .spawn()
                .map_err(|error| MediaToolError::ToolUnavailable(error.to_string()))?;
            let output = wait_output_or_cancel(child, cancel).await?;
            if !output.status.success() {
                return Err(MediaToolError::ProcessFailed(
                    String::from_utf8_lossy(&output.stderr).trim().to_owned(),
                ));
            }
            parse_probe(&output.stdout)
        })
    }

    fn extract_stt_audio<'a>(
        &'a self,
        input: &'a Path,
        output: &'a Path,
        cancel: &'a CancelToken,
    ) -> MediaFuture<'a, ()> {
        Box::pin(async move {
            if cancel.is_canceled() {
                return Err(MediaToolError::Canceled);
            }
            if let Some(parent) = output.parent() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .map_err(|error| MediaToolError::Io(error.to_string()))?;
            }
            let mut command = Command::new(&self.ffmpeg);
            command
                .kill_on_drop(true)
                .arg("-v")
                .arg("error")
                .arg("-i")
                .arg(input)
                .arg("-vn")
                .arg("-acodec")
                .arg("pcm_s16le")
                .arg("-ar")
                .arg("16000")
                .arg("-ac")
                .arg("1")
                .arg("-y")
                .arg(output)
                .stdout(Stdio::null())
                .stderr(Stdio::piped());
            let child = command
                .spawn()
                .map_err(|error| MediaToolError::ToolUnavailable(error.to_string()))?;
            let result = wait_output_status_or_cancel(child, cancel).await;
            if !matches!(result, Ok(())) {
                let _ = tokio::fs::remove_file(output).await;
            }
            result
        })
    }
}

async fn wait_output_status_or_cancel(
    child: tokio::process::Child,
    cancel: &CancelToken,
) -> Result<(), MediaToolError> {
    let output = wait_output_or_cancel(child, cancel).await?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        Err(MediaToolError::ProcessFailed(if stderr.is_empty() {
            format!("ffmpeg 退出码: {}", output.status)
        } else {
            stderr
        }))
    }
}

async fn wait_output_or_cancel(
    mut child: tokio::process::Child,
    cancel: &CancelToken,
) -> Result<std::process::Output, MediaToolError> {
    loop {
        if cancel.is_canceled() {
            child
                .kill()
                .await
                .map_err(|error| MediaToolError::Io(error.to_string()))?;
            return Err(MediaToolError::Canceled);
        }
        match child
            .try_wait()
            .map_err(|error| MediaToolError::Io(error.to_string()))?
        {
            Some(_) => {
                return child
                    .wait_with_output()
                    .await
                    .map_err(|error| MediaToolError::Io(error.to_string()))
            }
            None => tokio::time::sleep(Duration::from_millis(20)).await,
        }
    }
}

#[derive(Debug, Deserialize)]
struct ProbeResponse {
    #[serde(default)]
    streams: Vec<ProbeStream>,
    format: Option<ProbeFormat>,
}

#[derive(Debug, Deserialize)]
struct ProbeStream {
    codec_type: Option<String>,
    codec_name: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    r_frame_rate: Option<String>,
    sample_rate: Option<String>,
    channels: Option<u16>,
}

#[derive(Debug, Deserialize)]
struct ProbeFormat {
    duration: Option<String>,
    size: Option<String>,
}

fn parse_frame_rate_milli(value: &str) -> Option<u32> {
    let (numerator, denominator) = value.split_once('/').unwrap_or((value, "1"));
    let numerator = numerator.parse::<f64>().ok()?;
    let denominator = denominator.parse::<f64>().ok()?;
    if !numerator.is_finite() || !denominator.is_finite() || denominator <= 0.0 {
        return None;
    }
    u32::try_from((numerator * 1000.0 / denominator).round() as u64).ok()
}

fn parse_probe(bytes: &[u8]) -> Result<MediaInfo, MediaToolError> {
    let response: ProbeResponse = serde_json::from_slice(bytes)
        .map_err(|error| MediaToolError::InvalidOutput(error.to_string()))?;
    let video = response
        .streams
        .iter()
        .find(|stream| stream.codec_type.as_deref() == Some("video"))
        .ok_or_else(|| MediaToolError::InvalidOutput("媒体没有视频流".into()))?;
    let audio_tracks: Vec<_> = response
        .streams
        .iter()
        .filter(|stream| stream.codec_type.as_deref() == Some("audio"))
        .collect();
    let audio = audio_tracks.first().copied();
    let duration_ms = response
        .format
        .as_ref()
        .and_then(|format| format.duration.as_ref())
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|value| value.is_finite() && *value > 0.0)
        .map(|value| (value * 1000.0).round() as u64)
        .unwrap_or(0);
    let info = MediaInfo {
        duration_ms,
        width: video.width.unwrap_or(0),
        height: video.height.unwrap_or(0),
        video_codec: video.codec_name.clone().unwrap_or_default(),
        frame_rate_milli: video
            .r_frame_rate
            .as_deref()
            .and_then(parse_frame_rate_milli),
        source_size: response
            .format
            .as_ref()
            .and_then(|format| format.size.as_deref())
            .and_then(|value| value.parse().ok())
            .unwrap_or(0),
        audio_track_count: u16::try_from(audio_tracks.len()).unwrap_or(u16::MAX),
        audio_codec: audio.and_then(|stream| stream.codec_name.clone()),
        audio_sample_rate: audio
            .and_then(|stream| stream.sample_rate.as_deref())
            .and_then(|value| value.parse().ok()),
        audio_channels: audio.and_then(|stream| stream.channels),
    };
    validate_media_info(&info)?;
    Ok(info)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::media_tool::assert_media_tool_contract;

    #[test]
    fn parses_ffprobe_json() {
        let info = parse_probe(
            br#"{
                "streams": [
                    {"codec_type":"video","codec_name":"h264","width":1280,"height":720,"r_frame_rate":"30000/1001"},
                    {"codec_type":"audio","codec_name":"aac","sample_rate":"48000","channels":2},
                    {"codec_type":"audio","codec_name":"aac","sample_rate":"48000","channels":2}
                ],
                "format":{"duration":"1.250000","size":"12345"}
            }"#,
        )
        .unwrap();
        assert_eq!(info.duration_ms, 1250);
        assert_eq!((info.width, info.height), (1280, 720));
        assert_eq!(info.audio_sample_rate, Some(48000));
        assert_eq!(info.frame_rate_milli, Some(29970));
        assert_eq!(info.audio_track_count, 2);
        assert_eq!(info.source_size, 12345);
    }

    #[test]
    fn rejects_media_without_video_stream() {
        assert!(parse_probe(br#"{"streams":[],"format":{"duration":"1"}}"#).is_err());
    }

    #[tokio::test]
    async fn canceled_before_start_does_not_spawn_tool() {
        let tool = FfmpegMediaTool::new("missing-ffmpeg", "missing-ffprobe");
        let cancel = CancelToken::default();
        cancel.cancel();
        assert_eq!(
            tool.probe(Path::new("missing.mp4"), &cancel).await,
            Err(MediaToolError::Canceled)
        );
    }

    #[tokio::test]
    async fn real_ffmpeg_satisfies_media_tool_contract() {
        let root = std::env::temp_dir().join(format!("media-tool-{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&root).await.unwrap();
        let input = root.join("sample.mp4");
        let status = Command::new("ffmpeg")
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
            .arg(&input)
            .status()
            .await
            .unwrap();
        assert!(status.success());

        assert_media_tool_contract(&FfmpegMediaTool::default(), &input).await;
        tokio::fs::remove_dir_all(root).await.unwrap();
    }
}
