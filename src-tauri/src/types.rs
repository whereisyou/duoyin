use serde::{Deserialize, Serialize};

/// 字幕段
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Segment {
    pub idx: usize,
    pub start: f64,
    pub end: f64,
    pub text: String,
    #[serde(default)]
    pub translated: String,
}

fn default_stt_engine() -> String {
    // 默认给内存友好的 SenseVoice（~1.2GB）；whisper_native 需 ~3.9GB commit，
    // 低内存机开箱跑首任务必被 memcheck 拒绝——默认引擎必须开箱可跑。
    "sensevoice".into()
}
fn default_tts_engine() -> String {
    "supertonic".into()
}
fn default_deepseek_model() -> String {
    "deepseek-chat".into()
}
fn default_deepseek_url() -> String {
    "https://api.deepseek.com/chat/completions".into()
}
fn default_cosyvoice_url() -> String {
    "http://127.0.0.1:50000".into()
}
fn default_cosyvoice_sample_rate() -> u32 {
    24000
}

fn default_zipvoice_num_threads() -> i32 {
    2
}
fn default_false() -> bool {
    false
}

// 开箱即用默认模型路径（本机已存在，与前端 src/lib/engines.ts DEFAULT_CONFIG 对齐；
// 两处任一处改动必须同步并更新两侧契约测试）。已保存的 config.json 优先，见 normalize_defaults。
fn default_sensevoice_dir() -> String {
    "E:/projects/test2voices_backup/sense-voice-int8".into()
}
fn default_whisper_model_dir() -> String {
    "E:/projects/text2voices/CosyVoice/pretrained_models/whisper-large-v3-turbo".into()
}
fn default_supertonic_dir() -> String {
    "E:/projects/pyvideotrans-3.98/Supertone/supertonic-3".into()
}
fn default_zipvoice_dir() -> String {
    "E:/projects/test2voices_backup/sherpa-onnx-zipvoice-distill-int8-zh-en-emilia".into()
}
fn default_zipvoice_prompt_wav() -> String {
    "E:/projects/test2voices_backup/sherpa-onnx-zipvoice-distill-int8-zh-en-emilia/test_wavs/news-female.wav".into()
}
fn default_zipvoice_prompt_text() -> String {
    "各位村民, 大家新年好! 近期, 湖北省武汉市等多个地区".into()
}
fn default_api_max_concurrent() -> usize {
    1
}
fn default_api_interval_ms() -> u64 {
    1000
}
fn default_true() -> bool {
    true
}
fn default_min_speed_percent() -> u16 {
    85
}
fn default_max_speed_percent() -> u16 {
    125
}
fn default_subtitle_mode() -> String {
    "none".into()
}
fn default_output_naming() -> String {
    "source_variant".into()
}

/// 应用配置（持久化；字段均带 serde 默认值，旧版 config.json 可无缝升级）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    // STT：sensevoice（轻量首选）| whisper_native（candle）| whisper_local（CLI）| openai_api
    #[serde(default = "default_stt_engine")]
    pub stt_engine: String,
    /// SenseVoice 引擎：模型目录（model.int8.onnx + tokens.txt）
    #[serde(default = "default_sensevoice_dir")]
    pub sensevoice_dir: String,
    /// candle 引擎：HF 模型目录（config.json + model.safetensors + tokenizer.json）
    #[serde(default = "default_whisper_model_dir")]
    pub whisper_model_dir: String,
    /// whisper.cpp CLI 引擎：可执行文件与 ggml 模型
    #[serde(default)]
    pub whisper_cli_path: String,
    #[serde(default)]
    pub whisper_model_path: String,
    #[serde(default)]
    pub whisper_use_gpu: bool,
    #[serde(default)]
    pub openai_key: String,

    // 翻译
    #[serde(default)]
    pub deepseek_key: String,
    #[serde(default = "default_deepseek_model")]
    pub deepseek_model: String,
    /// 翻译 API 地址（OpenAI 兼容格式）
    #[serde(default = "default_deepseek_url")]
    pub deepseek_api_url: String,

    // TTS：supertonic（本地 ONNX 推理，默认）| cosyvoice3（预留）
    #[serde(default = "default_tts_engine")]
    pub tts_engine: String,
    /// Supertonic 资产目录（含 onnx/ 与 voice_styles/）
    #[serde(default = "default_supertonic_dir")]
    pub supertonic_dir: String,
    #[serde(default)]
    pub supertonic_voice: String,
    #[serde(default = "default_cosyvoice_url")]
    pub cosyvoice_url: String,
    #[serde(default)]
    pub cosyvoice_key: String,
    #[serde(default)]
    pub cosyvoice_voice: String,
    /// CosyVoice3 instruct2 必需的参考音频（至少 3 秒 WAV）
    #[serde(default)]
    pub cosyvoice_prompt_wav: String,
    /// 参考音频的文本内容（zero_shot 模式必需，instruct2 方言模式可留空）
    #[serde(default)]
    pub cosyvoice_prompt_text: String,
    /// FastAPI 返回裸 PCM16，不携带采样率；CosyVoice2/3 默认 24000。
    #[serde(default = "default_cosyvoice_sample_rate")]
    pub cosyvoice_sample_rate: u32,

    /// ZipVoice Distill INT8 模型目录（含 encoder/decoder.int8.onnx、tokens.txt、
    /// lexicon.txt、espeak-ng-data/、vocos_24khz.onnx）
    #[serde(default = "default_zipvoice_dir")]
    pub zipvoice_dir: String,
    /// 零样本音色克隆参考音频（至少 3 秒的干净 WAV）
    #[serde(default = "default_zipvoice_prompt_wav")]
    pub zipvoice_prompt_wav: String,
    /// 参考音频逐字转写（须与音频内容一致，否则克隆失败）
    #[serde(default = "default_zipvoice_prompt_text")]
    pub zipvoice_prompt_text: String,
    /// ZipVoice ONNX Runtime CPU 线程数
    #[serde(default = "default_zipvoice_num_threads")]
    pub zipvoice_num_threads: i32,
    /// 配音克隆原视频音色：TTS 前自动从原声提取参考段（shared/ref_voice.wav），
    /// 覆盖零样本引擎（ZipVoice）的全局参考音频；提取失败回退全局参考
    #[serde(default = "default_false")]
    pub tts_use_video_prompt: bool,

    // 外部 API 调度（翻译 / 云 STT / 云 TTS 共用，防限流）
    #[serde(default)]
    pub http_proxy: String,
    #[serde(default = "default_api_max_concurrent")]
    pub api_max_concurrent: usize,
    #[serde(default = "default_api_interval_ms")]
    pub api_interval_ms: u64,

    // 主流程高级设置（持久化，UI 放二级菜单）
    #[serde(default)]
    pub separation_enabled: bool,
    #[serde(default)]
    pub separator_model_path: String,
    #[serde(default)]
    pub diarization_seg_model: String,
    #[serde(default)]
    pub diarization_embedding_model: String,
    #[serde(default)]
    pub diarization_num_speakers: i32,
    #[serde(default)]
    pub separation_denoise: bool,
    #[serde(default)]
    pub separation_normalize: bool,
    #[serde(default = "default_true")]
    pub separation_fallback_no_bgm: bool,
    #[serde(default = "default_true")]
    pub generate_final_videos: bool,
    #[serde(default = "default_output_naming")]
    pub output_naming: String,
    #[serde(default)]
    pub keep_original_audio_track: bool,
    #[serde(default = "default_min_speed_percent")]
    pub min_speed_percent: u16,
    #[serde(default = "default_max_speed_percent")]
    pub max_speed_percent: u16,
    #[serde(default = "default_subtitle_mode")]
    pub subtitle_mode: String,

    // 输出
    #[serde(default)]
    pub output_dir: String,
}

impl AppConfig {
    /// 开箱即用兜底：config.json 字段缺失（serde default 已处理）或为空字符串时回填默认路径。
    /// 只回填有默认值的模型路径字段；用户显式保存的非空值一律不动。
    pub fn normalize_defaults(&mut self) {
        if self.sensevoice_dir.trim().is_empty() {
            self.sensevoice_dir = default_sensevoice_dir();
        }
        if self.whisper_model_dir.trim().is_empty() {
            self.whisper_model_dir = default_whisper_model_dir();
        }
        if self.supertonic_dir.trim().is_empty() {
            self.supertonic_dir = default_supertonic_dir();
        }
        if self.zipvoice_dir.trim().is_empty() {
            self.zipvoice_dir = default_zipvoice_dir();
        }
        if self.zipvoice_prompt_wav.trim().is_empty() {
            self.zipvoice_prompt_wav = default_zipvoice_prompt_wav();
        }
        if self.zipvoice_prompt_text.trim().is_empty() {
            self.zipvoice_prompt_text = default_zipvoice_prompt_text();
        }
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            stt_engine: default_stt_engine(),
            sensevoice_dir: default_sensevoice_dir(),
            whisper_model_dir: default_whisper_model_dir(),
            whisper_cli_path: String::new(),
            whisper_model_path: String::new(),
            whisper_use_gpu: false,
            openai_key: String::new(),
            deepseek_key: String::new(),
            deepseek_model: default_deepseek_model(),
            deepseek_api_url: default_deepseek_url(),
            tts_engine: default_tts_engine(),
            supertonic_dir: default_supertonic_dir(),
            supertonic_voice: String::new(),
            cosyvoice_url: default_cosyvoice_url(),
            cosyvoice_key: String::new(),
            cosyvoice_voice: String::new(),
            cosyvoice_prompt_wav: String::new(),
            cosyvoice_prompt_text: String::new(),
            cosyvoice_sample_rate: default_cosyvoice_sample_rate(),
            zipvoice_dir: default_zipvoice_dir(),
            zipvoice_prompt_wav: default_zipvoice_prompt_wav(),
            zipvoice_prompt_text: default_zipvoice_prompt_text(),
            zipvoice_num_threads: default_zipvoice_num_threads(),
            tts_use_video_prompt: false,
            http_proxy: String::new(),
            api_max_concurrent: default_api_max_concurrent(),
            api_interval_ms: default_api_interval_ms(),
            separation_enabled: false,
            separator_model_path: String::new(),
            diarization_seg_model: String::new(),
            diarization_embedding_model: String::new(),
            diarization_num_speakers: -1,
            separation_denoise: false,
            separation_normalize: false,
            separation_fallback_no_bgm: true,
            generate_final_videos: true,
            output_naming: default_output_naming(),
            keep_original_audio_track: false,
            min_speed_percent: default_min_speed_percent(),
            max_speed_percent: default_max_speed_percent(),
            subtitle_mode: default_subtitle_mode(),
            output_dir: String::new(),
        }
    }
}

/// 任务配置（每次执行）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskConfig {
    pub video: String,
    pub source_lang: String,
    pub target_lang: String,
}

/// 进度事件（发给前端）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgressEvent {
    pub step: String,
    pub progress: u8,
    pub status: String,
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub segments: Option<Vec<Segment>>,
    /// 完成时返回实际输出目录（前端"打开输出目录"用）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_status: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 旧版 config.json 缺字段必须无缝升级（serde default 生效）
    #[test]
    fn test_config_backward_compat() {
        let old = r#"{"openai_key":"sk-old","output_dir":"D:\\out"}"#;
        let c: AppConfig = serde_json::from_str(old).unwrap();
        assert_eq!(c.openai_key, "sk-old");
        assert_eq!(c.output_dir, "D:\\out");
        assert_eq!(c.stt_engine, "sensevoice");
        assert_eq!(c.tts_engine, "supertonic");
        assert_eq!(c.deepseek_model, "deepseek-chat");
        assert!(!c.deepseek_api_url.is_empty());
        assert_eq!(c.api_max_concurrent, 1);
        assert_eq!(c.api_interval_ms, 1000);
        assert!(!c.separation_enabled);
        assert!(c.separation_fallback_no_bgm);
        assert!(c.generate_final_videos);
        assert_eq!((c.min_speed_percent, c.max_speed_percent), (85, 125));
        assert_eq!(c.subtitle_mode, "none");
    }

    /// 未知字段被忽略（新版配置回退旧版程序也不炸）
    #[test]
    fn test_config_ignores_unknown_fields() {
        let future = r#"{"stt_engine":"openai_api","some_future_field":123}"#;
        let c: AppConfig = serde_json::from_str(future).unwrap();
        assert_eq!(c.stt_engine, "openai_api");
    }

    /// 默认配置必须携带本机模型路径（与前端 DEFAULT_CONFIG 对齐的防漂移门禁）
    #[test]
    fn default_config_carries_local_model_paths() {
        let c = AppConfig::default();
        assert!(!c.sensevoice_dir.is_empty());
        assert!(!c.whisper_model_dir.is_empty());
        assert!(!c.supertonic_dir.is_empty());
        assert!(!c.zipvoice_dir.is_empty());
        assert!(!c.zipvoice_prompt_wav.is_empty());
        assert!(!c.zipvoice_prompt_text.is_empty());
    }

    /// 旧 config.json 里空字符串模型路径被 normalize 回填默认，用户非空值不受影响
    #[test]
    fn normalize_defaults_fills_empty_paths_only() {
        let saved = r#"{"openai_key":"sk-keep","zipvoice_dir":"","zipvoice_prompt_wav":"","zipvoice_prompt_text":""}"#;
        let mut c: AppConfig = serde_json::from_str(saved).unwrap();
        c.normalize_defaults();
        assert_eq!(c.zipvoice_dir, default_zipvoice_dir());
        assert_eq!(c.zipvoice_prompt_wav, default_zipvoice_prompt_wav());
        assert_eq!(c.zipvoice_prompt_text, default_zipvoice_prompt_text());
        // 缺省字段经 serde default 拿到默认值（normalize 前即已生效）
        assert_eq!(c.supertonic_dir, default_supertonic_dir());
        assert_eq!(c.sensevoice_dir, default_sensevoice_dir());
        // 用户显式保存的非空值不被覆盖
        assert_eq!(c.openai_key, "sk-keep");
    }
}
