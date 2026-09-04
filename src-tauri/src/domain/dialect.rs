use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DialectSpec {
    pub id: String,
    pub label: String,
    pub translate_style: String,
    pub tts_accent: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LanguageDialectSpec {
    pub language: String,
    pub dialects: Vec<DialectSpec>,
}

pub fn builtin_dialects() -> Vec<LanguageDialectSpec> {
    let dialects = [
        ("mandarin", "普通话", "普通话", "请用普通话表达。"),
        ("yue", "粤语", "广东话/粤语用词", "请用广东话表达。"),
        ("dongbei", "东北话", "东北话用词", "请用东北话表达。"),
        ("gansu", "甘肃话", "甘肃话用词", "请用甘肃话表达。"),
        ("guizhou", "贵州话", "贵州话用词", "请用贵州话表达。"),
        ("henan", "河南话", "河南话用词", "请用河南话表达。"),
        ("hubei", "湖北话", "湖北话用词", "请用湖北话表达。"),
        ("hunan", "湖南话", "湖南话用词", "请用湖南话表达。"),
        ("jiangxi", "江西话", "江西话用词", "请用江西话表达。"),
        ("minnan", "闽南话", "闽南话用词", "请用闽南话表达。"),
        ("ningxia", "宁夏话", "宁夏话用词", "请用宁夏话表达。"),
        ("shanxi", "山西话", "山西话用词", "请用山西话表达。"),
        ("shaanxi", "陕西话", "陕西话用词", "请用陕西话表达。"),
        ("shandong", "山东话", "山东话用词", "请用山东话表达。"),
        ("shanghai", "上海话", "上海话用词", "请用上海话表达。"),
        ("sichuan", "四川话", "四川话用词", "请用四川话表达。"),
        ("tianjin", "天津话", "天津话用词", "请用天津话表达。"),
        ("yunnan", "云南话", "云南话用词", "请用云南话表达。"),
    ]
    .into_iter()
    .map(|(id, label, translate_style, tts_accent)| DialectSpec {
        id: id.into(),
        label: label.into(),
        translate_style: translate_style.into(),
        tts_accent: tts_accent.into(),
    })
    .collect();
    vec![LanguageDialectSpec {
        language: "zh".into(),
        dialects,
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chinese_dialects_include_mandarin_and_all_cosyvoice_accents() {
        let zh = &builtin_dialects()[0];
        assert_eq!(zh.language, "zh");
        assert_eq!(zh.dialects.len(), 18);
        assert_eq!(zh.dialects[0].id, "mandarin");
        assert!(zh.dialects.iter().any(|item| item.id == "yue"));
        assert!(zh.dialects.iter().any(|item| item.id == "minnan"));
    }
}
