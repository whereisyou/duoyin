use serde::{Deserialize, Serialize};

use super::ids::VariantId;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetVariant {
    pub id: VariantId,
    pub language: String,
    pub dialect: Option<String>,
    pub display_name: String,
    pub translate_style: String,
    pub tts_accent: String,
}

impl TargetVariant {
    pub fn language(code: &str) -> Result<Self, String> {
        let code = code.trim();
        if code.is_empty() || code == "." || code == ".." || code.contains(['/', '\\', ':']) {
            return Err("目标语言代码不安全或为空".into());
        }
        if code == "zh" || code == "zh-CN" {
            return Ok(Self::zh_mandarin());
        }
        let display_name = match code {
            "en" => "英语",
            "ja" => "日语",
            "ko" => "韩语",
            "fr" => "法语",
            "de" => "德语",
            "es" => "西班牙语",
            "ru" => "俄语",
            other => other,
        };
        Ok(Self {
            id: VariantId(code.into()),
            language: code.into(),
            dialect: None,
            display_name: display_name.into(),
            translate_style: String::new(),
            tts_accent: String::new(),
        })
    }

    pub fn zh_mandarin() -> Self {
        Self {
            id: VariantId("zh-CN".into()),
            language: "zh".into(),
            dialect: Some("mandarin".into()),
            display_name: "中文（普通话）".into(),
            translate_style: "mandarin".into(),
            tts_accent: "mandarin".into(),
        }
    }

    /// 方言目标构造辅助（方言列表配置解析用，与 language() 平行）
    #[allow(dead_code)]
    pub fn zh_dialect(id: &str, label: &str, instruct: &str) -> Self {
        Self {
            id: VariantId(format!("zh-{id}")),
            language: "zh".into(),
            dialect: Some(id.into()),
            display_name: format!("中文（{label}）"),
            translate_style: id.into(),
            tts_accent: instruct.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generic_language_has_stable_variant_id() {
        let v = TargetVariant::language("en").unwrap();
        assert_eq!(v.id.0, "en");
        assert_eq!(v.language, "en");
        assert_eq!(v.display_name, "英语");
        assert!(v.dialect.is_none());
    }

    #[test]
    fn chinese_default_variant_is_mandarin() {
        let v = TargetVariant::zh_mandarin();
        assert_eq!(v.id.0, "zh-CN");
        assert_eq!(v.display_name, "中文（普通话）");
        assert_eq!(v.dialect.as_deref(), Some("mandarin"));
    }

    #[test]
    fn dialect_variant_has_stable_id_and_display_name() {
        let v = TargetVariant::zh_dialect("yue", "粤语", "请用广东话表达。");
        assert_eq!(v.id.0, "zh-yue");
        assert_eq!(v.display_name, "中文（粤语）");
        assert_eq!(v.tts_accent, "请用广东话表达。");
    }
}
