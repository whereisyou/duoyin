use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub enum AlignOutcome {
    Unchanged,
    Adjusted { tempo: f64 },
    Limited { required: f64, applied: f64 },
}

pub fn align_wav_to_duration(
    input: &Path,
    output: &Path,
    actual_seconds: f64,
    target_seconds: f64,
    max_speed_percent: u16,
) -> Result<AlignOutcome, String> {
    if !actual_seconds.is_finite()
        || !target_seconds.is_finite()
        || actual_seconds <= 0.0
        || target_seconds <= 0.0
        || actual_seconds <= target_seconds
    {
        return Ok(AlignOutcome::Unchanged);
    }
    let required = actual_seconds / target_seconds;
    let max_tempo = (max_speed_percent as f64 / 100.0).max(1.0);
    let applied = required.min(max_tempo);
    let status = std::process::Command::new("ffmpeg")
        .arg("-v")
        .arg("error")
        .arg("-i")
        .arg(input)
        .arg("-af")
        .arg(format!("rubberband=tempo={applied:.6}"))
        .arg("-y")
        .arg(output)
        .status()
        .map_err(|error| format!("启动 FFmpeg 对齐失败: {error}"))?;
    if !status.success() {
        return Err("FFmpeg rubberband 音频对齐失败".into());
    }
    if required > max_tempo {
        log::warn!(
            "[audio-align] required={required:.3}x exceeds max={max_tempo:.3}x; applying limit"
        );
        Ok(AlignOutcome::Limited { required, applied })
    } else {
        Ok(AlignOutcome::Adjusted { tempo: applied })
    }
}

pub fn aligned_path(input: &Path) -> PathBuf {
    let stem = input.file_stem().unwrap_or_default().to_string_lossy();
    input.with_file_name(format!("{stem}.aligned.wav"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_audio_does_not_need_speed_change() {
        assert_eq!(
            align_wav_to_duration(Path::new("x"), Path::new("y"), 0.5, 1.0, 125).unwrap(),
            AlignOutcome::Unchanged
        );
    }

    #[tokio::test]
    async fn long_audio_is_limited_to_configured_tempo() {
        let root = std::env::temp_dir().join(format!("align-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let input = root.join("input.wav");
        let output = root.join("output.wav");
        let status = tokio::process::Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=1",
                "-acodec",
                "pcm_s16le",
                "-y",
            ])
            .arg(&input)
            .status()
            .await
            .unwrap();
        assert!(status.success());
        let result = align_wav_to_duration(&input, &output, 1.0, 0.5, 125).unwrap();
        assert!(
            matches!(result, AlignOutcome::Limited { applied, .. } if (applied - 1.25).abs() < 0.001)
        );
        assert!(output.is_file());
        std::fs::remove_dir_all(root).unwrap();
    }
}
