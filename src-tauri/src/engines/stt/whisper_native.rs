//! 本地 Whisper 语音识别 — candle 纯 Rust 推理（无外部进程、无 Python）
//! 模型格式：HuggingFace safetensors 目录（如 openai/whisper-large-v3-turbo），
//! 需含 config.json / model.safetensors / tokenizer.json。

use once_cell::sync::OnceCell;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use candle_core::{Device, IndexOp, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::whisper::{self as m, audio, model::Whisper};
use tokenizers::Tokenizer;

use crate::types::{AppConfig, Segment};

struct WhisperContext {
    model: Whisper,
    tokenizer: Tokenizer,
    device: Device,
    n_mels: usize,
    filters: Vec<f32>,
    suppress: Vec<u32>,
    no_speech: Vec<u32>,
    sot: u32,
    eot: u32,
    transcribe: u32,
    /// <|notimestamps|>，采样时必须屏蔽，否则模型会走无时间戳模式（识别不出段）
    no_timestamps: u32,
    ts_begin: u32,
    max_decode: usize,
}

/// 模型常驻缓存（按目录缓存，切换目录后重建）
static CTX: OnceCell<Mutex<Option<(PathBuf, Arc<WhisperContext>)>>> = OnceCell::new();

fn load_context(dir: &Path) -> Result<Arc<WhisperContext>, String> {
    let cell = CTX.get_or_init(|| Mutex::new(None));
    let mut guard = cell.lock().map_err(|e| e.to_string())?;
    if let Some((cached, ctx)) = guard.as_ref() {
        if cached == dir {
            return Ok(ctx.clone());
        }
    }

    let config_path = dir.join("config.json");
    let model_path = dir.join("model.safetensors");
    let tokenizer_path = dir.join("tokenizer.json");
    for p in [&config_path, &model_path, &tokenizer_path] {
        if !p.is_file() {
            return Err(format!("模型目录缺少文件：{}", p.display()));
        }
    }

    let config: m::Config =
        serde_json::from_str(&std::fs::read_to_string(&config_path).map_err(|e| e.to_string())?)
            .map_err(|e| format!("config.json 解析失败: {}", e))?;
    let tokenizer = Tokenizer::from_file(&tokenizer_path)
        .map_err(|e| format!("tokenizer.json 加载失败: {}", e))?;

    let device = Device::Cpu;
    // SAFETY: 模型文件为本地只读资产，加载期间不会被外部修改
    let vb = unsafe {
        VarBuilder::from_mmaped_safetensors(&[&model_path], m::DTYPE, &device)
            .map_err(|e| format!("模型权重加载失败: {}", e))?
    };
    let model = Whisper::load(&vb, config.clone()).map_err(|e| format!("模型构建失败: {}", e))?;

    let need = |t: &str| {
        tokenizer
            .token_to_id(t)
            .ok_or_else(|| format!("tokenizer 缺少特殊标记 {}", t))
    };
    let no_timestamps = need(m::NO_TIMESTAMPS_TOKEN)?;

    let ctx = Arc::new(WhisperContext {
        n_mels: config.num_mel_bins,
        filters: mel_filters(config.num_mel_bins),
        suppress: config.suppress_tokens.clone(),
        no_speech: m::NO_SPEECH_TOKENS
            .iter()
            .filter_map(|t| tokenizer.token_to_id(t))
            .collect(),
        sot: need(m::SOT_TOKEN)?,
        eot: need(m::EOT_TOKEN)?,
        transcribe: need(m::TRANSCRIBE_TOKEN)?,
        no_timestamps,
        ts_begin: no_timestamps + 1,
        max_decode: config.max_target_positions.min(224),
        model,
        tokenizer,
        device,
    });
    *guard = Some((dir.to_path_buf(), ctx.clone()));
    Ok(ctx)
}

/// librosa slaney 风格 mel 滤波器组（与 OpenAI whisper 相同配置）
/// 返回 n_mels × (n_fft/2+1) 行主序
fn mel_filters(n_mels: usize) -> Vec<f32> {
    const SR: f32 = 16000.0;
    const N_FFT: usize = 400;
    let f_sp = 200.0 / 3.0;
    let min_log_hz = 1000.0f32;
    let min_log_mel = min_log_hz / f_sp; // 15.0
    let logstep = 6.4f32.ln() / 27.0;
    let hz_to_mel = |f: f32| {
        if f >= min_log_hz {
            min_log_mel + (f / min_log_hz).ln() / logstep
        } else {
            f / f_sp
        }
    };
    let mel_to_hz = |mel: f32| {
        if mel >= min_log_mel {
            min_log_hz * (logstep * (mel - min_log_mel)).exp()
        } else {
            mel * f_sp
        }
    };

    let n_freqs = N_FFT / 2 + 1;
    let mel_lo = hz_to_mel(0.0);
    let mel_hi = hz_to_mel(SR / 2.0);
    let mel_f: Vec<f32> = (0..n_mels + 2)
        .map(|i| mel_to_hz(mel_lo + (mel_hi - mel_lo) * i as f32 / (n_mels + 1) as f32))
        .collect();

    let mut filters = vec![0.0f32; n_mels * n_freqs];
    for m in 0..n_mels {
        let (f0, f1, f2) = (mel_f[m], mel_f[m + 1], mel_f[m + 2]);
        let enorm = 2.0 / (f2 - f0); // slaney 面积归一
        for k in 0..n_freqs {
            let fr = SR * k as f32 / N_FFT as f32;
            let lower = (fr - f0) / (f1 - f0);
            let upper = (f2 - fr) / (f2 - f1);
            filters[m * n_freqs + k] = lower.min(upper).max(0.0) * enorm;
        }
    }
    filters
}

// 音频流式读取已抽到 crate::audio_io（STT 引擎共用）；
// 必须按 30s 窗口流式读 + 逐窗提特征，峰值内存与音频时长无关。
use crate::audio_io::{open_wav, read_window, seek as wav_seek};

/// 最大单窗口采样数：3000 mel 帧 × 160 hop = 480000（30s × 16kHz）
const WINDOW_SAMPLES: usize = m::N_FRAMES * m::HOP_LENGTH;

// Whisper 官方多语言模型支持的语言代码。只在这些 token 中做 LID argmax，
// 避免把 translate/transcribe/timestamp 等特殊 token 误判为语言。
const WHISPER_LANGUAGE_CODES: &[&str] = &[
    "en", "zh", "de", "es", "ru", "ko", "fr", "ja", "pt", "tr", "pl", "ca",
    "nl", "ar", "sv", "it", "id", "hi", "fi", "vi", "he", "uk", "el", "ms",
    "cs", "ro", "da", "hu", "ta", "no", "th", "ur", "hr", "bg", "lt", "la",
    "mi", "ml", "cy", "sk", "te", "fa", "lv", "bn", "sr", "az", "sl", "kn",
    "et", "mk", "br", "eu", "is", "hy", "ne", "mn", "bs", "kk", "sq", "sw",
    "gl", "mr", "pa", "si", "km", "sn", "yo", "so", "af", "oc", "ka", "be",
    "tg", "sd", "gu", "am", "yi", "lo", "uz", "fo", "ht", "ps", "tk", "nn",
    "mt", "sa", "lb", "my", "bo", "tl", "mg", "as", "tt", "haw", "ln", "ha",
    "ba", "jw", "su", "yue",
];

fn best_language_token(
    logits: &[f32],
    candidates: &[(u32, &'static str)],
) -> Option<(u32, &'static str)> {
    candidates
        .iter()
        .filter(|(id, _)| (*id as usize) < logits.len())
        .max_by(|(left, _), (right, _)| {
            logits[*left as usize]
                .total_cmp(&logits[*right as usize])
        })
        .copied()
}

fn detect_language(
    ctx: &WhisperContext,
    model: &mut Whisper,
    features: &Tensor,
) -> Result<(u32, &'static str), String> {
    let candidates: Vec<_> = WHISPER_LANGUAGE_CODES
        .iter()
        .filter_map(|&code| {
            ctx.tokenizer
                .token_to_id(&format!("<|{code}|>"))
                .map(|id| (id, code))
        })
        .collect();
    if candidates.is_empty() {
        return Err("Whisper tokenizer 不包含语言 token，无法自动识别语言".into());
    }

    model.reset_kv_cache();
    let input = Tensor::from_vec(vec![ctx.sot], (1, 1), &ctx.device)
        .map_err(|error| error.to_string())?;
    let decoded = model
        .decoder
        .forward(&input, features, true)
        .map_err(|error| format!("Whisper 语言识别 decoder 失败: {error}"))?;
    let logits = model
        .decoder
        .final_linear(&decoded)
        .map_err(|error| format!("Whisper 语言识别 logits 失败: {error}"))?
        .i((0, 0))
        .map_err(|error| error.to_string())?
        .to_vec1::<f32>()
        .map_err(|error| error.to_string())?;
    model.reset_kv_cache();
    best_language_token(&logits, &candidates)
        .ok_or_else(|| "Whisper 自动语言识别没有可用候选".into())
}

/// 贪婪采样：应用抑制词表 + 时间戳规则后取 argmax
fn sample_greedy(ctx: &WhisperContext, mut logits: Vec<f32>, last_ts: Option<u32>) -> u32 {
    let neg = f32::NEG_INFINITY;
    let len = logits.len();
    let mask = |t: u32, l: &mut Vec<f32>| {
        if (t as usize) < l.len() {
            l[t as usize] = neg;
        }
    };
    for &t in &ctx.suppress {
        mask(t, &mut logits);
    }
    for &t in &ctx.no_speech {
        mask(t, &mut logits);
    }
    mask(ctx.sot, &mut logits);
    mask(ctx.transcribe, &mut logits);
    mask(ctx.no_timestamps, &mut logits);

    let tsb = ctx.ts_begin as usize;
    match last_ts {
        // 尚未输出时间戳：首个时间戳不能超过 1.0s（0.02s/单位 × 50）
        None => {
            for l in logits.iter_mut().take(len).skip(tsb + 51) {
                *l = neg;
            }
        }
        // 时间戳必须单调不减
        Some(lt) => {
            for l in logits.iter_mut().take((lt as usize).min(len)).skip(tsb) {
                *l = neg;
            }
        }
    }

    logits
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(i, _)| i as u32)
        .unwrap_or(ctx.eot)
}

/// 对单个 30s 窗口做自回归解码（带时间戳）
fn decode_window(
    ctx: &WhisperContext,
    model: &mut Whisper,
    features: &Tensor,
    lang_token: u32,
) -> Result<Vec<u32>, String> {
    let prompt = vec![ctx.sot, lang_token, ctx.transcribe];
    model.reset_kv_cache();

    let mut tokens: Vec<u32> = prompt.clone();
    let mut last_ts: Option<u32> = None;

    for _ in 0..ctx.max_decode {
        // 每步都喂完整前缀且 flush=true：candle 的 TextDecoder 位置嵌入永远
        // 从 0 开始算，不支持 KV cache 偏移；增量喂单 token 会让所有 token
        // 都拿到 position 0，输出全是幻觉（踩坑实录：识别结果 0 段）
        let t = Tensor::from_vec(tokens.clone(), (1, tokens.len()), &ctx.device)
            .map_err(|e| e.to_string())?;
        let out = model
            .decoder
            .forward(&t, features, true)
            .map_err(|e| format!("decoder 失败: {}", e))?;
        let last = out.dim(1).map_err(|e| e.to_string())? - 1;
        // final_linear 按 x.dim(0) 当 batch 广播词表矩阵，传一维向量会被误解为
        // batch=1280 导致 shape 爆炸（踩坑实录），必须保持 [1, 1, dim] 再取值
        let logits = model
            .decoder
            .final_linear(&out.narrow(1, last, 1).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?
            .i((0, 0))
            .map_err(|e| e.to_string())?
            .to_vec1::<f32>()
            .map_err(|e| e.to_string())?;

        let next = sample_greedy(ctx, logits, last_ts);
        if next == ctx.eot {
            break;
        }
        if next >= ctx.ts_begin {
            last_ts = Some(next);
        }
        tokens.push(next);

        // 重复回路保护：静音/噪声下贪心解码会无限复读同一短语直到 max_decode，
        // 后缀与紧邻前一段完全重合即判定复读，提前终止（不影响正常文本：
        // 正常语音几乎不会出现 ≥2 token 的逐字紧邻重复）
        let n = tokens.len();
        let repeated = (2..=24).any(|p| n >= 2 * p && tokens[n - p..] == tokens[n - 2 * p..n - p]);
        if repeated {
            log::debug!("[stt] 检测到重复回路，提前结束本窗口解码");
            break;
        }
    }
    let generated = tokens[prompt.len()..].to_vec();
    // 窗口级原文留痕：识别为空/异常时不用重跑就能判断是解码问题还是解析问题
    log::debug!(
        "[stt] window tokens: {:?} → \"{}\"",
        generated,
        ctx.tokenizer.decode(&generated, false).unwrap_or_default()
    );
    Ok(generated)
}

/// 把时间戳 token 序列解析为 (start, end, text_ids) 段（窗口相对时间）
fn parse_segments(
    tokens: &[u32],
    ts_begin: u32,
    no_speech: &[u32],
    eot: u32,
) -> Vec<(f64, f64, Vec<u32>)> {
    let mut segs = Vec::new();
    let mut start: Option<f64> = None;
    let mut ids: Vec<u32> = Vec::new();
    for &t in tokens {
        if t >= ts_begin {
            let time = (t - ts_begin) as f64 * 0.02;
            if let Some(s) = start.take() {
                if !ids.is_empty() {
                    segs.push((s, time, std::mem::take(&mut ids)));
                }
            }
            start = Some(time);
        } else if t != eot && !no_speech.contains(&t) {
            ids.push(t);
        }
    }
    // 收尾：最后一个时间戳后没有闭合时间戳（常见于 eot 提前），
    // 用 start+5s 作兜底结束时间，避免整段丢失
    if let Some(s) = start {
        if !ids.is_empty() {
            segs.push((s, s + 5.0, ids));
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
    let dir = cfg.whisper_model_dir.trim();
    if dir.is_empty() {
        return Err("未配置 Whisper 模型目录，请在 设置 → 语音识别 中选择".into());
    }
    // 内存预审：commit 不够 30s 窗口就自动降 15s，还不够就明确报错（而不是 alloc abort 闪退）
    let model_bytes = std::fs::metadata(Path::new(dir).join("model.safetensors"))
        .map(|m| m.len())
        .unwrap_or(0);
    let avail = crate::memcheck::commit_available_bytes();
    let window_secs = crate::memcheck::plan_window(model_bytes, avail)?;
    let window_samples = (window_secs * m::SAMPLE_RATE).min(WINDOW_SAMPLES);
    log::info!(
        "[stt] commit 可用 {}MB，推理窗口 {}s",
        avail.map(|v| v / crate::memcheck::MB).unwrap_or(0),
        window_secs
    );
    let ctx = load_context(Path::new(dir))?;
    let mut lang_token = if lang.trim().is_empty() || lang == "auto" {
        None
    } else {
        Some(
            ctx.tokenizer
                .token_to_id(&format!("<|{}|>", lang))
                .ok_or_else(|| format!("Whisper 不支持语言代码 {}", lang))?,
        )
    };

    let mut stream = open_wav(audio_path)?;
    let total = stream.total_frames;
    if total == 0 {
        return Ok(vec![]);
    }
    let sr = m::SAMPLE_RATE as f64;
    log::info!(
        "[stt] 音频 {:.1}s，按 30s 窗口流式识别（峰值内存与时长无关）",
        total as f64 / sr
    );

    // 克隆模型：共享权重（Arc），独立 KV cache
    let mut model = ctx.model.clone();
    let mut out: Vec<Segment> = Vec::new();
    let mut seek = 0usize; // 单位：单声道采样点

    while seek < total {
        let window = (total - seek).min(window_samples);
        let time_offset = seek as f64 / sr;
        let pcm = read_window(&mut stream, window)?;
        if pcm.is_empty() {
            break;
        }
        let window_dur = pcm.len() as f64 / sr;

        // 逐窗提特征：3000 mel 帧 × 128 mel ≈ 1.5MB，替代原来整段 147MB
        let mel = audio::pcm_to_mel(&ctx.model.config, &pcm, &ctx.filters);
        // candle 的 pcm_to_mel 会把帧数向上取整到 15s 的倍数再 +15s（whisper.cpp 行为），
        // 编码器位置嵌入只有 1500 位（=3000 mel 帧），必须裁回本窗口真实帧数
        let content_frames = (pcm.len() / m::HOP_LENGTH + 1).min(m::N_FRAMES);
        drop(pcm); // 内存敏感环境，用完立即还
        let n_len = mel.len() / ctx.n_mels;
        if content_frames == 0 || n_len == 0 {
            break;
        }
        let mel = Tensor::from_vec(mel, (1, ctx.n_mels, n_len), &ctx.device)
            .map_err(|e| format!("mel 张量构建失败: {}", e))?
            .narrow(2, 0, content_frames)
            .map_err(|e| format!("mel 裁剪失败: {}", e))?;

        let features = model
            .encoder
            .forward(&mel, true)
            .map_err(|e| format!("encoder 失败: {}", e))?;

        if lang_token.is_none() {
            let (detected_token, detected_code) = detect_language(&ctx, &mut model, &features)?;
            log::info!("[stt] 自动识别源语言: {detected_code}");
            lang_token = Some(detected_token);
        }
        let tokens = decode_window(
            &ctx,
            &mut model,
            &features,
            lang_token.expect("首窗已完成语言识别"),
        )?;

        let mut last_end: Option<f64> = None;
        for (s, e, ids) in parse_segments(&tokens, ctx.ts_begin, &ctx.no_speech, ctx.eot) {
            let text = ctx
                .tokenizer
                .decode(&ids, true)
                .unwrap_or_default()
                .trim()
                .to_string();
            if text.is_empty() {
                continue;
            }
            let end = e.min(window_dur);
            if end <= s {
                continue;
            }
            out.push(Segment {
                idx: out.len(),
                start: time_offset + s,
                end: time_offset + end,
                text,
                translated: String::new(),
            });
            last_end = Some(end);
        }

        // 窗口推进：尽量按最后时间戳前进，避免在句中切断；
        // 结束过早（疑似幻觉/静音）则整窗跳过
        let adv = match last_end {
            Some(e) if e > window_dur * 0.25 => ((e * sr) as usize).max(1),
            _ => window,
        };
        seek += adv;
        if seek < total {
            wav_seek(&mut stream, seek)?;
        }
        progress(((seek * 100) / total.max(1)).min(99) as u8);
    }

    model.reset_kv_cache();
    log::info!("[stt] 识别完成，共 {} 段", out.len());
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_best_language_token_only_uses_candidates() {
        let logits = vec![100.0, 1.0, 3.0, 2.0];
        let candidates = [(1, "en"), (2, "zh"), (3, "ja")];
        assert_eq!(best_language_token(&logits, &candidates), Some((2, "zh")));
    }

    #[test]
    fn test_language_codes_cover_primary_and_dialect_languages() {
        for code in ["en", "zh", "ja", "yue"] {
            assert!(WHISPER_LANGUAGE_CODES.contains(&code));
        }
    }

    #[test]
    fn test_mel_filters_shape_and_norm() {
        let f = mel_filters(128);
        assert_eq!(f.len(), 128 * 201);
        // 每个滤波器权重和应大于 0（有覆盖频段）
        for m in 0..128 {
            let sum: f32 = f[m * 201..(m + 1) * 201].iter().sum();
            assert!(sum > 0.0, "filter {} has zero energy", m);
        }
    }

    #[test]
    fn test_parse_segments() {
        let ts_begin = 50364u32; // 假设 ts 起点
        let eot = 50257u32;
        let no_speech: &[u32] = &[1, 2];
        // <|0.00|> hello world <|2.00|> foo <|4.00|>
        let tokens = vec![ts_begin, 100, 200, ts_begin + 100, 300, ts_begin + 200, eot];
        let segs = parse_segments(&tokens, ts_begin, no_speech, eot);
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].0, 0.0);
        assert_eq!(segs[0].1, 2.0);
        assert_eq!(segs[0].2, vec![100, 200]);
        assert_eq!(segs[1].0, 2.0);
        assert_eq!(segs[1].1, 4.0);
        assert_eq!(segs[1].2, vec![300]);
    }

    #[test]
    fn test_parse_segments_trailing_without_closing_ts() {
        let ts_begin = 50364u32;
        let eot = 50257u32;
        // <|1.00|> hello eot —— 没有闭合时间戳，不能丢段
        let tokens = vec![ts_begin + 50, 100, 200, eot];
        let segs = parse_segments(&tokens, ts_begin, &[], eot);
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].0, 1.0);
        assert_eq!(segs[0].2, vec![100, 200]);
    }
}
