use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::domain::dialect::{builtin_dialects, DialectSpec, LanguageDialectSpec};

pub fn default_dialect_config_path() -> Result<PathBuf, String> {
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    Ok(executable
        .parent()
        .ok_or("应用程序路径没有父目录")?
        .join("config")
        .join("dialects.json"))
}

pub fn load_dialects(path: &Path) -> Result<Vec<LanguageDialectSpec>, String> {
    let mut merged: BTreeMap<String, BTreeMap<String, DialectSpec>> = BTreeMap::new();
    merge(&mut merged, builtin_dialects());
    if path.exists() {
        let external = fs::read(path)
            .map_err(|error| format!("读取方言配置失败: {error}"))
            .and_then(|bytes| {
                serde_json::from_slice::<Vec<LanguageDialectSpec>>(&bytes)
                    .map_err(|error| format!("方言配置 JSON 无效 {}: {error}", path.display()))
            })
            .and_then(|specs| {
                validate_specs(&specs)?;
                Ok(specs)
            });
        match external {
            Ok(specs) => merge(&mut merged, specs),
            Err(error) => log::warn!("{error}；已退回内置方言配置"),
        }
    }
    Ok(merged
        .into_iter()
        .map(|(language, dialects)| LanguageDialectSpec {
            language,
            dialects: dialects.into_values().collect(),
        })
        .collect())
}

fn merge(
    target: &mut BTreeMap<String, BTreeMap<String, DialectSpec>>,
    specs: Vec<LanguageDialectSpec>,
) {
    for language in specs {
        let dialects = target.entry(language.language).or_default();
        for dialect in language.dialects {
            dialects.insert(dialect.id.clone(), dialect);
        }
    }
}

fn validate_specs(specs: &[LanguageDialectSpec]) -> Result<(), String> {
    for language in specs {
        if !safe_id(&language.language) {
            return Err(format!("不安全的语言 ID: {}", language.language));
        }
        for dialect in &language.dialects {
            if !safe_id(&dialect.id) || dialect.label.trim().is_empty() {
                return Err(format!(
                    "不安全或无标签的方言配置: {}/{}",
                    language.language, dialect.id
                ));
            }
        }
    }
    Ok(())
}

fn safe_id(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && !value.contains(['/', '\\', ':', '<', '>', '"', '|', '?', '*'])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_config_overrides_and_extends_builtin_data() {
        let root = std::env::temp_dir().join(format!("dialects-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("dialects.json");
        fs::write(
            &path,
            r#"[
              {"language":"zh","dialects":[
                {"id":"yue","label":"广东话","translate_style":"粤语","tts_accent":"广东话"},
                {"id":"custom","label":"自定义","translate_style":"自定义用词","tts_accent":"自定义口音"}
              ]}
            ]"#
                .as_bytes(),
        )
        .unwrap();

        let loaded = load_dialects(&path).unwrap();
        let zh = loaded.iter().find(|item| item.language == "zh").unwrap();
        assert_eq!(
            zh.dialects
                .iter()
                .find(|item| item.id == "yue")
                .unwrap()
                .label,
            "广东话"
        );
        assert!(zh.dialects.iter().any(|item| item.id == "custom"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn malformed_external_config_falls_back_to_builtin_data() {
        let root = std::env::temp_dir().join(format!("dialects-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("dialects.json");
        fs::write(&path, b"not-json").unwrap();
        let loaded = load_dialects(&path).unwrap();
        let zh = loaded.iter().find(|item| item.language == "zh").unwrap();
        assert!(zh.dialects.iter().any(|item| item.id == "mandarin"));
        assert!(zh.dialects.iter().any(|item| item.id == "yue"));
        fs::remove_dir_all(root).unwrap();
    }
}
