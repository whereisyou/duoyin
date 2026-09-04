//! Supertonic 模型资产校验：官方 31 语言 + Supertonic-ZH 中文扩展的文件齐备性探测。
//! 纯文件存在性检查，不加载 ONNX 会话（合成与引擎加载在 supertonic/mod.rs）。

use std::path::{Path, PathBuf};

use super::helper as sh;

// 合成侧（mod.rs 的 load_engine）也要定位 onnx/ 目录，故对 supertonic 子树可见。
pub(in crate::engines::tts::supertonic) fn onnx_dir(dir: &str) -> PathBuf {
    Path::new(dir).join("onnx")
}

const OFFICIAL_MODELS: [&str; 4] = [
    "duration_predictor",
    "text_encoder",
    "vector_estimator",
    "vocoder",
];
const ZH_REPLACE_MODELS: [&str; 3] = ["duration_predictor", "text_encoder", "vector_estimator"];

pub fn missing_official_files(dir: &str) -> Vec<String> {
    let d = onnx_dir(dir);
    let mut missing: Vec<String> = OFFICIAL_MODELS
        .iter()
        .map(|name| format!("{name}.onnx"))
        .filter(|name| !d.join(name).is_file())
        .collect();
    for name in ["unicode_indexer.json", "tts.json"] {
        if !d.join(name).is_file() {
            missing.push(name.into());
        }
    }
    missing
}

/// 官方 31 语言模型是否就绪
pub fn official_available(dir: &str) -> bool {
    missing_official_files(dir).is_empty()
}

/// 返回中文扩展缺失的文件名；空列表表示可用。
pub fn missing_zh_files(dir: &str) -> Vec<String> {
    let d = onnx_dir(dir);
    let mut missing: Vec<String> = ZH_REPLACE_MODELS
        .iter()
        .map(|name| format!("{name}_zh.onnx"))
        .filter(|name| !d.join(name).is_file())
        .collect();
    for name in ["unicode_indexer_zh.json", "vocoder.onnx", "tts.json"] {
        if !d.join(name).is_file() {
            missing.push(name.into());
        }
    }
    missing
}

/// ZH 中文扩展是否就绪（三件 *_zh.onnx + 中文索引 + 共用的 vocoder/tts.json）
pub fn zh_available(dir: &str) -> bool {
    missing_zh_files(dir).is_empty()
}

/// 目标语言是否可配音（官方 31 语言，或已安装 ZH 扩展时的中文）
pub fn lang_supported(dir: &str, lang: &str) -> bool {
    if lang == "zh" {
        return zh_available(dir);
    }
    sh::is_valid_lang(lang) && official_available(dir)
}

pub fn validate_language_assets(dir: &str, lang: &str) -> Result<(), String> {
    if lang == "zh" {
        let missing = missing_zh_files(dir);
        if missing.is_empty() {
            Ok(())
        } else {
            Err(format!(
                "中文配音缺少 Supertonic-ZH 文件：{}；请安装中文扩展，或改用 CosyVoice3",
                missing.join("、")
            ))
        }
    } else if !sh::is_valid_lang(lang) {
        Err(format!("Supertonic 不支持目标语言代码 {lang}"))
    } else {
        let missing = missing_official_files(dir);
        if missing.is_empty() {
            Ok(())
        } else {
            Err(format!("Supertonic 基础模型缺少文件：{}", missing.join("、")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(p: &Path) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, b"").unwrap();
    }

    #[test]
    fn validate_language_assets_lists_missing_chinese_files() {
        let dir = std::env::temp_dir().join(format!("supertonic-missing-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("onnx")).unwrap();
        let error = validate_language_assets(&dir.to_string_lossy(), "zh").unwrap_err();
        assert!(error.contains("duration_predictor_zh.onnx"));
        assert!(error.contains("unicode_indexer_zh.json"));
        assert!(error.contains("CosyVoice3"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn test_availability_detection() {
        let root = std::env::temp_dir().join(format!("vt_tts_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir = root.to_string_lossy().to_string();
        let onnx = root.join("onnx");

        // 空目录：什么都不支持
        assert!(!official_available(&dir));
        assert!(!zh_available(&dir));
        assert!(!lang_supported(&dir, "en"));
        assert!(!lang_supported(&dir, "zh"));

        // 仅官方资产：en 可配，zh 不可
        for n in OFFICIAL_MODELS {
            touch(&onnx.join(format!("{}.onnx", n)));
        }
        touch(&onnx.join("unicode_indexer.json"));
        touch(&onnx.join("tts.json"));
        assert!(official_available(&dir));
        assert!(lang_supported(&dir, "en"));
        assert!(!lang_supported(&dir, "zh"));

        // 放入 ZH 扩展：zh 也可配
        for n in ZH_REPLACE_MODELS {
            touch(&onnx.join(format!("{}_zh.onnx", n)));
        }
        touch(&onnx.join("unicode_indexer_zh.json"));
        assert!(missing_zh_files(&dir).is_empty());
        assert!(zh_available(&dir));
        assert!(lang_supported(&dir, "zh"));

        let _ = std::fs::remove_dir_all(&root);
    }
}
