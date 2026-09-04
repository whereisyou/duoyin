//! Supertonic 3 本地 TTS 封装（ONNX Runtime 推理，无 Python/外部进程）
//! 模型目录结构：<dir>/onnx/*.onnx + <dir>/voice_styles/*.json（HF Supertone/supertonic-3）
//!
//! 中文扩展（Supertonic-ZH）：把 *_zh.onnx 三件 + unicode_indexer_zh.json 放入 onnx/，
//! 即可在官方 31 语言之外支持中文配音（vocoder 语言无关，与官方共用）。
//!
//! 本模块是对外唯一入口：合成（synthesize_segments*）在此，资产校验（official_available /
//! zh_available / lang_supported / validate_language_assets 等）在 assets.rs 并在此 re-export，
//! ONNX/张量内部件在 helper.rs（仅 supertonic 子树可见）。

// helper 是 ONNX/张量内部件，仅 supertonic 子树可见（不外泄到 engines 命名空间）。
pub(in crate::engines::tts::supertonic) mod helper;
mod assets;

pub use assets::{
    lang_supported, official_available, validate_language_assets, zh_available,
};

use once_cell::sync::OnceCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use self::helper as sh;
use crate::types::{AppConfig, Segment};

/// (模型目录|变体) → TTS 引擎。官方与 ZH 变体可共存，
/// 避免跨语言任务时反复加载数百 MB 的 ONNX 会话
static ENGINES: OnceCell<Mutex<HashMap<String, sh::TextToSpeech>>> = OnceCell::new();
/// 音色文件路径 → 音色（小对象，按路径缓存）
static STYLES: OnceCell<Mutex<HashMap<PathBuf, sh::Style>>> = OnceCell::new();

static ORT_INIT: OnceCell<Result<(), String>> = OnceCell::new();

/// load-dynamic 模式：首次使用时从程序目录加载 onnxruntime.dll
/// 查找顺序：程序目录 → 上级目录（cargo test 时 exe 在 target/debug/deps/ 下）
fn ensure_ort() -> Result<(), String> {
    ORT_INIT
        .get_or_init(|| {
            let exe_dir = std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(|d| d.to_path_buf()))
                .ok_or("无法定位程序目录")?;
            let dll = [
                Some(exe_dir.clone()),
                exe_dir.parent().map(|p| p.to_path_buf()),
            ]
            .into_iter()
            .flatten()
            .map(|d| d.join("onnxruntime.dll"))
            .find(|p| p.is_file())
            .ok_or_else(|| {
                format!(
                    "未找到 onnxruntime.dll（应位于程序目录 {}），请重新构建或手动放入",
                    exe_dir.display()
                )
            })?;
            ort::init_from(&dll)
                .map_err(|e| format!("ONNX Runtime 初始化失败: {}", e))?
                .commit();
            Ok(())
        })
        .clone()
}

/// 加载指定变体的引擎（zh=true 时用 *_zh.onnx 三件，vocoder 始终官方）
fn load_engine(dir: &str, zh: bool) -> Result<sh::TextToSpeech, String> {
    let d = assets::onnx_dir(dir);
    let suffix = if zh { "_zh" } else { "" };

    let cfgs = sh::load_cfgs(&d).map_err(|e| format!("读取 tts.json 失败: {}", e))?;
    let indexer = d.join(format!("unicode_indexer{}.json", suffix));
    if !indexer.is_file() {
        return Err(format!(
            "未找到语言索引文件：{}{}",
            indexer.display(),
            if zh {
                "（需下载 Supertonic-ZH 扩展并放入 onnx 目录）"
            } else {
                ""
            }
        ));
    }
    let text_processor =
        sh::UnicodeProcessor::new(&indexer).map_err(|e| format!("语言索引加载失败: {}", e))?;

    let session = |name: &str, sfx: &str| -> Result<ort::session::Session, String> {
        let p = d.join(format!("{}{}.onnx", name, sfx));
        if !p.is_file() {
            return Err(format!("未找到模型文件：{}", p.display()));
        }
        ort::session::Session::builder()
            .and_then(|mut b| b.commit_from_file(&p))
            .map_err(|e| format!("加载模型失败 {}: {}", p.display(), e))
    };

    Ok(sh::TextToSpeech::new(
        cfgs,
        text_processor,
        session("duration_predictor", suffix)?,
        session("text_encoder", suffix)?,
        session("vector_estimator", suffix)?,
        session("vocoder", "")?, // 语言无关，始终用官方
    ))
}

fn get_engine(dir: &str, zh: bool) -> Result<(), String> {
    let key = format!("{}|{}", dir, if zh { "zh" } else { "official" });
    let cell = ENGINES.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = cell.lock().map_err(|e| e.to_string())?;
    if !guard.contains_key(&key) {
        let engine = load_engine(dir, zh)?;
        guard.insert(key.clone(), engine);
    }
    Ok(())
}

/// 音色解析：优先 voice_styles/ 子目录，其次资产根目录
fn resolve_voice(dir: &str, voice: &str) -> Result<PathBuf, String> {
    let assets = Path::new(dir);
    for p in [
        assets.join("voice_styles").join(format!("{}.json", voice)),
        assets.join(format!("{}.json", voice)),
    ] {
        if p.is_file() {
            return Ok(p);
        }
    }
    Err(format!(
        "未找到音色文件 {}.json（查找位置：voice_styles/ 与资产根目录）",
        voice
    ))
}

/// 为翻译后的字幕段逐段合成配音：
/// - 每段写出 <out>/audio_segments_tts/NNNN.wav（44.1kHz）
/// - 按原始时间轴对齐拼接出 <out>/dub.wav（与原音频等长，空白处静音）
pub fn synthesize_segments(
    segments: &[Segment],
    lang: &str,
    cfg: &AppConfig,
    out: &Path,
    progress: impl Fn(u8),
) -> Result<(), String> {
    synthesize_segments_cancelable(segments, lang, cfg, out, progress, 125, || false)
}

pub fn synthesize_segments_cancelable(
    segments: &[Segment],
    lang: &str,
    cfg: &AppConfig,
    out: &Path,
    progress: impl Fn(u8),
    max_speed_percent: u16,
    is_canceled: impl Fn() -> bool,
) -> Result<(), String> {
    let dir = cfg.supertonic_dir.trim();
    if dir.is_empty() {
        return Err("未配置 Supertonic 模型目录，请在 设置 → 语音合成 中选择".into());
    }
    let zh = lang == "zh";
    if zh && !zh_available(dir) {
        return Err(
            "中文配音需要 Supertonic-ZH 扩展：将 *_zh.onnx 与 unicode_indexer_zh.json 放入资产的 onnx 目录"
                .into(),
        );
    }
    let voice = match cfg.supertonic_voice.trim() {
        "" => {
            if zh {
                "voice_zh"
            } else {
                "M1"
            }
        }
        v => v,
    };
    let engine_key = format!("{}|{}", dir, if zh { "zh" } else { "official" });

    ensure_ort()?;
    get_engine(dir, zh)?;

    // 音色（按路径缓存）
    let style_path = resolve_voice(dir, voice)?;
    let styles = STYLES.get_or_init(|| Mutex::new(HashMap::new()));
    {
        let mut guard = styles.lock().map_err(|e| e.to_string())?;
        if !guard.contains_key(&style_path) {
            let style = sh::load_voice_style(&[style_path.to_string_lossy().to_string()], false)
                .map_err(|e| format!("加载音色失败: {}", e))?;
            guard.insert(style_path.clone(), style);
        }
    }

    let engines = ENGINES.get_or_init(|| Mutex::new(HashMap::new()));
    let mut engines_guard = engines.lock().map_err(|e| e.to_string())?;
    let tts = engines_guard
        .get_mut(&engine_key)
        .ok_or("TTS 引擎未初始化")?;
    let styles_guard = styles.lock().map_err(|e| e.to_string())?;
    let style = styles_guard.get(&style_path).ok_or("音色未初始化")?;

    let sr = tts.sample_rate as usize;
    let seg_dir = out.join("audio_segments_tts");
    std::fs::create_dir_all(&seg_dir).map_err(|e| e.to_string())?;

    // dub 时间轴组装收敛到共享 tts_dub::TimelineWriter（流式写、段间补静音，与旧实现逐字节等价，
    // 内存只占单段大小——替代旧的全长 timeline Vec，47 分钟视频曾单次分配 505MB）
    let mut dub = crate::tts_dub::TimelineWriter::new(out, tts.sample_rate as u32)?;

    let synth_total = segments
        .iter()
        .filter(|s| !s.translated.trim().is_empty())
        .count()
        .max(1);
    let mut done = 0usize;

    for seg in segments {
        if is_canceled() {
            return Err("TTS 已取消".into());
        }
        let text = seg.translated.trim();
        if text.is_empty() {
            continue;
        }
        let (wav, dur) = tts
            .call(text, lang, style, 8, 1.05, 0.3)
            .map_err(|e| format!("合成失败（第 {} 段）: {}", seg.idx + 1, e))?;
        if is_canceled() {
            return Err("TTS 已取消".into());
        }
        let actual = ((dur * sr as f32) as usize).min(wav.len());
        let mut aligned_wav = wav[..actual].to_vec();
        let segment_path = seg_dir.join(format!("{:04}.wav", seg.idx + 1));
        sh::write_wav_file(&segment_path, &aligned_wav, tts.sample_rate)
            .map_err(|e| format!("写出配音片段失败: {}", e))?;
        let target_duration = (seg.end - seg.start).max(0.0);
        if dur as f64 > target_duration && target_duration > 0.0 {
            let aligned_path = crate::audio_align::aligned_path(&segment_path);
            crate::audio_align::align_wav_to_duration(
                &segment_path,
                &aligned_path,
                dur as f64,
                target_duration,
                max_speed_percent,
            )?;
            if aligned_path.is_file() {
                aligned_wav = hound::WavReader::open(&aligned_path)
                    .map_err(|e| e.to_string())?
                    .into_samples::<i16>()
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|e| e.to_string())?
                    .into_iter()
                    .map(|sample| sample as f32 / 32767.0)
                    .collect();
                let _ = std::fs::remove_file(aligned_path);
            }
        }
        // 贴到时间轴：段前补静音；与已写区域重叠时跳过头部（共享实现，f32→i16 在写入前转换）
        let samples_i16: Vec<i16> = aligned_wav
            .iter()
            .map(|v| crate::tts_dub::to_i16(*v))
            .collect();
        dub.push(seg.start, &samples_i16)?;

        done += 1;
        progress(((done * 100) / synth_total).min(99) as u8);
    }

    dub.finalize()
        .map_err(|e| format!("写出 dub.wav 失败: {}", e))?;
    Ok(())
}

