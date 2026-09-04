//! ZipVoice Distill INT8 本地零样本 TTS（sherpa-onnx `OfflineTts`，无 Python/外部进程）。
//!
//! 模型目录约定（`zipvoice_dir`）：encoder.int8.onnx + decoder.int8.onnx + tokens.txt +
//! lexicon.txt + espeak-ng-data/ + vocos_24khz.onnx（ vocoder 独立下载，见 register_tts 注释）。
//! 参考音频（`zipvoice_prompt_wav`）+ 逐字转写（`zipvoice_prompt_text`）做零样本音色克隆，
//! 转写必须与音频内容一致，否则克隆失败。
//!
//! 资源与生命周期：走 `scheduler::TTS` 重资源串行；RAII 租约在 TtsStageExecutor 获取后，
//! 本引擎在 synthesize 内加载一次模型、同目标全部字幕段复用——不做全局缓存
//! （`OfflineTts` 非 Clone，全局缓存必然引入 unsafe 或专属 worker，超出本轮范围）。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::domain::variant::TargetVariant;
use crate::pipeline::runner::CancelToken;
use crate::ports::tts::{
    validate_tts_input, TtsAlignment, TtsEngine, TtsError, TtsFuture, TtsOutput,
};
use crate::types::Segment;

/// 必需文件/目录；缺失会在任务启动前的 preflight 报错（不走昂贵 STT）。
const REQUIRED: [&str; 6] = [
    "encoder.int8.onnx",
    "decoder.int8.onnx",
    "tokens.txt",
    "lexicon.txt",
    "vocos_24khz.onnx",
    "espeak-ng-data",
];

/// 返回模型目录缺失的必需项；空列表表示可用。
pub fn missing_files(dir: &str) -> Vec<&'static str> {
    let root = Path::new(dir);
    REQUIRED
        .iter()
        .copied()
        .filter(|name| !root.join(name).exists())
        .collect()
}

/// preflight 校验：模型目录齐备返回 Ok，否则列出缺失项。
pub fn validate(dir: &str) -> Result<(), String> {
    let missing = missing_files(dir);
    if missing.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "ZipVoice 模型目录缺少文件：{}（应含 encoder/decoder.int8.onnx、tokens.txt、lexicon.txt、vocos_24khz.onnx、espeak-ng-data/）",
            missing.join("、")
        ))
    }
}

#[derive(Debug, Clone)]
pub struct ZipVoiceEngine {
    dir: String,
    prompt_wav: String,
    prompt_text: String,
    num_threads: i32,
    /// 任务级参考音色（with_task_reference 注入；合成时优先于全局 prompt）
    task_reference: Arc<std::sync::RwLock<Option<(PathBuf, String)>>>,
}

impl ZipVoiceEngine {
    pub fn new(
        dir: impl Into<String>,
        prompt_wav: impl Into<String>,
        prompt_text: impl Into<String>,
        num_threads: i32,
    ) -> Result<Self, TtsError> {
        let dir = dir.into();
        if dir.trim().is_empty() {
            return Err(TtsError::InvalidInput(
                "未配置 ZipVoice 模型目录，请在 设置 → 语音合成 中选择".into(),
            ));
        }
        let prompt_wav = prompt_wav.into();
        if prompt_wav.trim().is_empty() {
            return Err(TtsError::InvalidInput(
                "ZipVoice 零样本克隆需要参考音频（至少 3 秒 WAV）".into(),
            ));
        }
        let prompt_text = prompt_text.into();
        if prompt_text.trim().is_empty() {
            return Err(TtsError::InvalidInput(
                "ZipVoice 需要参考音频对应文本（逐字转写，须与音频一致）".into(),
            ));
        }
        Ok(Self {
            dir,
            prompt_wav,
            prompt_text,
            num_threads: num_threads.max(1),
            task_reference: Arc::new(std::sync::RwLock::new(None)),
        })
    }

    /// 实际参考输入：任务级覆盖优先（with_task_reference），否则全局配置。
    fn resolved_prompt(&self) -> (String, String) {
        if let Ok(guard) = self.task_reference.read() {
            if let Some((wav, text)) = guard.as_ref() {
                return (wav.to_string_lossy().into_owned(), text.clone());
            }
        }
        (self.prompt_wav.clone(), self.prompt_text.clone())
    }

    /// 阻塞全流程：读参考音频 → 加载一次模型 → 逐段生成 → 对齐 → 贴时间轴写 dub.wav。
    /// 在 spawn_blocking 中执行（模型加载与推理都是 CPU 密集）。
    fn synthesize_blocking(
        &self,
        segments: &[Segment],
        output_dir: &Path,
        alignment: TtsAlignment,
        cancel: &CancelToken,
    ) -> Result<TtsOutput, TtsError> {
        let (ref_wav, ref_text) = self.resolved_prompt();
        let wave = sherpa_onnx::Wave::read(&ref_wav).ok_or_else(|| {
            TtsError::InvalidInput(format!("读取参考音频失败: {}", ref_wav))
        })?;
        let reference_audio: Vec<f32> = wave.samples().to_vec();
        let reference_sample_rate = wave.sample_rate();
        if reference_audio.is_empty() {
            return Err(TtsError::InvalidInput("参考音频为空或无法解析".into()));
        }

        let dir = Path::new(&self.dir);
        let join = |name: &str| dir.join(name).to_string_lossy().into_owned();
        let config = sherpa_onnx::OfflineTtsConfig {
            model: sherpa_onnx::OfflineTtsModelConfig {
                zipvoice: sherpa_onnx::OfflineTtsZipvoiceModelConfig {
                    tokens: Some(join("tokens.txt")),
                    encoder: Some(join("encoder.int8.onnx")),
                    decoder: Some(join("decoder.int8.onnx")),
                    vocoder: Some(join("vocos_24khz.onnx")),
                    data_dir: Some(join("espeak-ng-data")),
                    lexicon: Some(join("lexicon.txt")),
                    // feat_scale/t_shift/target_rms/guidance_scale 置 0（官方示例 memset 同款默认）
                    ..Default::default()
                },
                num_threads: self.num_threads,
                ..Default::default()
            },
            ..Default::default()
        };
        // 模型加载一次，同目标全部段复用（OfflineTts 非 Clone，故不做全局缓存）
        let tts = sherpa_onnx::OfflineTts::create(&config).ok_or_else(|| {
            TtsError::Engine("ZipVoice 模型加载失败（检查模型目录与 vocos_24khz.onnx）".into())
        })?;
        let sample_rate = tts.sample_rate() as u32;

        // dub 组装与对齐收敛到共享 tts_dub（与旧实现逐字节等价）
        let mut timeline = crate::tts_dub::TimelineWriter::new(output_dir, sample_rate)
            .map_err(TtsError::Engine)?;

        for segment in segments {
            if cancel.is_canceled() {
                return Err(TtsError::Canceled);
            }
            let text = segment.translated.trim();
            if text.is_empty() {
                continue;
            }
            let cancel_cb = cancel.clone();
            let generation = sherpa_onnx::GenerationConfig {
                reference_audio: Some(reference_audio.clone()),
                reference_sample_rate,
                reference_text: Some(ref_text.clone()),
                num_steps: 4,
                ..Default::default()
            };
            // 进度回调返回 false 请求中断（取消时丢弃当前段、保留已写段）
            let audio = tts
                .generate_with_config(text, &generation, Some(move |_: &[f32], _: f32| {
                    !cancel_cb.is_canceled()
                }))
                .ok_or_else(|| {
                    TtsError::Engine(format!("ZipVoice 合成失败（第 {} 段）", segment.idx + 1))
                })?;
            if cancel.is_canceled() {
                return Err(TtsError::Canceled);
            }
            let samples: Vec<i16> = audio.samples().iter().map(|v| crate::tts_dub::to_i16(*v)).collect();

            // 超长段按 rubberband 限速对齐到字幕时长（共享实现）
            let target_duration = (segment.end - segment.start).max(0.0);
            let actual_duration = samples.len() as f64 / sample_rate as f64;
            let samples = crate::tts_dub::align_i16_to_duration(
                samples,
                actual_duration,
                target_duration,
                alignment.max_speed_percent,
                sample_rate,
                output_dir,
                segment.idx,
            )
            .map_err(TtsError::Engine)?;
            timeline.push(segment.start, &samples).map_err(TtsError::Engine)?;
        }
        let dub_path = timeline.finalize().map_err(TtsError::Engine)?;
        Ok(TtsOutput {
            dub_audio: dub_path,
            segment_dir: None,
        })
    }
}

impl TtsEngine for ZipVoiceEngine {
    fn version(&self) -> String {
        "zipvoice-distill-int8-zh-en-emilia".into()
    }

    fn resource_cost(&self) -> crate::scheduler::ResourceCost {
        // 与 supertonic 同级的本地 TTS 重资源：CPU 串行 + commit 预审（防 OOM）
        crate::scheduler::TTS.into()
    }

    fn with_task_reference(&self, wav: &Path, text: &str) {
        if let Ok(mut guard) = self.task_reference.write() {
            *guard = Some((wav.to_path_buf(), text.to_string()));
        }
    }

    fn synthesize<'a>(
        &'a self,
        segments: &'a [Segment],
        target: &'a TargetVariant,
        output_dir: &'a Path,
        alignment: TtsAlignment,
        cancel: &'a CancelToken,
    ) -> TtsFuture<'a> {
        Box::pin(async move {
            let _ = target; // 零样本克隆的音色来自参考音频，与目标语言/方言无关
            validate_tts_input(segments)?;
            // preflight：模型目录与参考输入不齐时，在昂贵的模型加载前报错
            validate(&self.dir).map_err(TtsError::InvalidInput)?;
            let (ref_wav, _) = self.resolved_prompt();
            if !Path::new(&ref_wav).is_file() {
                return Err(TtsError::InvalidInput(format!(
                    "ZipVoice 参考音频不存在: {}",
                    ref_wav
                )));
            }

            let segments = segments.to_vec();
            let output_dir = output_dir.to_path_buf();
            let cancel = cancel.clone();
            let engine = self.clone();
            tokio::task::spawn_blocking(move || {
                engine.synthesize_blocking(&segments, &output_dir, alignment, &cancel)
            })
            .await
            .map_err(|error| TtsError::Engine(format!("ZipVoice 推理线程失败: {error}")))?
        })
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    fn touch_file(root: &Path, name: &str) {
        let path = root.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"").unwrap();
    }

    #[test]
    fn missing_files_lists_everything_for_empty_dir() {
        let missing = missing_files("definitely-not-a-zipvoice-dir");
        assert_eq!(missing.len(), REQUIRED.len());
    }

    #[test]
    fn missing_files_empty_when_all_present() {
        let root = std::env::temp_dir().join(format!("zipvoice-assets-{}", uuid::Uuid::new_v4()));
        for name in ["encoder.int8.onnx", "decoder.int8.onnx", "tokens.txt", "lexicon.txt", "vocos_24khz.onnx"] {
            touch_file(&root, name);
        }
        std::fs::create_dir_all(root.join("espeak-ng-data")).unwrap();
        let dir = root.to_string_lossy().into_owned();
        assert!(missing_files(&dir).is_empty());
        assert!(validate(&dir).is_ok());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn validate_reports_specific_missing() {
        let root = std::env::temp_dir().join(format!("zipvoice-missing-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        // 只放 tokens.txt，其余应列入缺失
        touch_file(&root, "tokens.txt");
        let error = validate(&root.to_string_lossy()).unwrap_err();
        assert!(error.contains("vocos_24khz.onnx"));
        assert!(error.contains("encoder.int8.onnx"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn new_rejects_empty_inputs() {
        assert!(ZipVoiceEngine::new("", "wav", "text", 2).is_err());
        assert!(ZipVoiceEngine::new("dir", "", "text", 2).is_err());
        assert!(ZipVoiceEngine::new("dir", "wav", "", 2).is_err());
        assert!(ZipVoiceEngine::new("dir", "wav", "text", 2).is_ok());
    }

    #[tokio::test]
    async fn synthesize_fails_fast_on_missing_assets_before_model_load() {
        // 模型目录缺失时，应在昂贵的模型加载前报 preflight 错（不触发 ONNX 加载）
        let engine = ZipVoiceEngine::new("missing-zipvoice-dir", "missing-prompt.wav", "你好", 2)
            .unwrap();
        let result = engine
            .synthesize(
                &[Segment {
                    idx: 0,
                    start: 0.0,
                    end: 1.0,
                    text: "你好".into(),
                    translated: "你好".into(),
                }],
                &TargetVariant::zh_mandarin(),
                Path::new("out"),
                TtsAlignment {
                    min_speed_percent: 85,
                    max_speed_percent: 125,
                },
                &CancelToken::default(),
            )
            .await;
        assert!(matches!(result, Err(TtsError::InvalidInput(_))));
    }

    /// 真实模型 e2e：需本地 ZipVoice 模型（VT_ZIPVOICE_DIR 可覆盖路径）。
    /// 运行：`cargo test --features inference -- --ignored zipvoice --nocapture`
    /// 验证 vocoder(vocos_24khz) 完整加载且真正合成出声（非空音频）。
    #[tokio::test]
    #[ignore]
    async fn real_model_clones_reference_voice() {
        let dir = std::env::var("VT_ZIPVOICE_DIR").unwrap_or_else(|_| {
            r"E:\projects\text2voices\sherpa-onnx-zipvoice-distill-int8-zh-en-emilia".into()
        });
        if validate(&dir).is_err() {
            eprintln!("[zipvoice e2e] 模型目录不齐，跳过: {dir}");
            return;
        }
        let prompt_wav = Path::new(&dir).join("test_wavs/leijun-1.wav");
        let engine = ZipVoiceEngine::new(
            &dir,
            prompt_wav.to_string_lossy(),
            "那还是36年前, 1987年. 我呢考上了武汉大学的计算机系.",
            2,
        )
        .unwrap();
        let out = std::env::temp_dir().join(format!("zipvoice-e2e-{}", uuid::Uuid::new_v4()));
        let output = engine
            .synthesize(
                &[Segment {
                    idx: 0,
                    start: 0.0,
                    end: 12.0,
                    text: "大家好".into(),
                    translated: "大家好, 这是一个零样本语音合成测试.".into(),
                }],
                &TargetVariant::zh_mandarin(),
                &out,
                TtsAlignment {
                    min_speed_percent: 85,
                    max_speed_percent: 125,
                },
                &CancelToken::default(),
            )
            .await
            .unwrap();
        let reader = hound::WavReader::open(&output.dub_audio).unwrap();
        assert_eq!(reader.spec().sample_rate, 24000);
        let count = reader.into_samples::<i16>().count();
        assert!(count > 24000, "合成音频过短（{count} 采样），疑似未真正出声");
        std::fs::remove_dir_all(out).unwrap();
    }
}
