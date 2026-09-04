//! 端到端流水线验证 —— 需要本地模型，不参与常规 `cargo test`
//!
//! 运行方式：
//!   cargo test --features inference -- --ignored --nocapture
//!
//! 覆盖链路：TTS（Supertonic 本地合成真实语音）→ ffmpeg 转 16kHz →
//! STT（candle Whisper 本地识别）。不依赖任何外部 API，全程可离线。
//! 第二段语音故意放在 31s 处，横跨 30s 推理窗口，
//! 回归验证「长音频流式分窗」修复（原 47 分钟全量 mel 单次分配 147MB 导致 OOM abort）。

use std::path::PathBuf;

use crate::types::{AppConfig, Segment};

/// 把任意音频转成 STT 输入要求的 16kHz 单声道 WAV。
/// e2e 验证的是新 pipeline，故用新媒体工具 adapters::media::FfmpegMediaTool，
/// 不依赖 legacy::ffmpeg（冻结区）。
async fn to_stt_wav_16k(input: &std::path::Path, output: &std::path::Path) {
    use crate::ports::media_tool::MediaTool;

    crate::adapters::media::ffmpeg::FfmpegMediaTool::default()
        .extract_stt_audio(
            input,
            output,
            &crate::pipeline::runner::CancelToken::default(),
        )
        .await
        .expect("ffmpeg 转采样失败");
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.into())
}

fn whisper_dir() -> PathBuf {
    PathBuf::from(env_or(
        "VT_WHISPER_DIR",
        r"E:\projects\text2voices\CosyVoice\pretrained_models\whisper-large-v3-turbo",
    ))
}

fn supertonic_dir() -> PathBuf {
    PathBuf::from(env_or(
        "VT_SUPERTONIC_DIR",
        r"E:\projects\pyvideotrans-3.98\Supertone\supertonic-3",
    ))
}

fn sensevoice_dir() -> PathBuf {
    PathBuf::from(env_or(
        "VT_SENSEVOICE_DIR",
        r"E:\projects\test2voices_backup\sense-voice-int8",
    ))
}

/// 模型目录缺失时跳过而不是失败（没有模型的开发机也能跑测试套件）
macro_rules! need_dir {
    ($p:expr, $name:expr) => {
        if !$p.is_dir() {
            eprintln!("[e2e] 跳过：{} 不存在（{}）", $name, $p.display());
            return;
        }
    };
}

/// 低 commit 可用内存时跳过 TTS 真机用例：Supertonic 合成中 ORT 的 C++ 分配失败
/// 会穿过 extern "C" 边界直接 abort（0xc0000409，fatal runtime error，panic hook 捕不到），
/// 只能在开工前用 commit 水位预判（对齐 whisper_existing_audio 的“内存不足→skip”模式）。
macro_rules! need_mem {
    ($mb:expr, $name:expr) => {
        match crate::memcheck::commit_available_bytes() {
            Some(avail) if avail < ($mb * crate::memcheck::MB) => {
                eprintln!(
                    "[e2e] 跳过：{} 需要约 {}MB commit 可用（当前仅 {:.1}GB）",
                    $name,
                    $mb,
                    avail as f64 / 1073741824.0
                );
                return;
            }
            _ => {}
        }
    };
}

#[test]
#[ignore = "需要 VT_TEST_AUDIO 和本地 Whisper 模型"]
fn whisper_existing_audio_satisfies_stt_contract() {
    crate::logger::init();
    let whisper = whisper_dir();
    need_dir!(whisper, "whisper 模型目录");
    let Ok(audio) = std::env::var("VT_TEST_AUDIO") else {
        eprintln!("[e2e] 跳过：未设置 VT_TEST_AUDIO");
        return;
    };
    let audio = PathBuf::from(audio);
    if !audio.is_file() {
        eprintln!("[e2e] 跳过：测试音频不存在（{}）", audio.display());
        return;
    }
    let config = AppConfig {
        whisper_model_dir: whisper.to_string_lossy().into_owned(),
        ..Default::default()
    };
    let language = std::env::var("VT_TEST_LANG").unwrap_or_else(|_| "auto".into());
    match crate::engines::stt::whisper_native::transcribe(&audio, &language, &config, |_| {}) {
        Ok(segments) => {
            let segments = crate::ports::stt::sanitize_segments(segments)
                .expect("自动语言识别结果应满足 STT 端口契约");
            assert!(!segments.is_empty(), "Whisper 识别后没有字幕段");
            assert!(segments.iter().all(|segment| segment.end > segment.start));
            let duration = hound::WavReader::open(&audio)
                .map(|reader| reader.duration() as f64 / reader.spec().sample_rate as f64)
                .expect("读取测试音频时长失败");
            assert!(segments.iter().all(|segment| segment.end <= duration + 0.02));
            eprintln!("[e2e] Whisper auto 结果: {segments:?}");
        }
        Err(error) if error.contains("内存不足") => {
            eprintln!("[e2e] 跳过 Whisper 推理：{error}");
        }
        Err(error) => panic!("Whisper auto 失败: {error}"),
    }
}

#[tokio::test]
#[ignore = "需要本地模型，显式运行：cargo test -- --ignored --nocapture"]
async fn tts_then_stt_cross_window() {
    crate::logger::init();
    let sup = supertonic_dir();
    need_dir!(sup, "supertonic 模型目录");
    let whi = whisper_dir();
    need_dir!(whi, "whisper 模型目录");
    need_mem!(2048, "Supertonic TTS 合成");

    let tmp = std::env::temp_dir().join(format!("vt_e2e_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();

    // 第 1 步：TTS 合成两段真实语音，第二段放在 31s（横跨 30s 窗口边界）
    let tts_cfg = AppConfig {
        supertonic_dir: sup.to_string_lossy().into(),
        ..Default::default()
    };
    let segs = vec![
        Segment {
            idx: 0,
            start: 0.5,
            end: 4.0,
            text: String::new(),
            translated: "The watcher stands on the bridge.".into(),
        },
        Segment {
            idx: 1,
            start: 31.0,
            end: 35.0,
            text: String::new(),
            translated: "Star trek is a wonderful series.".into(),
        },
    ];
    crate::engines::tts::supertonic::synthesize_segments(&segs, "en", &tts_cfg, &tmp, |p| {
        eprintln!("[e2e] tts {}%", p)
    })
    .expect("TTS 合成失败");
    let dub = tmp.join("dub.wav");
    assert!(dub.is_file(), "dub.wav 未生成");

    // 第 2 步：ffmpeg 转 16kHz（STT 输入要求）
    let wav16 = tmp.join("dub_16k.wav");
    to_stt_wav_16k(&dub, &wav16).await;

    // 第 3 步：STT 识别
    let stt_cfg = AppConfig {
        whisper_model_dir: whi.to_string_lossy().into(),
        ..Default::default()
    };
    let out = match crate::engines::stt::whisper_native::transcribe(&wav16, "en", &stt_cfg, |p| {
        eprintln!("[e2e] stt {}%", p)
    }) {
        Ok(output) => output,
        Err(error) if error.contains("内存不足") => {
            eprintln!("[e2e] 跳过 Whisper 推理：{error}");
            let _ = std::fs::remove_dir_all(&tmp);
            return;
        }
        Err(error) => panic!("STT 识别失败: {error}"),
    };

    let timeline: Vec<String> = out
        .iter()
        .map(|s| format!("[{:.1}-{:.1}] {}", s.start, s.end, s.text))
        .collect();
    eprintln!("[e2e] 识别结果: {:?}", timeline);

    assert!(!out.is_empty(), "STT 未识别出任何字幕段");
    let all = out
        .iter()
        .map(|s| s.text.to_lowercase())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        all.contains("watcher") || all.contains("bridge"),
        "第一段识别偏差过大: {}",
        all
    );
    // 30s 之后必须有识别结果 → 证明跨窗口时间轴拼接正确
    assert!(
        out.iter().any(|s| s.start >= 25.0),
        "30s 之后的语音未被识别（分窗逻辑回归）: {:?}",
        out.iter().map(|s| s.start).collect::<Vec<_>>()
    );

    let _ = std::fs::remove_dir_all(&tmp);
    eprintln!("[e2e] TTS→STT 全链路通过（含跨 30s 窗口）");
}

/// SenseVoice 引擎 e2e：用 whisper 的 16k 测试音（由 TTS e2e 生成太绕，
/// 直接用 ffmpeg 造一段太假——这里复用 TTS 合成的真实语音）
#[tokio::test]
#[ignore = "需要本地模型，显式运行：cargo test -- --ignored --nocapture"]
async fn sensevoice_cross_window_roundtrip() {
    let tts_dir = supertonic_dir();
    let sv_dir = sensevoice_dir();
    need_dir!(tts_dir, "Supertonic 目录");
    need_dir!(sv_dir, "SenseVoice 模型目录");
    need_mem!(2048, "Supertonic TTS 合成");
    let tmp = std::env::temp_dir().join(format!("vt_e2e_sv_cross_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let tts_cfg = AppConfig {
        supertonic_dir: tts_dir.to_string_lossy().into(),
        supertonic_voice: "M1".into(),
        ..Default::default()
    };
    let segments = vec![
        Segment {
            idx: 0,
            start: 0.5,
            end: 4.0,
            text: String::new(),
            translated: "The watcher stands on the bridge.".into(),
        },
        Segment {
            idx: 1,
            start: 31.0,
            end: 35.0,
            text: String::new(),
            translated: "Star trek is a wonderful series.".into(),
        },
    ];
    crate::engines::tts::supertonic::synthesize_segments(&segments, "en", &tts_cfg, &tmp, |_| {})
        .expect("TTS 合成失败");
    let wav16 = tmp.join("dub_16k.wav");
    to_stt_wav_16k(&tmp.join("dub.wav"), &wav16).await;
    let stt_cfg = AppConfig {
        sensevoice_dir: sv_dir.to_string_lossy().into(),
        ..Default::default()
    };
    let output = crate::engines::stt::sensevoice::transcribe(&wav16, "en", &stt_cfg, |_| {})
        .expect("SenseVoice 跨窗口识别失败");
    assert!(
        output.iter().any(|segment| segment.start >= 29.0),
        "30秒后无字幕: {output:?}"
    );
    let _ = std::fs::remove_dir_all(tmp);
}

#[tokio::test]
#[ignore = "诊断：VAD 参数扫描"]
async fn vad_diagnostic() {
    use sherpa_onnx::{SileroVadModelConfig, VadModelConfig, VoiceActivityDetector};
    crate::logger::init();
    let sv = sensevoice_dir();
    need_dir!(sv, "sensevoice 模型目录");
    let sup = supertonic_dir();
    need_dir!(sup, "supertonic 模型目录（合成语音用）");

    let tmp = std::env::temp_dir().join(format!("vt_vad_diag_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    // supertonic 合成 12s 连续语音（与 stt_roundtrip 同文本）
    let tts_cfg = AppConfig {
        supertonic_dir: sup.to_string_lossy().into(),
        ..Default::default()
    };
    crate::engines::tts::supertonic::synthesize_segments(
        &[Segment { idx: 0, start: 0.0, end: 12.0, text: String::new(), translated: "The watcher stands on the bridge and watches the ships pass under it, while the city lights flicker in the cold night air.".into() }],
        "en",
        &tts_cfg,
        &tmp,
        |_| {},
    )
    .unwrap();
    let wav16 = tmp.join("dub_16k.wav");
    to_stt_wav_16k(&tmp.join("dub.wav"), &wav16).await;

    // 读 pcm + RMS
    let mut stream = crate::audio_io::open_wav(&wav16).unwrap();
    let total = stream.total_frames;
    let pcm = crate::audio_io::read_window(&mut stream, total).unwrap();
    let rms = (pcm.iter().map(|s| s * s).sum::<f32>() / pcm.len() as f32).sqrt();
    let peak = pcm.iter().cloned().fold(0.0f32, f32::max);
    eprintln!("[vad-diag] pcm: {} samples = {:.1}s, RMS={:.4}, peak={:.4}", pcm.len(), pcm.len() as f64 / 16000.0, rms, peak);

    for window in [512i32, 1024] {
        eprintln!("[vad-diag] === window={window} ===");
        let silero = SileroVadModelConfig {
            model: Some(sv.join("silero_vad.onnx").to_string_lossy().into_owned()),
            threshold: 0.5,
            min_silence_duration: 0.5,
            min_speech_duration: 0.25,
            max_speech_duration: 20.0,
            window_size: window,
        };
        let config = VadModelConfig {
            silero_vad: silero,
            sample_rate: 16000,
            num_threads: 2,
            ..Default::default()
        };
        eprintln!("[vad-diag] creating VAD (window={window})...");
        let Some(vad) = VoiceActivityDetector::create(&config, 60.0) else {
            eprintln!("[vad-diag] window={window}: create FAILED");
            continue;
        };
        eprintln!("[vad-diag] accepting {} samples...", pcm.len());
        // 官方 example 模式：分块喂 + 逐块取段（对比一次性全量喂）
        let chunk = 512usize;
        let mut fed = 0usize;
        let mut emitted = 0usize;
        while fed < pcm.len() {
            let end = (fed + chunk).min(pcm.len());
            vad.accept_waveform(&pcm[fed..end]);
            while let Some(seg) = vad.front() {
                emitted += 1;
                eprintln!(
                    "[vad-diag]   stream-emit #{emitted}: {:.2}-{:.2}s",
                    seg.start() as f64 / 16000.0,
                    (seg.start() + seg.n()) as f64 / 16000.0
                );
                vad.pop();
            }
            fed = end;
        }
        vad.flush();
        eprintln!("[vad-diag] collecting segments (after flush)...");
        let mut segments = Vec::new();
        while let Some(seg) = vad.front() {
            segments.push((seg.start(), seg.n()));
            vad.pop();
        }
        let desc = segments
            .iter()
            .map(|(s, n)| format!("{:.2}-{:.2}s", *s as f64 / 16000.0, (*s + *n) as f64 / 16000.0))
            .collect::<Vec<_>>()
            .join(", ");
        eprintln!(
            "[vad-diag] window={window}: {} 段 [{}] ({:.1}% 语音)",
            segments.len(),
            if desc.is_empty() { "无".into() } else { desc },
            segments.iter().map(|(_, n)| *n).sum::<i32>() as f64 * 100.0 / pcm.len() as f64
        );
    }
    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
#[ignore = "需要本地模型，显式运行：cargo test -- --ignored --nocapture"]
async fn sensevoice_stt_roundtrip() {
    crate::logger::init();
    let sup = supertonic_dir();
    need_dir!(sup, "supertonic 模型目录");
    let sv = sensevoice_dir();
    need_dir!(sv, "sensevoice 模型目录");
    need_mem!(2048, "Supertonic TTS 合成");

    let tmp = std::env::temp_dir().join(format!("vt_e2e_sv_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();

    // TTS 造真实语音（en）
    let tts_cfg = AppConfig {
        supertonic_dir: sup.to_string_lossy().into(),
        ..Default::default()
    };
    let segs = vec![Segment {
        idx: 0,
        start: 0.0,
        end: 12.0,
        text: String::new(),
        translated: "The watcher stands on the bridge and watches the ships pass under it, while the city lights flicker in the cold night air.".into(),
    }];
    crate::engines::tts::supertonic::synthesize_segments(&segs, "en", &tts_cfg, &tmp, |_| {})
        .expect("TTS 合成失败");

    let wav16 = tmp.join("dub_16k.wav");
    to_stt_wav_16k(&tmp.join("dub.wav"), &wav16).await;

    // SenseVoice 识别
    let stt_cfg = AppConfig {
        sensevoice_dir: sv.to_string_lossy().into(),
        ..Default::default()
    };
    let out = crate::engines::stt::sensevoice::transcribe(&wav16, "en", &stt_cfg, |p| {
        eprintln!("[e2e] sensevoice {}%", p)
    })
    .expect("SenseVoice 识别失败");

    eprintln!(
        "[e2e] sensevoice 结果: {:?}",
        out.iter()
            .map(|s| format!("[{:.1}-{:.1}] {}", s.start, s.end, s.text))
            .collect::<Vec<_>>()
    );
    assert!(!out.is_empty(), "SenseVoice 未识别出任何字幕段");
    let all = out
        .iter()
        .map(|s| s.text.to_lowercase())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        all.contains("watcher") || all.contains("bridge"),
        "识别偏差过大: {}",
        all
    );

    let _ = std::fs::remove_dir_all(&tmp);
    eprintln!("[e2e] SenseVoice 引擎通过");
}
