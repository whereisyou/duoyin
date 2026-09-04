//! SenseVoice-Small 本地识别 —— sherpa-onnx 官方 Rust 绑定（预编译库，零 CMake）
//!
//! 为什么选它（对比 candle whisper 的实测痛点）：
//! - 模型 245MB int8 vs 1.6GB safetensors 转 f32 常驻 3.2GB
//! - 非自回归一次出全文，RTF≈0.1，比自回归 whisper 快一个量级
//! - token 级时间戳 + ITN 标点，天然适合字幕
//!
//! 模型目录需含 model.int8.onnx + tokens.txt（下载地址见设置页提示）。
//! 注意：识别器按 (目录|语言) 缓存——language 是创建期参数，跨语言任务避免反复加载。

use once_cell::sync::OnceCell;
use std::path::Path;
use std::sync::{Arc, Mutex};

use sherpa_onnx::{
    OfflineRecognizer, OfflineRecognizerConfig, VadModelConfig, VoiceActivityDetector,
};

use crate::audio_io::{open_wav, read_window};
use crate::types::{AppConfig, Segment};

/// 30s 窗口：与 whisper 引擎一致的处理粒度，峰值内存与时长无关
const WINDOW_SAMPLES: usize = 30 * 16000;

/// (模型目录|语言) → 识别器。SenseVoice 模型 ~245MB，缓存避免每任务重载
static RECOG: OnceCell<Mutex<std::collections::HashMap<String, Arc<OfflineRecognizer>>>> =
    OnceCell::new();

fn load_recognizer(dir: &Path, lang: &str) -> Result<Arc<OfflineRecognizer>, String> {
    let language = match lang {
        "zh" | "en" | "ja" | "ko" | "yue" => lang,
        _ => "auto",
    };
    let key = format!("{}|{}", dir.display(), language);

    let cell = RECOG.get_or_init(|| Mutex::new(std::collections::HashMap::new()));
    let mut guard = cell.lock().map_err(|e| e.to_string())?;
    if let Some(r) = guard.get(&key) {
        return Ok(r.clone());
    }

    let model = dir.join("model.int8.onnx");
    let tokens = dir.join("tokens.txt");
    for p in [&model, &tokens] {
        if !p.is_file() {
            return Err(format!("SenseVoice 模型目录缺少文件：{}", p.display()));
        }
    }

    let mut config = OfflineRecognizerConfig::default();
    config.model_config.sense_voice.model = Some(model.to_string_lossy().into());
    config.model_config.sense_voice.language = Some(language.into());
    config.model_config.sense_voice.use_itn = true; // 标点 + 逆文本规范化，字幕刚需
    config.model_config.tokens = Some(tokens.to_string_lossy().into());
    config.model_config.num_threads = 4; // 内存敏感机器，别拉满

    let recog = OfflineRecognizer::create(&config)
        .ok_or_else(|| format!("SenseVoice 识别器创建失败（模型目录：{}）", dir.display()))?;
    let recog = Arc::new(recog);
    guard.insert(key, recog.clone());
    log::info!("[stt:sensevoice] 模型已加载（lang={}）", language);
    Ok(recog)
}

/// token 拼接：英文/BPE 片段间补空格，中文直排。
/// 规则：前后都是 ASCII 字母数字时补一个空格（"Star"+"Trek" → "Star Trek"，
/// 而 "早上"+"9点" 不加，"world"+"." 也不加）。
fn push_token(text: &mut String, tok: &str) {
    let t = tok.replace('▁', " ");
    let need_space = text
        .chars()
        .last()
        .map(|c| c.is_ascii_alphanumeric())
        .unwrap_or(false)
        && t.chars()
            .next()
            .map(|c| c.is_ascii_alphanumeric())
            .unwrap_or(false);
    if need_space {
        text.push(' ');
    }
    text.push_str(&t);
}

/// 把 token 级时间戳聚成字幕段（纯函数，可无模型测试）：
/// - 句读（，。！？；：,.!?;:）后断开
/// - 相邻 token 间隔 >1.5s 处断开
/// - 段尾 = 末 token 时间；尾段兜底 +0.5s
pub fn tokens_to_segments(tokens: &[String], ts: &[f32]) -> Vec<(f64, f64, String)> {
    fn is_punct(t: &str) -> bool {
        matches!(
            t,
            "，" | "。" | "！" | "？" | "；" | "：" | "," | "." | "!" | "?" | ";" | ":"
        )
    }
    let mut segs = Vec::new();
    let mut text = String::new();
    let mut start: Option<f32> = None;
    let mut prev_t: f32 = 0.0;

    for (i, tok) in tokens.iter().enumerate() {
        let t = ts.get(i).copied().unwrap_or(prev_t);
        // 大间隔：先结算上一段（结束于上一 token 的时间）
        if let Some(s) = start {
            if t - prev_t > 1.5 && !text.trim().is_empty() {
                segs.push((s as f64, prev_t as f64, std::mem::take(&mut text)));
                start = None;
            }
        }
        let st = start.get_or_insert(t);
        push_token(&mut text, tok);
        if is_punct(tok.trim()) && !text.trim().is_empty() {
            if t > *st {
                segs.push((*st as f64, t as f64, std::mem::take(&mut text)));
            } else {
                // 长停顿后单独返回的收尾标点没有可用时长，不应成为独立字幕/TTS 段。
                text.clear();
            }
            start = None;
        }
        prev_t = t;
    }
    if let Some(s) = start {
        if !text.trim().is_empty() {
            segs.push((s as f64, (prev_t + 0.5) as f64, text));
        }
    }
    segs
}

/// 主入口：整段音频 → 带时间戳的字幕段（同步阻塞，调用方需放 spawn_blocking）
pub fn transcribe(
    audio_path: &Path,
    lang: &str,
    cfg: &AppConfig,
    progress: impl Fn(u8),
) -> Result<Vec<Segment>, String> {
    transcribe_cancelable(audio_path, lang, cfg, progress, || false)
}

/// 可取消入口：每个 30s 窗口开始前和解码后检查一次。
pub fn transcribe_cancelable(
    audio_path: &Path,
    lang: &str,
    cfg: &AppConfig,
    progress: impl Fn(u8),
    is_canceled: impl Fn() -> bool,
) -> Result<Vec<Segment>, String> {
    let dir = cfg.sensevoice_dir.trim();
    if dir.is_empty() {
        return Err("未配置 SenseVoice 模型目录，请在 设置 → 语音识别 中选择".into());
    }
    let recog = load_recognizer(Path::new(dir), lang)?;
    // Silero VAD：模型目录存在 silero_vad.onnx 才启用（向后兼容）
    let vad = load_vad(Path::new(dir));

    let mut stream = open_wav(audio_path)?;
    let total = stream.total_frames;
    if total == 0 {
        return Ok(vec![]);
    }
    log::info!(
        "[stt:sensevoice] 音频 {:.1}s，按 30s 窗口识别（VAD={}）",
        total as f64 / 16000.0,
        if vad.is_some() { "on" } else { "off" }
    );

    let mut out: Vec<Segment> = Vec::new();
    let mut seek = 0usize;
    while seek < total {
        if is_canceled() {
            return Err("STT 已取消".into());
        }
        let window = (total - seek).min(WINDOW_SAMPLES);
        let offset = seek as f64 / 16000.0;
        let pcm = read_window(&mut stream, window)?;
        if pcm.is_empty() {
            break;
        }

        match &vad {
            Some(vad) => {
                // 窗口内先切语音段：Silero 对静音/背景音乐输出空 → 消除"开头幻听字幕"
                // 必须按 window_size=512 分块喂 + 逐块取段（官方 example 模式）：
                // 实测一次性全量喂超长音频，内部状态机异常截段只剩尾部（vt-scenario 实证，
                // 12s 语音只剩 0.3s "Yeah."），分块喂则完整切出。
                vad.reset();
                const VAD_CHUNK: usize = 512;
                let mut fed = 0usize;
                while fed < pcm.len() {
                    if is_canceled() {
                        return Err("STT 已取消".into());
                    }
                    let end = (fed + VAD_CHUNK).min(pcm.len());
                    vad.accept_waveform(&pcm[fed..end]);
                    // 先拷贝段数据再 pop（SpeechSegment Drop 释放内部指针）
                    while let Some((start_sample, samples)) = (|| {
                        let segment = vad.front()?;
                        Some((segment.start(), segment.samples().to_vec()))
                    })() {
                        vad.pop();
                        push_decoded(
                            &recog,
                            &samples,
                            offset + start_sample as f64 / 16000.0,
                            &mut out,
                        )?;
                    }
                    fed = end;
                }
                vad.flush();
                // flush 后取尾段
                while let Some((start_sample, samples)) = (|| {
                    let segment = vad.front()?;
                    Some((segment.start(), segment.samples().to_vec()))
                })() {
                    vad.pop();
                    push_decoded(&recog, &samples, offset + start_sample as f64 / 16000.0, &mut out)?;
                }
            }
            None => push_decoded(&recog, &pcm, offset, &mut out)?,
        }

        seek += window;
        progress(((seek * 100) / total.max(1)).min(99) as u8);
    }

    log::info!("[stt:sensevoice] 识别完成，共 {} 段", out.len());
    Ok(out)
}

/// 把一段 PCM 交给识别器并将结果并入输出（时间戳 + 段内偏移）
fn push_decoded(
    recog: &Arc<OfflineRecognizer>,
    pcm: &[f32],
    offset: f64,
    out: &mut Vec<Segment>,
) -> Result<(), String> {
    let st = recog.create_stream();
    st.accept_waveform(16000, pcm);
    recog.decode(&st);
    let result = st
        .get_result()
        .ok_or_else(|| "SenseVoice 解码无结果".to_string())?;
    let ts = result.timestamps.unwrap_or_default();
    for (s, e, text) in tokens_to_segments(&result.tokens, &ts) {
        let text = text.trim().to_string();
        if text.is_empty() {
            continue;
        }
        out.push(Segment {
            idx: out.len(),
            start: offset + s,
            end: offset + e,
            text,
            translated: String::new(),
        });
    }
    Ok(())
}

/// 创建 Silero VAD（模型目录存在 silero_vad.onnx 时启用）。
/// SenseVoice 对前导静音/背景音乐会幻觉出字幕，VAD 先切语音段再识别即可根治。
fn load_vad(dir: &Path) -> Option<VoiceActivityDetector> {
    let path = dir.join("silero_vad.onnx");
    if !path.is_file() {
        log::warn!(
            "[stt:sensevoice] 未找到 {}，当前无 VAD 门控：开头静音/音乐可能被识别成语音。\n\
             下载 silero_vad.onnx（约 1.5MB，k2-fsa/sherpa-onnx 发布页）放入该目录即启用",
            path.display()
        );
        return None;
    }
    let mut vad_config = VadModelConfig::default();
    vad_config.silero_vad.model = Some(path.to_string_lossy().into_owned());
    // 0.5：官方推荐默认值（分块喂修复后实测切段正常）
    vad_config.silero_vad.threshold = 0.5;
    vad_config.silero_vad.min_silence_duration = 0.5;
    vad_config.silero_vad.min_speech_duration = 0.25;
    vad_config.silero_vad.window_size = 512;
    vad_config.silero_vad.max_speech_duration = 20.0;
    vad_config.sample_rate = 16000;
    vad_config.num_threads = 2;
    let vad = VoiceActivityDetector::create(&vad_config, 60.0);
    if vad.is_none() {
        log::warn!("[stt:sensevoice] Silero VAD 创建失败，继续无 VAD 识别");
    }
    vad
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn test_segments_split_on_punctuation() {
        // 中文逐字 token：「你好世界。」+「第二句」
        let tokens = toks(&["开", "饭", "时", "间", "，", "吃", "面", "。"]);
        let ts = [0.1, 0.3, 0.5, 0.7, 0.9, 1.5, 1.7, 2.0];
        let segs = tokens_to_segments(&tokens, &ts);
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].2, "开饭时间，");
        assert!((segs[0].0 - 0.1).abs() < 1e-6);
        assert!((segs[0].1 - 0.9).abs() < 1e-6);
        assert_eq!(segs[1].2, "吃面。");
    }

    #[test]
    fn test_gap_followed_by_punctuation_does_not_create_zero_width_segment() {
        let tokens = toks(&["你", "好", "。"]) ;
        let ts = [0.1, 0.5, 3.0];
        let segs = tokens_to_segments(&tokens, &ts);
        assert_eq!(segs.len(), 1);
        assert!(segs.iter().all(|(start, end, _)| end > start));
        assert_eq!(segs[0].2, "你好");
    }

    #[test]
    fn test_leading_punctuation_does_not_create_zero_width_segment() {
        let tokens = toks(&["。", "你", "好"]);
        let ts = [0.3, 0.5, 0.8];
        let segs = tokens_to_segments(&tokens, &ts);
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].2, "你好");
        assert!(segs.iter().all(|(start, end, _)| end > start));
    }

    #[test]
    fn test_segments_split_on_gap() {
        // 无标点但间隔 >1.5s → 断开
        let tokens = toks(&["你", "好", "吗", "我", "很", "好"]);
        let ts = [0.1, 0.3, 0.5, 3.0, 3.2, 3.4];
        let segs = tokens_to_segments(&tokens, &ts);
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].2, "你好吗");
        assert!((segs[0].1 - 0.5).abs() < 1e-6); // 结束于上一 token 时间
        assert_eq!(segs[1].2, "我很好");
    }

    #[test]
    fn test_segments_english_word_spacing() {
        let tokens = toks(&["Star", "Trek", "is", "wonderful", "."]);
        let ts = [0.1, 0.4, 0.6, 0.8, 1.2];
        let segs = tokens_to_segments(&tokens, &ts);
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].2, "Star Trek is wonderful.");
    }

    #[test]
    fn test_segments_bpe_marker_replaced() {
        let tokens = toks(&["▁The", "▁watcher", "▁stands", "."]);
        let ts = [0.1, 0.4, 0.7, 1.0];
        let segs = tokens_to_segments(&tokens, &ts);
        assert_eq!(segs[0].2.trim(), "The watcher stands.");
    }

    #[test]
    fn test_segments_trailing_flush() {
        // 没有收尾标点：兜底 +0.5s 结束
        let tokens = toks(&["还", "没", "说", "完"]);
        let ts = [1.0, 1.2, 1.4, 1.6];
        let segs = tokens_to_segments(&tokens, &ts);
        assert_eq!(segs.len(), 1);
        assert!((segs[0].1 - 2.1).abs() < 1e-6);
    }

    #[test]
    fn test_segments_empty() {
        assert!(tokens_to_segments(&[], &[]).is_empty());
    }
}
