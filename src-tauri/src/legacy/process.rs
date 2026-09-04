use crate::types::{AppConfig, ProgressEvent, TaskConfig};
use std::path::PathBuf;
use std::sync::Arc;

type Emit = Arc<dyn Fn(ProgressEvent) + Send + Sync>;

fn running(step: &str, progress: u8) -> ProgressEvent {
    ProgressEvent {
        step: step.into(),
        progress,
        status: "running".into(),
        error: None,
        segments: None,
        output_dir: None,
        scope: None,
        variant_id: None,
        parent_status: None,
    }
}

fn failed(step: &str, e: String) -> ProgressEvent {
    ProgressEvent {
        step: step.into(),
        progress: 0,
        status: "error".into(),
        error: Some(e),
        segments: None,
        output_dir: None,
        scope: None,
        variant_id: None,
        parent_status: None,
    }
}

/// 在阻塞线程池执行 CPU 密集任务，进度回调经通道转发为事件
async fn run_blocking<T, F>(step: &str, emit: &Emit, work: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce(Box<dyn Fn(u8) + Send>) -> Result<T, String> + Send + 'static,
{
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<u8>();
    let emit_c = emit.clone();
    let step_c = step.to_string();
    let fwd = tokio::spawn(async move {
        while let Some(p) = rx.recv().await {
            emit_c(running(&step_c, p.min(99)));
        }
    });
    let res = tokio::task::spawn_blocking(move || {
        work(Box::new(move |p| {
            let _ = tx.send(p);
        }))
    })
    .await
    .map_err(|e| e.to_string())?;
    let _ = fwd.await;
    res
}

/// 执行完整流程：提取音频 → STT → 翻译 → 合成配音 → 切音频 → 写SRT
pub async fn run(cfg: &AppConfig, task: &TaskConfig, work: &PathBuf, out: &PathBuf, emit: Emit) {
    // 第1步：提取音频
    emit(running("extract_audio", 0));
    let audio = work.join("audio.wav");
    if let Err(e) = super::ffmpeg::extract_audio(&task.video.as_ref(), &audio).await {
        emit(failed("extract_audio", e));
        return;
    }
    emit(running("extract_audio", 100));

    // 第2步：语音识别（按引擎分发）
    emit(running("stt", 0));
    let result = match cfg.stt_engine.as_str() {
        "openai_api" => {
            let _api =
                crate::scheduler::admit_api(cfg.api_max_concurrent, cfg.api_interval_ms).await;
            crate::engines::stt::openai_api::transcribe(&audio, &task.source_lang, &cfg.openai_key).await
        }
        "whisper_local" => {
            crate::engines::stt::whisper_cli::transcribe(&audio, &task.source_lang, cfg).await
        }
        "sensevoice" => {
            #[cfg(feature = "inference")]
            {
                // 调度器准入：成本由引擎决定（sensevoice 仅 1.2GB，candle 要 3.9GB），
                // 内存不够时在这里退避等待而不是跑到一半 abort
                let _lease = crate::scheduler::admit(crate::scheduler::stt(&cfg.stt_engine)).await;
                let cfg_c = cfg.clone();
                let lang_c = task.source_lang.clone();
                let audio_c = audio.clone();
                run_blocking("stt", &emit, move |progress| {
                    crate::engines::stt::sensevoice::transcribe(&audio_c, &lang_c, &cfg_c, progress)
                })
                .await
            }
            #[cfg(not(feature = "inference"))]
            {
                emit(failed(
                    "stt",
                    "推理功能未启用，请用 --features inference 构建".into(),
                ));
                return;
            }
        }
        _ => {
            #[cfg(feature = "inference")]
            {
                // 调度器准入：成本由引擎决定（candle 3.9GB / sensevoice 1.2GB），
                // 内存不够时在这里退避等待而不是跑到一半 abort
                let _lease = crate::scheduler::admit(crate::scheduler::stt(&cfg.stt_engine)).await;
                let cfg_c = cfg.clone();
                let lang_c = task.source_lang.clone();
                let audio_c = audio.clone();
                run_blocking("stt", &emit, move |progress| {
                    crate::engines::stt::whisper_native::transcribe(&audio_c, &lang_c, &cfg_c, progress)
                })
                .await
            }
            #[cfg(not(feature = "inference"))]
            {
                emit(failed(
                    "stt",
                    "推理功能未启用，请用 --features inference 构建".into(),
                ));
                return;
            }
        }
    };
    let segments = match result {
        Ok(s) => crate::segments::sanitize(s),
        Err(e) => {
            emit(failed("stt", e));
            return;
        }
    };
    if segments.is_empty() {
        emit(failed("stt", "识别结果为空或时间戳全部无效".into()));
        return;
    }
    let _ = tokio::fs::write(
        work.join("segments.json"),
        serde_json::to_string_pretty(&segments).unwrap(),
    )
    .await;
    emit(running("stt", 100));

    // 第3步：翻译
    emit(running("translate", 0));
    let _api = crate::scheduler::admit_api(cfg.api_max_concurrent, cfg.api_interval_ms).await;
    let translated = match crate::engines::translate::deepseek::translate(
        &segments,
        &task.source_lang,
        &task.target_lang,
        &cfg.deepseek_key,
        &cfg.deepseek_model,
        &cfg.deepseek_api_url,
    )
    .await
    {
        Ok(s) => s,
        Err(e) => {
            emit(failed("translate", e));
            return;
        }
    };
    let _ = tokio::fs::write(
        work.join("translated.json"),
        serde_json::to_string_pretty(&translated).unwrap(),
    )
    .await;
    emit(running("translate", 100));

    // 第4步：合成配音（Supertonic 本地 TTS；未配置或目标语言不支持则跳过）
    emit(running("tts", 0));
    #[cfg(feature = "inference")]
    {
        let tts_ready = cfg.tts_engine == "supertonic"
            && !cfg.supertonic_dir.trim().is_empty()
            && crate::engines::tts::supertonic::lang_supported(&cfg.supertonic_dir, &task.target_lang);
        if !tts_ready {
            emit(failed(
                "tts",
                format!(
                    "TTS 引擎未就绪或不支持目标语言 {}，无法生成最终视频",
                    task.target_lang
                ),
            ));
            return;
        }
        if tts_ready {
            // 调度器准入：TTS 与 STT 互斥（ONNX 会话 + 波形缓冲都是内存大户）
            let _lease = crate::scheduler::admit(crate::scheduler::TTS).await;
            let cfg_c = cfg.clone();
            let lang_c = task.target_lang.clone();
            let out_c = out.clone();
            let segments_c = translated.clone();
            if let Err(e) = run_blocking("tts", &emit, move |progress| {
                crate::engines::tts::supertonic::synthesize_segments(
                    &segments_c,
                    &lang_c,
                    &cfg_c,
                    &out_c,
                    progress,
                )
            })
            .await
            {
                emit(failed("tts", e));
                return;
            }
        }
    }
    let dub = out.join("dub.wav");
    if !dub.is_file() {
        emit(failed(
            "tts",
            format!("TTS 未生成必要配音文件：{}", dub.display()),
        ));
        return;
    }
    emit(running("tts", 100));

    // 第5步：切割音频片段
    emit(running("split_audio", 0));
    let seg_dir = out.join("audio_segments");
    if let Err(e) = super::ffmpeg::split_audio(&audio, &translated, &seg_dir).await {
        emit(failed("split_audio", e));
        return;
    }
    let _ = tokio::fs::write(
        out.join("segments.json"),
        serde_json::to_string_pretty(&translated).unwrap(),
    )
    .await;
    emit(running("split_audio", 100));

    // 第6步：写 SRT 字幕
    let srt = out.join("translated.srt");
    if let Err(e) = crate::subtitle::write_srt(&translated, &srt).await {
        emit(failed("srt", e));
        return;
    }
    emit(running("srt", 100));

    // 第7步：源视频 + 新配音重新合成最终视频（不覆盖源文件）
    emit(running("final_video", 0));
    let final_video = out.join("final.mp4");
    if let Err(e) =
        super::ffmpeg::mux_replaced_audio(PathBuf::from(&task.video).as_path(), &dub, &final_video)
            .await
    {
        emit(failed("final_video", e));
        return;
    }
    if !final_video.is_file() {
        emit(failed(
            "final_video",
            format!("视频合成未生成最终文件：{}", final_video.display()),
        ));
        return;
    }
    emit(running("final_video", 100));

    // 只有最终视频存在才算完成 — 字幕段与输出目录一并发给前端
    emit(ProgressEvent {
        step: "done".into(),
        progress: 100,
        status: "done".into(),
        error: None,
        segments: Some(translated),
        output_dir: Some(out.to_string_lossy().to_string()),
        scope: None,
        variant_id: None,
        parent_status: None,
    });
}
