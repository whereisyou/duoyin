use serde::{Deserialize, Serialize};

use super::variant::TargetVariant;

fn default_min_speed() -> u16 {
    85
}
fn default_max_speed() -> u16 {
    125
}
fn default_true() -> bool {
    true
}

/// 每个任务持久化一份执行快照。这里禁止保存 API 密钥，只记录影响产物复用的配置。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PipelineConfig {
    pub source_language: Option<String>,
    pub targets: Vec<TargetVariant>,
    pub engines: EngineSelection,
    #[serde(default)]
    pub separation: SeparationConfig,
    #[serde(default)]
    pub output: OutputConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineSelection {
    pub stt: String,
    pub translator: String,
    pub tts: String,
    pub separator: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeparationConfig {
    pub enabled: bool,
    pub denoise: bool,
    pub normalize: bool,
    #[serde(default = "default_true")]
    pub allow_no_bgm_fallback: bool,
}

impl Default for SeparationConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            denoise: false,
            normalize: false,
            allow_no_bgm_fallback: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputConfig {
    pub generate_final_videos: bool,
    #[serde(default)]
    pub naming: OutputNaming,
    pub keep_original_audio_track: bool,
    #[serde(default = "default_min_speed")]
    pub min_speed_percent: u16,
    #[serde(default = "default_max_speed")]
    pub max_speed_percent: u16,
    #[serde(default)]
    pub subtitle: SubtitleMode,
}

impl Default for OutputConfig {
    fn default() -> Self {
        Self {
            generate_final_videos: false,
            naming: OutputNaming::SourceVariant,
            keep_original_audio_track: false,
            min_speed_percent: default_min_speed(),
            max_speed_percent: default_max_speed(),
            subtitle: SubtitleMode::None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum OutputNaming {
    #[default]
    SourceVariant,
    Final,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SubtitleMode {
    #[default]
    None,
    ExternalSrt,
    /// 配置占位；执行器在基础版必须返回“不支持”，不能静默忽略。
    HardSubtitlePlanned,
}

impl PipelineConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.targets.is_empty() {
            return Err("至少选择一个目标语言或方言".into());
        }
        if self.output.min_speed_percent == 0
            || self.output.min_speed_percent > self.output.max_speed_percent
        {
            return Err("配音变速范围无效".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> PipelineConfig {
        PipelineConfig {
            source_language: None,
            targets: vec![TargetVariant::zh_mandarin()],
            engines: EngineSelection {
                stt: "sensevoice".into(),
                translator: "openai-compatible".into(),
                tts: "cosyvoice3".into(),
                separator: None,
            },
            separation: SeparationConfig::default(),
            output: OutputConfig::default(),
        }
    }

    #[test]
    fn safe_defaults_prioritize_success_and_low_storage() {
        let c = config();
        assert!(!c.separation.enabled);
        assert!(c.separation.allow_no_bgm_fallback);
        assert!(!c.output.generate_final_videos);
        assert!(!c.output.keep_original_audio_track);
        assert_eq!(
            (c.output.min_speed_percent, c.output.max_speed_percent),
            (85, 125)
        );
        assert!(c.validate().is_ok());
    }

    #[test]
    fn target_is_required() {
        let mut c = config();
        c.targets.clear();
        assert!(c.validate().is_err());
    }

    #[test]
    fn old_snapshot_gets_new_advanced_defaults() {
        let json = r#"{
            "source_language": null,
            "targets": [{
                "id":"zh-CN","language":"zh","dialect":"mandarin",
                "display_name":"中文（普通话）","translate_style":"mandarin","tts_accent":"mandarin"
            }],
            "engines":{"stt":"sensevoice","translator":"api","tts":"cosyvoice3","separator":null}
        }"#;
        let loaded: PipelineConfig = serde_json::from_str(json).unwrap();
        assert_eq!(loaded.separation, SeparationConfig::default());
        assert_eq!(loaded.output, OutputConfig::default());
    }
}
