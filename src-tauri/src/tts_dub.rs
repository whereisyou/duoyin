//! TTS 配音时间轴组装与对齐的共享实现（cosyvoice3 / zipvoice / supertonic 三引擎共用）。
//!
//! 此前三引擎各自重复实现同一套「逐段对齐到字幕时间轴 + 静音填补 + 流式写 dub.wav +
//! rubberband 限速 + i16 段 wav 读写」逻辑；统一收敛到这里，改 TTS 只改一处。
//! 与推理框架无关（只做 WAV IO + ffmpeg rubberband 子进程），故不挂 inference feature。

use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use crate::audio_align::{align_wav_to_duration, aligned_path};

/// f32 归一化采样 → 16-bit PCM（削波防溢出）。
/// 仅本地推理引擎（zipvoice/supertonic，inference-gated）用；cosyvoice3 已是 i16。
#[cfg(feature = "inference")]
pub fn to_i16(v: f32) -> i16 {
    (v.clamp(-1.0, 1.0) * 32767.0) as i16
}

/// 把 i16 采样写成 16-bit 单声道 WAV
pub fn write_segment_wav(path: &Path, sample_rate: u32, samples: &[i16]) -> Result<(), String> {
    let mut writer = hound::WavWriter::create(
        path,
        hound::WavSpec {
            channels: 1,
            sample_rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        },
    )
    .map_err(|e| e.to_string())?;
    for sample in samples {
        writer.write_sample(*sample).map_err(|e| e.to_string())?;
    }
    writer.finalize().map_err(|e| e.to_string())
}

/// 读回 16-bit 单声道 WAV 的 i16 采样
pub fn read_segment_wav(path: &Path) -> Result<Vec<i16>, String> {
    hound::WavReader::open(path)
        .map_err(|e| e.to_string())?
        .into_samples::<i16>()
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

/// 超长段按 rubberband 限速对齐到目标时长：写临时 wav → ffmpeg 对齐 → 读回 → 清理临时文件。
/// 段时长不超过目标（或目标非法）时原样返回，不动ffmpeg。
pub fn align_i16_to_duration(
    samples: Vec<i16>,
    actual_seconds: f64,
    target_seconds: f64,
    max_speed_percent: u16,
    sample_rate: u32,
    work_dir: &Path,
    segment_idx: usize,
) -> Result<Vec<i16>, String> {
    if actual_seconds <= target_seconds || target_seconds <= 0.0 {
        return Ok(samples);
    }
    let raw_path = work_dir.join(format!("{segment_idx:04}.raw.wav"));
    let aligned = aligned_path(&raw_path);
    write_segment_wav(&raw_path, sample_rate, &samples)?;
    align_wav_to_duration(
        &raw_path,
        &aligned,
        actual_seconds,
        target_seconds,
        max_speed_percent,
    )?;
    let result = if aligned.is_file() {
        read_segment_wav(&aligned)?
    } else {
        samples
    };
    let _ = std::fs::remove_file(raw_path);
    let _ = std::fs::remove_file(aligned);
    Ok(result)
}

/// 把逐段 i16 采样按字幕时间轴组装成单条 dub.wav。
/// 段前补静音（分块写，避免超长 gap 逐样本分配）、与已写区域重叠时跳过头部。
/// 与原三引擎的时间轴逻辑逐字节等价（分块静音与逐样本静音产出相同 WAV）。
pub struct TimelineWriter {
    writer: hound::WavWriter<BufWriter<File>>,
    sample_rate: u32,
    cursor: usize,
    path: PathBuf,
}

impl TimelineWriter {
    pub fn new(output_dir: &Path, sample_rate: u32) -> Result<Self, String> {
        std::fs::create_dir_all(output_dir).map_err(|e| e.to_string())?;
        let path = output_dir.join("dub.wav");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let writer = hound::WavWriter::new(
            BufWriter::new(File::create(&path).map_err(|e| e.to_string())?),
            spec,
        )
        .map_err(|e| e.to_string())?;
        Ok(Self {
            writer,
            sample_rate,
            cursor: 0,
            path,
        })
    }

    /// 贴一段到时间轴。samples 已是目标采样率的 i16（必要时调用方先用 to_i16 转换）。
    pub fn push(&mut self, start_seconds: f64, samples: &[i16]) -> Result<(), String> {
        let start = (start_seconds * self.sample_rate as f64) as usize;
        if start > self.cursor {
            self.write_silence(start - self.cursor)?;
        }
        let skip = self.cursor.saturating_sub(start).min(samples.len());
        self.write_samples(&samples[skip..])?;
        self.cursor = start.max(self.cursor) + (samples.len() - skip);
        Ok(())
    }

    fn write_silence(&mut self, n: usize) -> Result<(), String> {
        const CHUNK: usize = 16384;
        let zeros = [0i16; CHUNK];
        let mut left = n;
        while left > 0 {
            let k = left.min(CHUNK);
            for &s in &zeros[..k] {
                self.writer.write_sample(s).map_err(|e| e.to_string())?;
            }
            left -= k;
        }
        Ok(())
    }

    fn write_samples(&mut self, samples: &[i16]) -> Result<(), String> {
        for &v in samples {
            self.writer.write_sample(v).map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    pub fn finalize(self) -> Result<PathBuf, String> {
        let path = self.path.clone();
        self.writer.finalize().map_err(|e| e.to_string())?;
        Ok(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeline_places_segments_with_silence_gaps() {
        let root = std::env::temp_dir().join(format!("tts-dub-{}", uuid::Uuid::new_v4()));
        let mut tl = TimelineWriter::new(&root, 10).unwrap();
        // 段 A 占 [0,3)，段 B 从 5 开始（[3,5) 补 2 个静音）
        tl.push(0.0, &[1, 1, 1]).unwrap();
        tl.push(0.5, &[2, 2]).unwrap();
        let dub = tl.finalize().unwrap();
        let samples = read_segment_wav(&dub).unwrap();
        assert_eq!(samples, vec![1, 1, 1, 0, 0, 2, 2]);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn timeline_skips_overlapping_head() {
        let root = std::env::temp_dir().join(format!("tts-dub-{}", uuid::Uuid::new_v4()));
        let mut tl = TimelineWriter::new(&root, 10).unwrap();
        tl.push(0.0, &[1, 1, 1, 1, 1]).unwrap(); // cursor=5
        tl.push(0.3, &[9, 9, 9, 9]).unwrap(); // start=3 < cursor=5，跳过头部 2 个 → 写 [9,9]
        let dub = tl.finalize().unwrap();
        let samples = read_segment_wav(&dub).unwrap();
        assert_eq!(samples, vec![1, 1, 1, 1, 1, 9, 9]);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn short_segment_skips_alignment() {
        // actual <= target：不触发 ffmpeg，原样返回
        let root = std::env::temp_dir().join(format!("tts-dub-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let samples = vec![1, 2, 3];
        let out = align_i16_to_duration(samples.clone(), 0.5, 1.0, 125, 10, &root, 0).unwrap();
        assert_eq!(out, samples);
        std::fs::remove_dir_all(root).unwrap();
    }
}
