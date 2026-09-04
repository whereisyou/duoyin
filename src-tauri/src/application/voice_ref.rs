//! 任务级参考音色：从原视频语音中自动挑段，供零样本克隆引擎（ZipVoice）使用。
//!
//! 流程：读 shared/segments.json 的源文段 → 挑「时长 3~20s 且文本 ≥2 字」中最长的一段 →
//! ffmpeg 从 shared/vocals.wav（分离后更干净）或 shared/audio.wav 截取该区间 →
//! 写 shared/ref_voice.wav + shared/ref_voice.txt（prompt 文本须与音频逐字一致，用 STT 原文）。
//! 提取失败/无合适语音段时，调用方回退全局参考音频（见 tts/stage.rs）。
//!
//! 幂等：shared/ref_voice.wav 已存在（重跑/复用）直接读回，不再重复截取。

use std::path::{Path, PathBuf};

use crate::types::Segment;

/// 参考段选择：时长 3~20s、文本 ≥2 字，取最长（语音信息最足、克隆最稳）。
pub fn pick_reference_segment(segments: &[Segment]) -> Option<&Segment> {
    segments
        .iter()
        .filter(|segment| {
            let duration = segment.end - segment.start;
            duration >= 3.0 && duration <= 20.0 && segment.text.trim().chars().count() >= 2
        })
        .max_by(|a, b| {
            (a.end - a.start)
                .partial_cmp(&(b.end - b.start))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
}

/// 从任务目录提取参考音频。返回 (wav 路径, 逐字文本)。
pub fn extract_voice_reference(
    task_root: &Path,
    segments: &[Segment],
) -> Result<(PathBuf, String), String> {
    let shared = task_root.join("shared");
    let ref_wav = shared.join("ref_voice.wav");
    let ref_txt = shared.join("ref_voice.txt");
    if ref_wav.is_file() {
        let text = std::fs::read_to_string(&ref_txt).map_err(|error| error.to_string())?;
        return Ok((ref_wav, text));
    }
    let segment = pick_reference_segment(segments)
        .ok_or_else(|| "没有可作参考的语音段（需时长 3~20s 且文本 ≥2 字）".to_string())?;
    // 分离开启时 vocals 更干净；否则用原音轨
    let source = if shared.join("vocals.wav").is_file() {
        shared.join("vocals.wav")
    } else {
        shared.join("audio.wav")
    };
    if !source.is_file() {
        return Err(format!("音频产物缺失，无法提取参考音色: {}", source.display()));
    }
    std::fs::create_dir_all(&shared).map_err(|error| error.to_string())?;
    let start = segment.start.to_string();
    let duration = (segment.end - segment.start).to_string();
    let source_str = source.to_string_lossy().into_owned();
    let out_str = ref_wav.to_string_lossy().into_owned();
    // 截取并归一化到 24kHz 单声道（zipvoice 模型规格）
    let status = std::process::Command::new("ffmpeg")
        .args([
            "-y",
            "-ss",
            &start,
            "-t",
            &duration,
            "-i",
            &source_str,
            "-ac",
            "1",
            "-ar",
            "24000",
            "-c:a",
            "pcm_s16le",
            &out_str,
        ])
        .status()
        .map_err(|error| format!("调用 ffmpeg 失败: {error}"))?;
    if !status.success() {
        return Err("ffmpeg 截取参考音频失败".into());
    }
    let text = segment.text.trim();
    std::fs::write(&ref_txt, text).map_err(|error| error.to_string())?;
    log::info!(
        "[voice_ref] 提取参考音色: {:.1}s~{:.1}s → {}",
        segment.start,
        segment.end,
        ref_wav.display()
    );
    Ok((ref_wav, text.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(idx: usize, start: f64, end: f64, text: &str) -> Segment {
        Segment {
            idx,
            start,
            end,
            text: text.into(),
            translated: String::new(),
        }
    }

    #[test]
    fn picks_longest_qualified_segment() {
        let segments = vec![
            seg(0, 0.0, 1.0, "太短"),                        // <3s 排除
            seg(1, 2.0, 30.0, "这个太长了会被过滤掉"), // >20s 排除
            seg(2, 5.0, 10.0, "第二段五秒"),              // 候选
            seg(3, 11.0, 19.0, "第一段八秒最长"),        // 最长 → 选中
        ];
        let picked = pick_reference_segment(&segments).expect("应选出一段");
        assert_eq!(picked.idx, 3);
    }

    #[test]
    fn rejects_too_short_text() {
        let segments = vec![seg(0, 0.0, 6.0, "啊")];
        assert!(pick_reference_segment(&segments).is_none());
    }

    #[test]
    fn ref_voice_is_idempotent_when_already_extracted() {
        let root = std::env::temp_dir().join(format!("voice-ref-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("shared")).unwrap();
        std::fs::write(root.join("shared/ref_voice.wav"), b"RIFF-existing").unwrap();
        std::fs::write(root.join("shared/ref_voice.txt"), "已有参考文本").unwrap();
        let result = extract_voice_reference(&root, &[]).unwrap();
        assert_eq!(result.1, "已有参考文本");
        std::fs::remove_dir_all(root).unwrap();
    }
}