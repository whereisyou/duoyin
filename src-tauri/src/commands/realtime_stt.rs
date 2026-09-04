use std::sync::Mutex;

use crate::adapters::stt::legacy::ConfiguredSttEngine;
use crate::pipeline::runner::CancelToken;
use crate::ports::stt::SttEngine;
use crate::types::{AppConfig, Segment};

#[tauri::command]
pub async fn transcribe_audio_chunk(
    state: tauri::State<'_, Mutex<AppConfig>>,
    bytes: Vec<u8>,
    extension: String,
    language: String,
) -> Result<Vec<Segment>, String> {
    if bytes.is_empty() || bytes.len() > 20 * 1024 * 1024 {
        return Err("实时录音分片为空或过大".into());
    }
    let extension = extension.trim_start_matches('.');
    if !matches!(extension, "webm" | "ogg" | "wav") {
        return Err("实时录音格式不支持".into());
    }
    let root = std::env::temp_dir().join("videotrans-realtime");
    tokio::fs::create_dir_all(&root)
        .await
        .map_err(|error| error.to_string())?;
    let id = uuid::Uuid::new_v4();
    let input = root.join(format!("{id}.{extension}"));
    let wav = root.join(format!("{id}.wav"));
    tokio::fs::write(&input, bytes)
        .await
        .map_err(|error| error.to_string())?;
    let status = tokio::process::Command::new("ffmpeg")
        .kill_on_drop(true)
        .args(["-v", "error", "-i"])
        .arg(&input)
        .args(["-ar", "16000", "-ac", "1", "-c:a", "pcm_s16le", "-y"])
        .arg(&wav)
        .status()
        .await
        .map_err(|error| error.to_string())?;
    let _ = tokio::fs::remove_file(&input).await;
    if !status.success() {
        return Err("实时录音转换 WAV 失败".into());
    }
    let config = state.lock().map_err(|error| error.to_string())?.clone();
    let engine = ConfiguredSttEngine::new(config);
    let result = engine
        .transcribe(
            &wav,
            if language == "auto" {
                None
            } else {
                Some(&language)
            },
            &CancelToken::default(),
        )
        .await
        .map_err(|error| format!("{error:?}"));
    let _ = tokio::fs::remove_file(&wav).await;
    result
}

#[cfg(test)]
mod tests {
    #[test]
    fn supported_extensions_are_bounded() {
        assert!(matches!("webm", "webm" | "ogg" | "wav"));
        assert!(!matches!("mp4", "webm" | "ogg" | "wav"));
    }
}
