use std::path::Path;

use serde::Serialize;

use crate::types::AppConfig;

#[derive(Debug, Clone, Serialize)]
pub struct RuntimeModelStatus {
    pub id: String,
    pub ready: bool,
    pub path: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct RuntimeInfo {
    pub gpu: Option<String>,
    pub models: Vec<RuntimeModelStatus>,
}

#[tauri::command]
pub fn get_runtime_info(
    state: tauri::State<'_, std::sync::Mutex<AppConfig>>,
) -> Result<RuntimeInfo, String> {
    let config = state.lock().map_err(|error| error.to_string())?.clone();
    let gpu = std::process::Command::new("nvidia-smi")
        .args([
            "--query-gpu=name,memory.total",
            "--format=csv,noheader,nounits",
        ])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    let models = vec![
        directory_model(
            "sensevoice",
            &config.sensevoice_dir,
            &["model.int8.onnx", "tokens.txt"],
        ),
        directory_model(
            "whisper_native",
            &config.whisper_model_dir,
            &["config.json", "model.safetensors", "tokenizer.json"],
        ),
        directory_model(
            "supertonic",
            &config.supertonic_dir,
            &["onnx", "voice_styles"],
        ),
        RuntimeModelStatus {
            id: "supertonic_base".into(),
            ready: supertonic_base_ready(&config.supertonic_dir),
            path: config.supertonic_dir.clone(),
            bytes: 0,
        },
        RuntimeModelStatus {
            id: "supertonic_zh".into(),
            ready: supertonic_zh_ready(&config.supertonic_dir),
            path: config.supertonic_dir.clone(),
            bytes: 0,
        },
        directory_model(
            "zipvoice",
            &config.zipvoice_dir,
            &[
                "encoder.int8.onnx",
                "decoder.int8.onnx",
                "tokens.txt",
                "lexicon.txt",
                "vocos_24khz.onnx",
                "espeak-ng-data",
            ],
        ),
        file_model("uvr_mdx", &config.separator_model_path),
        file_model("speaker_segmentation", &config.diarization_seg_model),
        file_model("speaker_embedding", &config.diarization_embedding_model),
    ];
    Ok(RuntimeInfo { gpu, models })
}

#[cfg(feature = "inference")]
fn supertonic_base_ready(dir: &str) -> bool {
    crate::engines::tts::supertonic::official_available(dir)
}

#[cfg(not(feature = "inference"))]
fn supertonic_base_ready(_dir: &str) -> bool {
    false
}

#[cfg(feature = "inference")]
fn supertonic_zh_ready(dir: &str) -> bool {
    crate::engines::tts::supertonic::zh_available(dir)
}

#[cfg(not(feature = "inference"))]
fn supertonic_zh_ready(_dir: &str) -> bool {
    false
}

fn file_model(id: &str, value: &str) -> RuntimeModelStatus {
    let path = Path::new(value);
    let bytes = path.metadata().map(|metadata| metadata.len()).unwrap_or(0);
    RuntimeModelStatus {
        id: id.into(),
        ready: path.is_file() && bytes > 0,
        path: value.into(),
        bytes,
    }
}

fn directory_model(id: &str, value: &str, required: &[&str]) -> RuntimeModelStatus {
    let path = Path::new(value);
    let ready = path.is_dir() && required.iter().all(|name| path.join(name).exists());
    RuntimeModelStatus {
        id: id.into(),
        ready,
        path: value.into(),
        bytes: if path.is_dir() {
            directory_size(path)
        } else {
            0
        },
    }
}

fn directory_size(path: &Path) -> u64 {
    std::fs::read_dir(path)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| {
            entry
                .metadata()
                .map(|metadata| {
                    if metadata.is_dir() {
                        directory_size(&entry.path())
                    } else {
                        metadata.len()
                    }
                })
                .unwrap_or(0)
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_model_is_not_ready() {
        assert!(!file_model("x", "definitely-missing.onnx").ready);
    }
}
