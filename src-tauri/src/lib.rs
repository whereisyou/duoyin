//! Composition root：只负责模块声明、配置加载与 Tauri Builder 装配。
//!
//! 命令实现按职责拆在 commands/ 下（task / config / api_test / tasks / media_tools /
//! runtime / realtime_stt / diarization），运行中任务登记表在 state.rs。
//! 前端 invoke 只认 IPC 函数名，改模块路径不影响前端。

mod adapters;
mod application;
mod audio_align;
#[cfg(feature = "inference")]
mod audio_io;
mod commands;
mod domain;
mod engines;
mod infra;
mod legacy;
mod logger;
mod memcheck;
mod pipeline;
mod ports;
mod scheduler;
mod segments;
mod state;
mod subtitle;
mod subtitle_parse;
mod text_align;
mod tts_dub;
mod types;

#[cfg(all(test, feature = "inference"))]
mod e2e;

use std::sync::Mutex;

use tauri::{Emitter, Manager};

use commands::config::{app_config_path, legacy_config_path};
use types::AppConfig;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // 日志必须最先初始化：之后任何一步（含 Builder 构建、插件加载）出错都有记录
    let log_dir = logger::init();
    log::info!(
        "=== videotrans-tauri v{} starting, logs: {} ===",
        env!("CARGO_PKG_VERSION"),
        log_dir.display()
    );

    let saved_config = (|| -> Option<AppConfig> {
        let primary = app_config_path().ok()?;
        let path = if primary.is_file() {
            primary
        } else {
            legacy_config_path()?.is_file().then(|| legacy_config_path().unwrap())?
        };
        let data = std::fs::read_to_string(&path).ok()?;
        let mut config = serde_json::from_str::<AppConfig>(&data).ok()?;
        // 旧 config.json 空路径字段回填开箱默认（新用户/换机器后第一次启动也拿到可用目录）
        config.normalize_defaults();
        log::info!("config loaded: {}", path.display());
        Some(config)
    })();
    log::info!(
        "config: {}",
        if saved_config.is_some() {
            "loaded from disk"
        } else {
            "default (no saved config)"
        }
    );

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(Mutex::new(saved_config.unwrap_or_default()))
        .setup(|app| {
            if let Some(window) = app.get_webview_window("main") {
                let w = window.clone();
                window.on_window_event(move |event| {
                    if let tauri::WindowEvent::DragDrop(evt) = event {
                        match evt {
                            tauri::DragDropEvent::Enter { paths, .. } => {
                                let paths: Vec<String> = paths
                                    .iter()
                                    .map(|p| p.to_string_lossy().to_string())
                                    .collect();
                                let _ = w.emit(
                                    "tauri-drag-drop",
                                    serde_json::json!({
                                        "type": "enter",
                                        "paths": paths
                                    }),
                                );
                            }
                            tauri::DragDropEvent::Drop { paths, .. } => {
                                let paths: Vec<String> = paths
                                    .iter()
                                    .map(|p| p.to_string_lossy().to_string())
                                    .collect();
                                let _ = w.emit(
                                    "tauri-drag-drop",
                                    serde_json::json!({
                                        "type": "drop",
                                        "paths": paths
                                    }),
                                );
                            }
                            tauri::DragDropEvent::Leave => {
                                let _ = w.emit(
                                    "tauri-drag-drop",
                                    serde_json::json!({
                                        "type": "leave"
                                    }),
                                );
                            }
                            _ => {}
                        }
                    }
                });
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            legacy::command::start_task,
            commands::task::start_multi_target_task,
            commands::tasks::load_dialect_specs,
            commands::tasks::ensure_uvr_model,
            commands::tasks::create_persistent_task,
            commands::tasks::list_persistent_tasks,
            commands::tasks::load_persistent_task,
            commands::tasks::import_target_srt,
            commands::tasks::delete_persistent_task,
            commands::tasks::load_task_segments,
            commands::tasks::save_task_segments,
            commands::media_tools::match_text_to_srt,
            commands::media_tools::clip_video,
            commands::media_tools::separate_media,
            commands::media_tools::merge_video_audio,
            commands::runtime::get_runtime_info,
            commands::realtime_stt::transcribe_audio_chunk,
            commands::diarization::run_speaker_diarization,
            commands::api_test::test_api_endpoint,
            commands::api_test::test_api_reachable,
            commands::task::cancel_task,
            commands::task::cancel_child_task,
            commands::config::load_config,
            commands::config::save_config,
            commands::config::check_ffmpeg,
            commands::config::pick_video_files,
            commands::config::pick_onnx_model,
            commands::config::write_text_file,
            commands::config::read_text_file,
            commands::config::open_path,
            commands::config::get_log_dir,
            commands::config::log_frontend,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
