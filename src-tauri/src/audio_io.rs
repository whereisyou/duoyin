//! 音频流式读取（STT 引擎共用）
//!
//! 踩坑记录：整段读入再全量提特征，47 分钟音频单次分配上百 MB，
//! 内存吃紧的机器上直接 alloc abort（不可恢复的硬崩溃）。
//! 所以各引擎统一按窗口流式读，峰值内存与音频时长无关。

use std::path::Path;

pub struct WavStream {
    reader: hound::WavReader<std::io::BufReader<std::fs::File>>,
    pub channels: usize,
    /// 每声道总帧数
    pub total_frames: usize,
}

/// 打开 16kHz WAV 并校验格式（16-bit PCM 或 32-bit float）
pub fn open_wav(path: &Path) -> Result<WavStream, String> {
    open_wav_at(path, 16000)
}

pub fn open_wav_at(path: &Path, sample_rate: u32) -> Result<WavStream, String> {
    let reader = hound::WavReader::open(path).map_err(|e| format!("读取音频失败: {}", e))?;
    let spec = reader.spec();
    if spec.sample_rate != sample_rate {
        return Err(format!(
            "采样率 {} 不支持（需要 {}Hz）",
            spec.sample_rate, sample_rate
        ));
    }
    match (spec.sample_format, spec.bits_per_sample) {
        (hound::SampleFormat::Int, 16) | (hound::SampleFormat::Float, 32) => {}
        _ => return Err("仅支持 16-bit PCM 或 32-bit float WAV".into()),
    }
    Ok(WavStream {
        channels: spec.channels.max(1) as usize,
        total_frames: reader.duration() as usize,
        reader,
    })
}

/// 多声道混单声道（各声道取平均）
pub fn mix_mono(raw: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return raw.to_vec();
    }
    raw.chunks(channels)
        .map(|c| c.iter().sum::<f32>() / channels as f32)
        .collect()
}

/// 从当前位置读最多 max_frames 帧（混合后单声道样本），文件尾可能更少
pub fn read_window(stream: &mut WavStream, max_frames: usize) -> Result<Vec<f32>, String> {
    let take = max_frames * stream.channels;
    let spec = stream.reader.spec();
    let raw: Vec<f32> = match (spec.sample_format, spec.bits_per_sample) {
        (hound::SampleFormat::Int, 16) => stream
            .reader
            .samples::<i16>()
            .take(take)
            .map(|s| s.unwrap_or(0) as f32 / 32768.0)
            .collect(),
        (hound::SampleFormat::Float, 32) => stream
            .reader
            .samples::<f32>()
            .take(take)
            .map(|s| s.unwrap_or(0.0))
            .collect(),
        _ => return Err("仅支持 16-bit PCM 或 32-bit float WAV".into()),
    };
    // 单声道直接移动 raw，省去 mix_mono 的 to_vec 克隆（每 30s 窗一次，长音频累计省数百 MB 拷贝）；
    // 多声道才走 mix_mono 混音求平均。输出内容与旧实现完全一致。
    if stream.channels <= 1 {
        Ok(raw)
    } else {
        Ok(mix_mono(&raw, stream.channels))
    }
}

/// 重定位到指定帧（窗口回退用）
pub fn seek(stream: &mut WavStream, frame: usize) -> Result<(), String> {
    stream
        .reader
        .seek(frame as u32)
        .map_err(|e| format!("音频定位失败: {}", e))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 造一个 16kHz 16bit 正弦 WAV
    fn make_test_wav(path: &Path, secs: f64, channels: u16) {
        let spec = hound::WavSpec {
            channels,
            sample_rate: 16000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(path, spec).unwrap();
        let n = (secs * 16000.0) as usize;
        for i in 0..n {
            let v = (i as f32 * 0.1).sin() * 0.3;
            for _ in 0..channels {
                w.write_sample((v * 32767.0) as i16).unwrap();
            }
        }
        w.finalize().unwrap();
    }

    #[test]
    fn test_mix_mono() {
        assert_eq!(mix_mono(&[0.1, 0.2], 1), vec![0.1, 0.2]);
        let stereo = [1.0, -1.0, 0.5, 0.5];
        let mono = mix_mono(&stereo, 2);
        assert_eq!(mono.len(), 2);
        assert!((mono[0] - 0.0).abs() < 1e-6);
        assert!((mono[1] - 0.5).abs() < 1e-6);
    }

    /// 75s 音频必须被拆成 30+30+15 三个窗口，而不是一次性全量读入
    #[test]
    fn test_window_streaming_splits_long_audio() {
        let dir = std::env::temp_dir().join(format!("vt_audio_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("75s.wav");
        make_test_wav(&path, 75.0, 1);
        const WINDOW: usize = 30 * 16000;

        let mut stream = open_wav(&path).unwrap();
        assert_eq!(stream.total_frames, 75 * 16000);
        assert_eq!(read_window(&mut stream, WINDOW).unwrap().len(), 480_000);
        assert_eq!(read_window(&mut stream, WINDOW).unwrap().len(), 480_000);
        assert_eq!(read_window(&mut stream, WINDOW).unwrap().len(), 240_000);
        assert_eq!(read_window(&mut stream, WINDOW).unwrap().len(), 0);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_window_seek_back() {
        let dir = std::env::temp_dir().join(format!("vt_audio_seek_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("seek.wav");
        make_test_wav(&path, 3.0, 1);

        let mut stream = open_wav(&path).unwrap();
        let _ = read_window(&mut stream, 16000).unwrap();
        seek(&mut stream, 8000).unwrap();
        assert_eq!(read_window(&mut stream, 16000).unwrap().len(), 16000);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_open_wav_rejects_wrong_sample_rate() {
        let dir = std::env::temp_dir().join(format!("vt_audio_sr_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("8k.wav");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 8000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(&path, spec).unwrap();
        w.write_sample(0i16).unwrap();
        w.finalize().unwrap();

        match open_wav(&path) {
            Ok(_) => panic!("应拒绝 8kHz 音频"),
            Err(e) => assert!(e.contains("采样率"), "应报采样率错误: {}", e),
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_open_wav_stereo_mixdown() {
        let dir = std::env::temp_dir().join(format!("vt_audio_st_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("stereo.wav");
        make_test_wav(&path, 1.0, 2);

        let mut stream = open_wav(&path).unwrap();
        assert_eq!(stream.channels, 2);
        assert_eq!(stream.total_frames, 16000);
        assert_eq!(read_window(&mut stream, 30 * 16000).unwrap().len(), 16000);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
