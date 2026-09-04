//! 配置与系统级命令：应用配置读写、文件选择、文本读写、路径打开、日志。

use std::sync::Mutex;

use crate::logger;
use crate::types::AppConfig;

/// 检查 ffmpeg 是否可用
#[tauri::command]
pub fn check_ffmpeg() -> Result<String, String> {
    let out = std::process::Command::new("ffmpeg")
        .arg("-version")
        .output()
        .map_err(|e| format!("ffmpeg not found: {}", e))?;
    if out.status.success() {
        let version = String::from_utf8_lossy(&out.stdout)
            .lines()
            .next()
            .unwrap_or("unknown")
            .to_string();
        Ok(version)
    } else {
        Err("ffmpeg not available".into())
    }
}

#[tauri::command]
pub fn pick_onnx_model() -> Result<Option<String>, String> {
    Ok(rfd::FileDialog::new()
        .add_filter("ONNX Model", &["onnx"])
        .pick_file()
        .map(|path| path.to_string_lossy().into_owned()))
}

/// 打开文件选择对话框（支持多选）
#[tauri::command]
pub fn pick_video_files() -> Result<Vec<String>, String> {
    let files = rfd::FileDialog::new()
        .add_filter("Video", &["mp4", "mkv", "avi", "mov", "wmv", "flv"])
        .pick_files();
    Ok(files
        .unwrap_or_default()
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect())
}

/// 写文本文件（导出 SRT 等）
#[tauri::command]
pub fn write_text_file(path: String, content: String) -> Result<(), String> {
    std::fs::write(&path, content).map_err(|e| e.to_string())
}

/// 读文本文件（导入 SRT 等）
#[tauri::command]
pub fn read_text_file(path: String) -> Result<String, String> {
    std::fs::read_to_string(&path).map_err(|e| e.to_string())
}

pub(crate) fn app_config_path() -> Result<std::path::PathBuf, String> {
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    Ok(executable
        .parent()
        .ok_or("应用程序路径没有父目录")?
        .join("config.json"))
}

pub(crate) fn legacy_config_path() -> Option<std::path::PathBuf> {
    dirs_next::config_dir().map(|path| path.join("videotrans").join("config.json"))
}

/// 读取配置
#[tauri::command]
pub fn load_config(state: tauri::State<Mutex<AppConfig>>) -> Result<AppConfig, String> {
    Ok(state.lock().map_err(|e| e.to_string())?.clone())
}

/// 保存配置：写磁盘 + 同步更新内存托管状态，后续任务立即使用新配置
#[tauri::command]
pub fn save_config(
    state: tauri::State<Mutex<AppConfig>>,
    mut config: AppConfig,
) -> Result<(), String> {
    let path = app_config_path()?;
    let dir = path.parent().ok_or("配置路径没有父目录")?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    config.api_max_concurrent = config.api_max_concurrent.clamp(1, 16);
    config.api_interval_ms = config.api_interval_ms.min(600_000);
    config.min_speed_percent = config.min_speed_percent.clamp(50, 100);
    config.max_speed_percent = config.max_speed_percent.clamp(100, 200);
    if config.min_speed_percent > config.max_speed_percent {
        return Err("配音变速范围无效".into());
    }
    if !matches!(config.output_naming.as_str(), "source_variant" | "final") {
        return Err("输出命名规则无效".into());
    }
    if !matches!(
        config.subtitle_mode.as_str(),
        "none" | "external_srt" | "hard_subtitle_planned"
    ) {
        return Err("字幕模式无效".into());
    }
    if config.cosyvoice_sample_rate == 0 {
        return Err("CosyVoice3 采样率不能为 0".into());
    }
    let json = serde_json::to_string_pretty(&config).unwrap();
    std::fs::write(&path, json).map_err(|e| e.to_string())?;
    *state.lock().map_err(|e| e.to_string())? = config;
    log::info!("config saved: {}", path.display());
    Ok(())
}

fn validate_openable_path(path: &str) -> Result<std::path::PathBuf, String> {
    let p = std::path::PathBuf::from(path.trim());
    if path.trim().is_empty() {
        return Err("路径为空".into());
    }
    if !p.exists() {
        return Err(format!("路径不存在：{}", p.display()));
    }
    Ok(p)
}

/// 后端打开路径：避免前端 opener.open_path 的 capability path scope 限制误伤输出目录
#[tauri::command]
pub fn open_path(path: String) -> Result<(), String> {
    let p = validate_openable_path(&path)?;
    log::info!("open path: {}", p.display());
    tauri_plugin_opener::open_path(p, None::<&str>).map_err(|e| e.to_string())
}

/// 获取日志目录（前端「打开日志目录」按钮用）
#[tauri::command]
pub fn get_log_dir() -> String {
    logger::log_dir().to_string_lossy().to_string()
}

/// 前端异常落日志（webview 里的报错静默丢太可惜，排查时两眼一抹黑）
#[tauri::command]
pub fn log_frontend(level: String, message: String) {
    let msg = logger::snippet(&message, 500);
    match level.as_str() {
        "error" => log::error!("[frontend] {}", msg),
        _ => log::warn!("[frontend] {}", msg),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_validate_openable_path() {
        let dir = std::env::temp_dir();
        assert!(super::validate_openable_path(&dir.to_string_lossy()).is_ok());
        assert!(super::validate_openable_path("   ").is_err());
        assert!(super::validate_openable_path("Z:/definitely/not/exist").is_err());
    }
}
