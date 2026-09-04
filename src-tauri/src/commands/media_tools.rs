use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde::Serialize;
use tokio::process::Command;

#[derive(Debug, Clone, Serialize)]
pub struct DemuxOutput {
    pub video: String,
    pub audio: String,
}

#[tauri::command]
pub async fn match_text_to_srt(
    srt_path: String,
    text_path: String,
    output: String,
) -> Result<(), String> {
    let srt = std::fs::read_to_string(&srt_path).map_err(|error| error.to_string())?;
    let text = std::fs::read_to_string(&text_path).map_err(|error| error.to_string())?;
    let segments = crate::subtitle_parse::parse_srt(&srt)?;
    let aligned = crate::text_align::align_text_to_segments(&segments, &text)?;
    crate::subtitle::write_srt(&aligned, Path::new(&output)).await
}

#[tauri::command]
pub async fn clip_video(
    input: String,
    output: String,
    start_seconds: f64,
    end_seconds: f64,
) -> Result<(), String> {
    if !start_seconds.is_finite()
        || !end_seconds.is_finite()
        || start_seconds < 0.0
        || end_seconds <= start_seconds
    {
        return Err("裁剪时间范围无效".into());
    }
    run_ffmpeg(
        Path::new(&output),
        [
            "-v".into(),
            "error".into(),
            "-ss".into(),
            format!("{start_seconds:.3}"),
            "-to".into(),
            format!("{end_seconds:.3}"),
            "-i".into(),
            input,
            "-c".into(),
            "copy".into(),
            "-y".into(),
            output.clone(),
        ],
    )
    .await
}

#[tauri::command]
pub async fn separate_media(input: String, output_dir: String) -> Result<DemuxOutput, String> {
    let input_path = required_file(&input)?;
    let output_dir = PathBuf::from(output_dir);
    tokio::fs::create_dir_all(&output_dir)
        .await
        .map_err(|error| error.to_string())?;
    let stem = input_path.file_stem().unwrap_or_default().to_string_lossy();
    let video = output_dir.join(format!("{stem}.video.mp4"));
    let audio = output_dir.join(format!("{stem}.audio.wav"));
    run_ffmpeg(
        &video,
        [
            "-v".into(),
            "error".into(),
            "-i".into(),
            input.clone(),
            "-an".into(),
            "-c:v".into(),
            "copy".into(),
            "-y".into(),
            video.to_string_lossy().into_owned(),
        ],
    )
    .await?;
    if let Err(error) = run_ffmpeg(
        &audio,
        [
            "-v".into(),
            "error".into(),
            "-i".into(),
            input,
            "-vn".into(),
            "-c:a".into(),
            "pcm_s16le".into(),
            "-y".into(),
            audio.to_string_lossy().into_owned(),
        ],
    )
    .await
    {
        let _ = tokio::fs::remove_file(&video).await;
        return Err(error);
    }
    Ok(DemuxOutput {
        video: video.to_string_lossy().into_owned(),
        audio: audio.to_string_lossy().into_owned(),
    })
}

#[tauri::command]
pub async fn merge_video_audio(video: String, audio: String, output: String) -> Result<(), String> {
    required_file(&video)?;
    required_file(&audio)?;
    run_ffmpeg(
        Path::new(&output),
        [
            "-v".into(),
            "error".into(),
            "-i".into(),
            video,
            "-i".into(),
            audio,
            "-map".into(),
            "0:v:0".into(),
            "-map".into(),
            "1:a:0".into(),
            "-c:v".into(),
            "copy".into(),
            "-c:a".into(),
            "aac".into(),
            "-shortest".into(),
            "-y".into(),
            output.clone(),
        ],
    )
    .await
}

fn required_file(value: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(value);
    if !path.is_file() {
        Err(format!("输入文件不存在: {}", path.display()))
    } else {
        Ok(path)
    }
}

async fn run_ffmpeg<const N: usize>(output: &Path, args: [String; N]) -> Result<(), String> {
    if output.as_os_str().is_empty() {
        return Err("输出路径为空".into());
    }
    if let Some(parent) = output.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|error| error.to_string())?;
    }
    let status = Command::new("ffmpeg")
        .kill_on_drop(true)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .status()
        .await
        .map_err(|error| format!("启动 FFmpeg 失败: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        let _ = tokio::fs::remove_file(output).await;
        Err(format!("FFmpeg 执行失败: {status}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_clip_range_is_rejected() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        assert!(runtime
            .block_on(clip_video("x".into(), "y".into(), 3.0, 2.0))
            .is_err());
    }
}
