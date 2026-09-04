use std::path::Path;
use tokio::process::Command;

/// 从视频提取 16kHz WAV 音频
pub async fn extract_audio(video: &Path, out: &Path) -> Result<(), String> {
    let s = Command::new("ffmpeg")
        .kill_on_drop(true)
        .arg("-i")
        .arg(video)
        .arg("-vn")
        .arg("-acodec")
        .arg("pcm_s16le")
        .arg("-ar")
        .arg("16000")
        .arg("-ac")
        .arg("1")
        .arg("-y")
        .arg(out)
        .status()
        .await
        .map_err(|e| format!("ffmpeg not found: {}", e))?;
    if !s.success() {
        return Err("ffmpeg extract_audio failed".into());
    }
    Ok(())
}

/// 复制源视频流并用新配音替换音轨，输出新视频，不覆盖源文件。
/// 对短配音使用 apad 补静音，避免 `-shortest` 截短原视频。
pub async fn mux_replaced_audio(video: &Path, audio: &Path, out: &Path) -> Result<(), String> {
    if !video.is_file() {
        return Err(format!("源视频不存在：{}", video.display()));
    }
    if !audio.is_file() {
        return Err(format!("配音文件不存在：{}", audio.display()));
    }
    if let Some(parent) = out.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("创建视频输出目录失败: {e}"))?;
    }
    let status = Command::new("ffmpeg")
        .kill_on_drop(true)
        .arg("-v")
        .arg("error")
        .arg("-i")
        .arg(video)
        .arg("-i")
        .arg(audio)
        .arg("-filter_complex")
        .arg("[1:a]apad[new_audio]")
        .arg("-map")
        .arg("0:v:0")
        .arg("-map")
        .arg("[new_audio]")
        .arg("-c:v")
        .arg("copy")
        .arg("-c:a")
        .arg("aac")
        .arg("-shortest")
        .arg("-y")
        .arg(out)
        .status()
        .await
        .map_err(|e| format!("ffmpeg not found: {e}"))?;
    if !status.success() {
        let _ = tokio::fs::remove_file(out).await;
        return Err("ffmpeg final video mux failed".into());
    }
    Ok(())
}

/// 按字幕段时间戳切割音频片段
pub async fn split_audio(
    audio: &Path,
    segments: &[crate::types::Segment],
    dir: &Path,
) -> Result<Vec<crate::types::Segment>, String> {
    tokio::fs::create_dir_all(dir)
        .await
        .map_err(|e| e.to_string())?;

    let mut files = Vec::new();
    for seg in segments {
        let Some((ss, to)) = split_bounds(seg) else {
            log::warn!(
                "skip invalid segment {}: start={:.3}, end={:.3}",
                seg.idx,
                seg.start,
                seg.end
            );
            continue;
        };
        let out = dir.join(format!("{:04}.mp3", seg.idx));
        let s = Command::new("ffmpeg")
            .kill_on_drop(true)
            .arg("-i")
            .arg(audio)
            .arg("-ss")
            .arg(&ss)
            .arg("-to")
            .arg(&to)
            .arg("-y")
            .arg(&out)
            .status()
            .await
            .map_err(|e| format!("ffmpeg error: {}", e))?;
        if !s.success() {
            return Err(format!("split segment {} failed", seg.idx));
        }
        files.push(seg.clone());
    }
    Ok(files)
}

fn split_bounds(seg: &crate::types::Segment) -> Option<(String, String)> {
    // ffmpeg 在 -to <= -ss 时会直接 abort；模型时间戳偶尔会产出 0 长度/倒置段，
    // 这类段应该跳过，不能中断整个视频流程。
    if !seg.start.is_finite() || !seg.end.is_finite() {
        return None;
    }
    if seg.start < 0.0 || seg.end <= seg.start + 0.02 {
        return None;
    }
    Some((fmt_sec(seg.start), fmt_sec(seg.end)))
}

fn fmt_sec(s: f64) -> String {
    format!("{:.3}", s.max(0.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fmt_sec() {
        assert_eq!(fmt_sec(0.0), "0.000");
        assert_eq!(fmt_sec(-1.0), "0.000");
        assert_eq!(fmt_sec(1.5), "1.500");
        assert_eq!(fmt_sec(123.456), "123.456");
    }

    #[test]
    fn test_split_bounds_rejects_invalid_timestamps() {
        let mut seg = crate::types::Segment {
            idx: 0,
            start: 1.0,
            end: 2.0,
            text: "x".into(),
            translated: String::new(),
        };
        assert_eq!(split_bounds(&seg), Some(("1.000".into(), "2.000".into())));

        seg.start = 2.0;
        seg.end = 1.0;
        assert!(split_bounds(&seg).is_none(), "倒置时间戳必须跳过");

        seg.start = 1.0;
        seg.end = 1.0;
        assert!(split_bounds(&seg).is_none(), "零长度段必须跳过");

        seg.start = f64::NAN;
        seg.end = 2.0;
        assert!(split_bounds(&seg).is_none(), "NaN 时间戳必须跳过");
    }

    #[tokio::test]
    async fn test_mux_replaced_audio_creates_full_length_video() {
        let dir = std::env::temp_dir().join(format!("videotrans_mux_{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let video = dir.join("source.mp4");
        let audio = dir.join("dub.wav");
        let output = dir.join("final.mp4");
        let video_status = Command::new("ffmpeg")
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
            .arg(&video)
            .status()
            .await
            .unwrap();
        assert!(video_status.success());
        let audio_status = Command::new("ffmpeg")
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
            .arg(&audio)
            .status()
            .await
            .unwrap();
        assert!(audio_status.success());

        mux_replaced_audio(&video, &audio, &output).await.unwrap();

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
                .arg(&output)
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
        assert!(duration >= 0.5);
        assert_eq!(audio_tracks, 1);
        tokio::fs::remove_dir_all(dir).await.unwrap();
    }

    #[tokio::test]
    async fn test_extract_audio_file_not_found() {
        let result = extract_audio(
            Path::new("/nonexistent/file.mp4"),
            Path::new("/tmp/out.wav"),
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_split_audio_skips_invalid_segments_without_touching_ffmpeg() {
        let dir = std::env::temp_dir().join("videotrans_test_split_invalid");
        let segments = vec![
            crate::types::Segment {
                idx: 14,
                start: 9.5,
                end: 9.4,
                text: "bad".into(),
                translated: String::new(),
            },
            crate::types::Segment {
                idx: 15,
                start: 1.0,
                end: 1.0,
                text: "zero".into(),
                translated: String::new(),
            },
        ];
        let result = split_audio(Path::new("nonexistent.wav"), &segments, &dir).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().len(), 0);
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn test_split_audio_empty_segments() {
        let dir = std::env::temp_dir().join("videotrans_test_split_empty");
        let result = split_audio(Path::new("dummy.wav"), &[], &dir).await;
        // 空 segments 应当成功（没有要切的文件）
        assert!(result.is_ok());
        assert_eq!(result.unwrap().len(), 0);
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn test_ffmpeg_available() {
        let s = Command::new("ffmpeg").arg("-version").output().await;
        assert!(
            s.is_ok(),
            "ffmpeg should be installed and available in PATH"
        );
        let output = s.unwrap();
        assert!(output.status.success());
        let version = String::from_utf8_lossy(&output.stdout);
        assert!(
            version.contains("ffmpeg"),
            "ffmpeg version output should contain 'ffmpeg'"
        );
    }
}
