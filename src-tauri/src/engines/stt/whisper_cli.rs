use crate::types::{AppConfig, Segment};
use std::path::Path;
use tokio::process::Command;

/// 调用本地 whisper.cpp CLI 离线识别语音
///
/// 输入音频已由 ffmpeg 步骤统一为 16kHz 单声道 WAV（whisper.cpp 的要求）。
/// JSON 输出结构：{"transcription":[{"offsets":{"from":ms,"to":ms},"text":"..."}]}
pub async fn transcribe(audio: &Path, lang: &str, cfg: &AppConfig) -> Result<Vec<Segment>, String> {
    if cfg.whisper_cli_path.trim().is_empty() {
        return Err("未配置 whisper-cli，请在 设置 → 语音识别 中选择可执行文件".into());
    }
    if cfg.whisper_model_path.trim().is_empty() {
        return Err("未配置 Whisper 模型，请在 设置 → 语音识别 中选择 ggml 模型文件".into());
    }

    let work = audio.parent().ok_or("invalid audio path")?;
    let out_prefix = work.join("whisper_result");

    let mut cmd = Command::new(&cfg.whisper_cli_path);
    cmd.arg("-m")
        .arg(&cfg.whisper_model_path)
        .arg("-f")
        .arg(audio)
        .arg("-l")
        .arg(lang)
        .arg("-oj") // 输出 JSON
        .arg("-of")
        .arg(&out_prefix)
        .arg("-np"); // 不在 stdout 打印进度
    if !cfg.whisper_use_gpu {
        cmd.arg("-ng"); // CPU 版禁用 GPU（v1.5+ 支持该参数）
    }

    let status = cmd
        .status()
        .await
        .map_err(|e| format!("启动 whisper-cli 失败: {}", e))?;
    if !status.success() {
        return Err("whisper-cli 执行失败，请检查可执行文件与模型路径是否正确匹配".into());
    }

    let data = tokio::fs::read_to_string(out_prefix.with_extension("json"))
        .await
        .map_err(|e| format!("读取识别结果失败: {}", e))?;
    let v: serde_json::Value = serde_json::from_str(&data).map_err(|e| e.to_string())?;
    let arr = v["transcription"]
        .as_array()
        .ok_or("识别结果格式异常（缺少 transcription 字段）")?;

    arr.iter()
        .enumerate()
        .map(|(i, s)| {
            Ok(Segment {
                idx: i,
                start: s["offsets"]["from"]
                    .as_f64()
                    .ok_or("missing offsets.from")?
                    / 1000.0,
                end: s["offsets"]["to"].as_f64().ok_or("missing offsets.to")? / 1000.0,
                text: s["text"].as_str().unwrap_or("").trim().to_string(),
                translated: String::new(),
            })
        })
        .collect()
}
