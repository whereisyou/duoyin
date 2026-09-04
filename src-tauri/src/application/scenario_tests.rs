//! 真机场景测试矩阵（verifier t32636-3 胜者方案 C：薄壳场景函数 + 共享 runner + 声明式断言）。
//!
//! 素材：真实视频（默认 `E:\projects\pyvideotrans-3.98\10.mp4`，`VT_TEST_VIDEO` 可覆盖）。
//! 翻译：本地 mock（TcpListener 假 OpenAI 兼容服务，译文固定 `translated-{idx}`）——免费、稳定、不依赖 key。
//! 运行：`npm run e2e` 全量；单场景过滤：
//! `cargo test --features inference -- --ignored --nocapture scenario_dual_track`
//! 缺模型 / 缺测试视频 / commit 内存不足 → 打印原因后跳过（不视为失败）。
//! 场景矩阵登记：FUNCTION_CHECKLIST.md「测试矩阵」节；改流水线行为后跑一遍全量。

use std::path::PathBuf;
use std::sync::Arc;

use crate::application::pipeline_service::run_configured_pipeline;
use crate::application::task_service::TaskService;
use crate::domain::config::{EngineSelection, OutputConfig, PipelineConfig, SeparationConfig};
use crate::domain::variant::TargetVariant;
use crate::pipeline::runner::{CancelToken, PipelineObserver};
use crate::types::AppConfig;

macro_rules! need_dir {
    ($p:expr, $name:expr) => {
        if !$p.is_dir() {
            eprintln!("[scenario] 跳过：{} 不存在（{}）", $name, $p.display());
            return None;
        }
    };
}

macro_rules! need_file {
    ($p:expr, $name:expr) => {
        if !$p.is_file() {
            eprintln!("[scenario] 跳过：{} 不存在（{}）", $name, $p.display());
            return None;
        }
    };
}

macro_rules! need_mem {
    ($mb:expr, $name:expr) => {
        match crate::memcheck::commit_available_bytes() {
            Some(avail) if avail < ($mb * crate::memcheck::MB) => {
                eprintln!(
                    "[scenario] 跳过：{} 需要约 {}MB commit 可用（当前仅 {:.1}GB）",
                    $name,
                    $mb,
                    avail as f64 / 1073741824.0
                );
                return None;
            }
            _ => {}
        }
    };
}

/// 声明式场景规格：薄壳函数只填差异，其余由 run_scenario 统一处理。
struct ScenarioSpec {
    name: &'static str,
    /// TTS 引擎：决定依赖的模型目录与注册分支
    tts: &'static str,
    targets: Vec<TargetVariant>,
    /// AppConfig 追加定制（模型路径/克隆开关/双音轨等）
    customize: fn(&mut AppConfig),
    /// PipelineConfig 追加定制（分离开关等）
    pipeline_customize: fn(&mut PipelineConfig),
    /// 期望产出最终视频（generate_final_videos）
    expect_final: bool,
    /// 期望最终视频的音轨数（None = 不校验）
    expect_audio_tracks: Option<u32>,
    /// 期望背景分离产物（vocals/bgm）
    expect_separation: bool,
    /// 期望原声克隆参考段（shared/ref_voice.wav）
    expect_voice_ref: bool,
    /// 用自备合成语音素材（10.mp4 语音稀疏仅 0.18s，无法测克隆/变速等路径）
    synthetic_source: bool,
}

impl Default for ScenarioSpec {
    fn default() -> Self {
        Self {
            name: "scenario",
            tts: "supertonic",
            targets: vec![TargetVariant::language("en").unwrap()],
            customize: |_| {},
            pipeline_customize: |_| {},
            expect_final: true,
            expect_audio_tracks: Some(1),
            expect_separation: false,
            expect_voice_ref: false,
            synthetic_source: false,
        }
    }
}

/// 一次完整运行的结果（复用/编辑场景拿它做二次操作）。
pub(crate) struct ScenarioRun {
    pub task_id: crate::domain::ids::TaskId,
    pub task_root: PathBuf,
    pub targets: Vec<TargetVariant>,
    pub store: Arc<crate::infra::task_store::TaskStore>,
    pub app: AppConfig,
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.into())
}

fn test_video() -> PathBuf {
    PathBuf::from(env_or("VT_TEST_VIDEO", r"E:\projects\pyvideotrans-3.98\10.mp4"))
}

fn sensevoice_dir() -> PathBuf {
    PathBuf::from(env_or("VT_SENSEVOICE_DIR", r"E:\projects\test2voices_backup\sense-voice-int8"))
}

fn supertonic_dir() -> PathBuf {
    PathBuf::from(env_or("VT_SUPERTONIC_DIR", r"E:\projects\pyvideotrans-3.98\Supertone\supertonic-3"))
}

fn zipvoice_dir() -> PathBuf {
    PathBuf::from(env_or(
        "VT_ZIPVOICE_DIR",
        r"E:\projects\test2voices_backup\sherpa-onnx-zipvoice-distill-int8-zh-en-emilia",
    ))
}

fn uvr_model() -> PathBuf {
    if let Ok(path) = std::env::var("VT_UVR_MODEL") {
        return PathBuf::from(path);
    }
    let local = std::env::var("LOCALAPPDATA").unwrap_or_default();
    PathBuf::from(local).join("videotrans").join("models").join("UVR-MDX-NET-Inst_HQ_4.onnx")
}

/// 本地 mock OpenAI 兼容翻译服务：请求体里每行 `[idx] text` → `{"translations":[{idx,translated}]}`
/// 包在 choices.message.content 里返回。accept 上限 64 次足够任何分批策略。
fn spawn_mock_translator() -> String {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for _ in 0..64 {
            let Ok((mut stream, _)) = listener.accept() else { return };
            let mut bytes = Vec::new();
            let mut buffer = [0u8; 8192];
            loop {
                let Ok(read) = stream.read(&mut buffer) else { break };
                if read == 0 {
                    break;
                }
                bytes.extend_from_slice(&buffer[..read]);
                if let Some(header_end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&bytes[..header_end]);
                    let length = headers
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length: ")
                                .and_then(|v| v.parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    if bytes.len() >= header_end + 4 + length {
                        break;
                    }
                }
            }
            let body_start = bytes.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
            let Ok(request) = serde_json::from_slice::<serde_json::Value>(&bytes[body_start..]) else {
                continue;
            };
            let user = request["messages"][1]["content"].as_str().unwrap_or("");
            let translations: Vec<_> = user
                .lines()
                .filter_map(|line| {
                    let end = line.find(']')?;
                    let idx = line.get(1..end)?.parse::<usize>().ok()?;
                    Some(serde_json::json!({"idx": idx, "translated": format!("translated-{idx}")}))
                })
                .collect();
            let content = serde_json::json!({"translations": translations}).to_string();
            let body = serde_json::json!({"choices":[{"message":{"content": content}}]}).to_string();
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
        }
    });
    format!("http://{address}/chat")
}

/// ffprobe 取时长（秒）。
async fn ffprobe_duration(path: &std::path::Path) -> f64 {
    let out = tokio::process::Command::new("ffprobe")
        .args(["-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0"])
        .arg(path)
        .output()
        .await
        .expect("ffprobe 执行失败");
    String::from_utf8_lossy(&out.stdout).trim().parse().unwrap_or(0.0)
}

/// ffprobe 数音轨数（codec_type=audio 的流数）。
async fn ffprobe_audio_tracks(path: &std::path::Path) -> u32 {
    let out = tokio::process::Command::new("ffprobe")
        .args(["-v", "error", "-show_entries", "stream=codec_type", "-of", "csv=p=0"])
        .arg(path)
        .output()
        .await
        .expect("ffprobe 执行失败");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|line| line.trim() == "audio")
        .count() as u32
}

struct NoopObserver;
impl PipelineObserver for NoopObserver {
    fn on_stage_update(&self, _update: crate::pipeline::runner::StageUpdate) {}
}

/// 自备合成素材：Supertonic 合成 5s 清晰语音 + 黑底视频（语音稀疏的真实视频测不了克隆/变速）。
async fn synthesized_voice_video(root: &std::path::Path) -> Option<PathBuf> {
    let supertonic = supertonic_dir();
    need_dir!(supertonic, "Supertonic 模型目录（合成素材用）");
    let speech_dir = root.join("speech");
    std::fs::create_dir_all(&speech_dir).unwrap();
    let mut synthesis = AppConfig::default();
    synthesis.supertonic_dir = supertonic.to_string_lossy().into_owned();
    synthesis.supertonic_voice = "M1".into();
    crate::engines::tts::supertonic::synthesize_segments(
        &[crate::types::Segment {
            idx: 0,
            start: 0.0,
            end: 12.0,
            text: String::new(),
            translated: "The watcher stands on the bridge and watches the ships pass under it, while the city lights flicker in the cold night air."
                .into(),
        }],
        "en",
        &synthesis,
        &speech_dir,
        |_| {},
    )
    .unwrap();
    let video = root.join("synthetic-source.mp4");
    let status = tokio::process::Command::new("ffmpeg")
        .args([
            "-v", "error", "-f", "lavfi", "-i", "color=c=black:s=160x90:d=12", "-i",
        ])
        .arg(speech_dir.join("dub.wav"))
        .args(["-shortest", "-c:v", "libx264", "-c:a", "aac", "-y"])
        .arg(&video)
        .status()
        .await
        .unwrap();
    assert!(status.success(), "合成素材视频失败");
    Some(video)
}

/// 共享 runner：就绪预检 → mock 翻译 → 跑完整流水线 → 声明式产物断言。
/// 返回 None 表示环境不满足已跳过；断言失败会 panic（真失败）。
async fn run_scenario(mut spec: ScenarioSpec) -> Option<ScenarioRun> {
    crate::logger::init();
    let root = std::env::temp_dir().join(format!("vt-scenario-{}-{}", spec.name, uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let video = if spec.synthetic_source {
        synthesized_voice_video(&root).await?
    } else {
        let video = test_video();
        need_file!(video, "测试视频");
        video
    };
    let sensevoice = sensevoice_dir();
    need_dir!(sensevoice, "SenseVoice 模型目录");
    let tts_dir = match spec.tts {
        "zipvoice" => {
            let dir = zipvoice_dir();
            need_dir!(dir, "ZipVoice 模型目录");
            dir
        }
        _ => {
            let dir = supertonic_dir();
            need_dir!(dir, "Supertonic 模型目录");
            dir
        }
    };
    if spec.expect_separation {
        let uvr = uvr_model();
        need_file!(uvr, "UVR 分离模型");
    }
    // sensevoice(~1200) + TTS(~1200) + ffmpeg/系统余量
    need_mem!(2500, "场景流水线");

    let mock_url = spawn_mock_translator();
    let service = TaskService::new(root.join("data"));

    let mut config = PipelineConfig {
        source_language: None,
        targets: spec.targets.clone(),
        engines: EngineSelection {
            stt: "sensevoice".into(),
            translator: "openai-compatible".into(),
            tts: spec.tts.into(),
            separator: None,
        },
        separation: SeparationConfig::default(),
        output: OutputConfig {
            generate_final_videos: spec.expect_final,
            ..Default::default()
        },
    };
    (spec.pipeline_customize)(&mut config);

    let mut app = AppConfig::default();
    app.stt_engine = "sensevoice".into();
    app.sensevoice_dir = sensevoice.to_string_lossy().into_owned();
    app.tts_engine = spec.tts.into();
    if spec.tts == "zipvoice" {
        app.zipvoice_dir = zipvoice_dir().to_string_lossy().into_owned();
    } else {
        app.supertonic_dir = tts_dir.to_string_lossy().into_owned();
        app.supertonic_voice = "M1".into();
    }
    app.deepseek_api_url = mock_url;
    app.deepseek_model = "mock".into();
    app.api_interval_ms = 0;
    if spec.expect_separation {
        app.separation_enabled = true;
        app.separator_model_path = uvr_model().to_string_lossy().into_owned();
    }
    (spec.customize)(&mut app);

    let created = service.create_task(&video, config, 1).unwrap();
    let child_tokens = spec
        .targets
        .iter()
        .map(|t| (t.id.clone(), CancelToken::default()))
        .collect();
    let result = run_configured_pipeline(
        app.clone(),
        created.document,
        created.manifest,
        service.store(),
        CancelToken::default(),
        child_tokens,
        Arc::new(NoopObserver),
        Arc::new(|_, _| {}),
    )
    .await
    .unwrap_or_else(|e| panic!("[{}] 流水线失败: {e}", spec.name));
    assert!(
        result.targets.values().all(Result::is_ok),
        "[{}] 存在失败目标: {:?}",
        spec.name,
        result.targets
            .iter()
            .map(|(k, v)| (k.0.clone(), v.as_ref().err().map(|e| e.to_string())))
            .collect::<Vec<_>>()
    );

    let source_duration = ffprobe_duration(&video).await;
    // SourceVariant 命名 = 源文件主干.变体.mp4（10.mp4 + en → 10.en.mp4）
    let source_stem = video.file_stem().unwrap().to_string_lossy().into_owned();
    for target in &spec.targets {
        let dir = created.task_root.join("targets").join(&target.id.0);
        assert!(dir.join("dub.wav").is_file(), "[{}] {} 缺 dub.wav", spec.name, target.id.0);
        let srt_path = dir.join("translated.srt");
        assert!(srt_path.is_file(), "[{}] {} 缺 translated.srt", spec.name, target.id.0);
        let srt = std::fs::read_to_string(&srt_path).unwrap();
        assert!(srt.contains("translated-"), "[{}] {} SRT 不含 mock 译文", spec.name, target.id.0);
        if spec.expect_final {
            let final_path = dir.join(format!("{}.{}.mp4", source_stem, target.id.0));
            assert!(final_path.is_file(), "[{}] {} 缺成片", spec.name, target.id.0);
            let duration = ffprobe_duration(&final_path).await;
            assert!(
                (duration - source_duration).abs() <= 2.0,
                "[{}] {} 成片时长 {}s 偏离源 {}s",
                spec.name,
                target.id.0,
                duration,
                source_duration
            );
            if let Some(expected) = spec.expect_audio_tracks {
                let tracks = ffprobe_audio_tracks(&final_path).await;
                assert_eq!(
                    tracks, expected,
                    "[{}] {} 成片音轨数应为 {}（实际 {}）",
                    spec.name, target.id.0, expected, tracks
                );
            }
        }
    }
    if spec.expect_separation {
        assert!(created.task_root.join("vocals.raw.wav").is_file(), "[{}] 缺 vocals.raw.wav", spec.name);
        assert!(created.task_root.join("bgm.raw.wav").is_file(), "[{}] 缺 bgm.raw.wav", spec.name);
    }
    if spec.expect_voice_ref {
        assert!(
            created.task_root.join("shared").join("ref_voice.wav").is_file(),
            "[{}] 缺原声克隆参考段 shared/ref_voice.wav",
            spec.name
        );
    }

    let _ = spec.name;
    Some(ScenarioRun {
        task_id: created.task_id,
        task_root: created.task_root,
        targets: spec.targets,
        store: service.store(),
        app,
    })
}

// ─────────────────────────── 场景矩阵（加场景：抄一个薄壳改 spec） ───────────────────────────

/// ① 基础闭环：SenseVoice→mock 翻译→Supertonic→成片+SRT（英文目标，全链路最短路径）
#[cfg(feature = "inference")]
#[tokio::test]
#[ignore = "需要本地模型与 10.mp4"]
async fn scenario_basic_supertonic() {
    run_scenario(ScenarioSpec {
        name: "basic_supertonic",
        targets: vec![TargetVariant::language("en").unwrap()],
        ..Default::default()
    })
    .await;
}

/// ② 中文目标闭环：ZipVoice（zh-en 模型）——中文配音主路径
#[cfg(feature = "inference")]
#[tokio::test]
#[ignore = "需要本地模型与 10.mp4"]
async fn scenario_zipvoice_chinese() {
    run_scenario(ScenarioSpec {
        name: "zipvoice_chinese",
        tts: "zipvoice",
        targets: vec![TargetVariant::zh_mandarin()],
        ..Default::default()
    })
    .await;
}

/// ③ 多目标：zh + en 双版本共享一次 STT/探测/提取（ZipVoice 同时支持 zh/en）
#[cfg(feature = "inference")]
#[tokio::test]
#[ignore = "需要本地模型与 10.mp4"]
async fn scenario_multi_target() {
    run_scenario(ScenarioSpec {
        name: "multi_target",
        tts: "zipvoice",
        targets: vec![TargetVariant::zh_mandarin(), TargetVariant::language("en").unwrap()],
        ..Default::default()
    })
    .await;
}

/// ④ 方言目标：粤语版本（variant 展开与 tts_accent 注入路径）
#[cfg(feature = "inference")]
#[tokio::test]
#[ignore = "需要本地模型与 10.mp4"]
async fn scenario_dialect_yue() {
    run_scenario(ScenarioSpec {
        name: "dialect_yue",
        tts: "zipvoice",
        targets: vec![TargetVariant::zh_dialect("yue", "粤语", "请用粤语口语表达。")],
        ..Default::default()
    })
    .await;
}

/// ⑤ 原声克隆：从原视频提取参考段注入 ZipVoice（voice_ref 路径）
/// 用自备合成素材（5s 清晰语音）——10.mp4 仅 0.18s 语音，无合格参考段（3~20s）
#[cfg(feature = "inference")]
#[tokio::test]
#[ignore = "需要本地模型与 10.mp4"]
async fn scenario_voice_clone() {
    run_scenario(ScenarioSpec {
        name: "voice_clone",
        tts: "zipvoice",
        targets: vec![TargetVariant::zh_mandarin()],
        customize: |app| app.tts_use_video_prompt = true,
        expect_voice_ref: true,
        synthetic_source: true,
        ..Default::default()
    })
    .await;
}

/// ⑥ 背景分离 + 混音：UVR 分离 vocals/bgm，配音与背景合成 mixed
#[cfg(feature = "inference")]
#[tokio::test]
#[ignore = "需要本地模型、UVR 模型与 10.mp4"]
async fn scenario_separation_mix() {
    run_scenario(ScenarioSpec {
        name: "separation_mix",
        tts: "zipvoice",
        targets: vec![TargetVariant::zh_mandarin()],
        pipeline_customize: |config| {
            config.separation.enabled = true;
        },
        expect_separation: true,
        ..Default::default()
    })
    .await;
}

/// ⑦ 双音轨：保留原音轨 + 新配音轨（ffprobe 校验成片音轨数 = 2）
#[cfg(feature = "inference")]
#[tokio::test]
#[ignore = "需要本地模型与 10.mp4"]
async fn scenario_dual_track() {
    run_scenario(ScenarioSpec {
        name: "dual_track",
        targets: vec![TargetVariant::language("en").unwrap()],
        customize: |app| app.keep_original_audio_track = true,
        pipeline_customize: |config| {
            config.output.keep_original_audio_track = true;
        },
        expect_audio_tracks: Some(2),
        ..Default::default()
    })
    .await;
}

/// ⑧ 断点复用：完整跑完后对同一任务二次运行——依赖哈希命中全 Reused，产物 mtime 不变
#[cfg(feature = "inference")]
#[tokio::test]
#[ignore = "需要本地模型与 10.mp4"]
async fn scenario_resume_reuse() {
    let Some(run) = run_scenario(ScenarioSpec {
        name: "resume_reuse",
        tts: "zipvoice",
        targets: vec![TargetVariant::zh_mandarin()],
        ..Default::default()
    })
    .await
    else {
        return;
    };
    let dub = run.task_root.join("targets").join("zh-CN").join("dub.wav");
    let mtime_before = std::fs::metadata(&dub).unwrap().modified().unwrap();
    let loaded = run.store.load_bundle(&run.task_id).unwrap();
    let child_tokens = run
        .targets
        .iter()
        .map(|t| (t.id.clone(), CancelToken::default()))
        .collect();
    let second = run_configured_pipeline(
        run.app.clone(),
        loaded.task,
        loaded.manifest,
        run.store.clone(),
        CancelToken::default(),
        child_tokens,
        Arc::new(NoopObserver),
        Arc::new(|_, _| {}),
    )
    .await
    .unwrap();
    assert!(second.targets.values().all(Result::is_ok), "二次运行失败");
    let mtime_after = std::fs::metadata(&dub).unwrap().modified().unwrap();
    assert_eq!(
        mtime_before, mtime_after,
        "二次运行不应重跑 TTS（dub.wav 被重写）"
    );
}

/// ⑨ 字幕编辑触发下游重跑：改译文后二次运行，dub.wav 必须重新生成（mtime 变化）
#[cfg(feature = "inference")]
#[tokio::test]
#[ignore = "需要本地模型与 10.mp4"]
async fn scenario_subtitle_edit_rerun() {
    let Some(run) = run_scenario(ScenarioSpec {
        name: "subtitle_edit_rerun",
        tts: "zipvoice",
        targets: vec![TargetVariant::zh_mandarin()],
        ..Default::default()
    })
    .await
    else {
        return;
    };
    let variant_id = run.targets[0].id.clone();
    let mut segments =
        crate::application::subtitle_edit::load_segments(&run.store, &run.task_id, Some(&variant_id))
            .unwrap();
    assert!(!segments.is_empty(), "编辑前应已有字幕段");
    segments[0].translated = format!("{}-edited", segments[0].translated);
    crate::application::subtitle_edit::save_segments(&run.store, &run.task_id, Some(&variant_id), &segments)
        .unwrap();

    let dub = run.task_root.join("targets").join(&variant_id.0).join("dub.wav");
    let mtime_before = std::fs::metadata(&dub).unwrap().modified().unwrap();
    let loaded = run.store.load_bundle(&run.task_id).unwrap();
    let child_tokens = run
        .targets
        .iter()
        .map(|t| (t.id.clone(), CancelToken::default()))
        .collect();
    run_configured_pipeline(
        run.app.clone(),
        loaded.task,
        loaded.manifest,
        run.store.clone(),
        CancelToken::default(),
        child_tokens,
        Arc::new(NoopObserver),
        Arc::new(|_, _| {}),
    )
    .await
    .unwrap();
    let mtime_after = std::fs::metadata(&dub).unwrap().modified().unwrap();
    assert_ne!(
        mtime_before, mtime_after,
        "编辑译文后下游（tts）必须重跑，dub.wav 应被重写"
    );
}
