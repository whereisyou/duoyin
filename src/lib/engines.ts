import type { AppConfig } from "./types";

/**
 * 引擎注册表：设置页按此数据驱动渲染。
 * 新增引擎 = 在对应数组加一条记录，无需改页面代码。
 */

export interface FieldDef {
  key: keyof AppConfig;
  label: string;
  type: "text" | "password" | "switch" | "number";
  placeholder?: string;
  hint?: string;
  /** 显示"浏览"按钮：选文件或目录 */
  browse?: "file" | "dir";
  /** browse=file 时的扩展名过滤 */
  extensions?: string[];
  /** 该字段可做连通性测试：显示「测试连通」按钮。同引擎的 password 字段作 Key、*_model 字段作模型 */
  testable?: boolean;
  /** 测试方式：chat = POST chat/completions（验证鉴权+模型）；reachable = GET 只验连通。默认 chat */
  testMode?: "chat" | "reachable";
  /** 固定测试地址（字段本身不是 URL 时用，如 openai_key 测 OpenAI 固定端点） */
  testUrl?: string;
}

export interface EngineDef {
  id: string;
  label: string;
  desc?: string;
  fields: FieldDef[];
  /** 该引擎是否已配置就绪（驱动服务状态圆点与"开始翻译"可用性） */
  ready: (cfg: AppConfig) => boolean;
}

export const STT_ENGINES: EngineDef[] = [
  {
    id: "sensevoice",
    label: "SenseVoice（本地·轻量·快，推荐）",
    desc: "sherpa-onnx 推理，模型仅 245MB，速度比 Whisper 快约 10 倍；中英日韩粤，自带标点",
    fields: [
      {
        key: "sensevoice_dir",
        label: "模型目录",
        type: "text",
        browse: "dir",
        placeholder: "例如 E:/models/sense-voice-int8",
        hint: "目录需含 model.int8.onnx + tokens.txt；下载：HF 镜像 https://hf-mirror.com/csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17 （只需这两个文件）",
      },
    ],
    ready: (c) => !!c.sensevoice_dir,
  },
  {
    id: "whisper_native",
    label: "本地 Whisper（内置 Rust 推理）",
    desc: "离线识别，candle 引擎直接读取 HuggingFace 模型，无需外部程序",
    fields: [
      {
        key: "whisper_model_dir",
        label: "模型目录（HF 格式）",
        type: "text",
        browse: "dir",
        placeholder: "例如 E:/models/whisper-large-v3-turbo",
        hint: "目录需含 config.json / model.safetensors / tokenizer.json",
      },
      {
        key: "whisper_use_gpu",
        label: "使用 GPU 加速",
        type: "switch",
        hint: "需要 CUDA 版构建，当前版本为 CPU 推理",
      },
    ],
    ready: (c) => !!c.whisper_model_dir,
  },
  {
    id: "whisper_local",
    label: "whisper.cpp CLI（备选）",
    desc: "外部 whisper.cpp 命令行，CPU 优化最好",
    fields: [
      {
        key: "whisper_cli_path",
        label: "whisper-cli 可执行文件",
        type: "text",
        browse: "file",
        extensions: ["exe"],
        placeholder: "例如 D:/whisper/whisper-cli.exe",
        hint: "whisper.cpp 的 CLI 程序（v1.5+）",
      },
      {
        key: "whisper_model_path",
        label: "模型文件（ggml）",
        type: "text",
        browse: "file",
        extensions: ["bin"],
        placeholder: "例如 D:/models/ggml-base.bin",
        hint: "模型越大越准但越慢，base / small 适合日常使用",
      },
    ],
    ready: (c) => !!c.whisper_cli_path && !!c.whisper_model_path,
  },
  {
    id: "openai_api",
    label: "OpenAI Whisper API",
    desc: "云端识别，按量计费",
    fields: [
      {
        key: "openai_key",
        label: "OpenAI API Key",
        type: "password",
        placeholder: "sk-...",
        hint: "使用 OpenAI Whisper 接口转写语音",
        testable: true,
        testMode: "reachable",
        testUrl: "https://api.openai.com/v1/models",
      },
    ],
    ready: (c) => !!c.openai_key,
  },
];

export const TRANSLATE_ENGINES: EngineDef[] = [
  {
    id: "deepseek",
    label: "DeepSeek / OpenAI 兼容",
    desc: "支持任何 OpenAI 兼容 API（DeepSeek、硅基流动、通义千问、Ollama 等）",
    fields: [
      {
        key: "deepseek_api_url",
        label: "API 地址",
        type: "text",
        testable: true,
        placeholder: "https://api.deepseek.com/chat/completions",
        hint: "OpenAI 兼容的 chat/completions 接口",
      },
      { key: "deepseek_key", label: "API Key", type: "password", placeholder: "sk-..." },
      { key: "deepseek_model", label: "模型", type: "text", placeholder: "deepseek-chat" },
    ],
    ready: (c) => !!c.deepseek_key,
  },
];

export const TTS_ENGINES: EngineDef[] = [
  {
    id: "supertonic",
    label: "Supertonic 3（内置 Rust 推理）",
    desc: "本地 ONNX 推理，31 种语言；放入 Supertonic-ZH 扩展后支持中文",
    fields: [
      {
        key: "supertonic_dir",
        label: "模型资产目录",
        type: "text",
        browse: "dir",
        placeholder: "例如 E:/projects/supertonic-3.0.0/assets",
        hint: "含 onnx/ 与 voice_styles/（HF Supertone/supertonic-3）；中文：再把 Supertonic-ZH 的 *_zh.onnx 与 unicode_indexer_zh.json 放入 onnx/",
      },
      {
        key: "supertonic_voice",
        label: "音色",
        type: "text",
        placeholder: "留空自动选择（中文 voice_zh，其他 M1）",
        hint: "voice_styles 目录中的音色名，如 M1 / F1 / voice_zh",
      },
    ],
    ready: (c) => !!c.supertonic_dir,
  },
  {
    id: "cosyvoice3",
    label: "CosyVoice 3",
    desc: "阿里 CosyVoice3 FastAPI，本地 instruct2 方言/风格控制",
    fields: [
      { key: "cosyvoice_url", label: "FastAPI 服务地址", type: "text", testable: true, testMode: "reachable", placeholder: "http://127.0.0.1:50000" },
      { key: "cosyvoice_key", label: "API Key（可选）", type: "password", placeholder: "本地服务通常留空" },
      { key: "cosyvoice_voice", label: "音色标识（预留）", type: "text", placeholder: "基础版使用参考音频音色" },
      { key: "cosyvoice_prompt_wav", label: "参考音频", type: "text", browse: "file", placeholder: "至少 3 秒 WAV，用于音色" },
      { key: "cosyvoice_prompt_text", label: "参考音频对应文本", type: "text", placeholder: "参考音频中说的话", hint: "参考音频的逐字转写，须与音频内容一致；方言模式可留空" },
      { key: "cosyvoice_sample_rate", label: "服务输出采样率", type: "number", placeholder: "24000", hint: "FastAPI 返回裸 PCM16；CosyVoice2/3 默认 24000，请与服务模型一致" },
    ],
    ready: (c) => !!c.cosyvoice_url && !!c.cosyvoice_prompt_wav && c.cosyvoice_sample_rate > 0,
  },
  {
    id: "zipvoice",
    label: "ZipVoice Distill INT8（本地零样本）",
    desc: "Rust sherpa-onnx 推理，中英/普通话零样本语音克隆；需提供参考音频及对应文本",
    fields: [
      {
        key: "zipvoice_dir",
        label: "模型目录",
        type: "text",
        browse: "dir",
        placeholder: "例如 E:/models/zipvoice-distill-int8",
        hint: "应含 encoder/decoder.int8.onnx、tokens.txt、lexicon.txt、vocos_24khz.onnx、espeak-ng-data/",
      },
      {
        key: "zipvoice_prompt_wav",
        label: "参考音频",
        type: "text",
        browse: "file",
        extensions: ["wav"],
        placeholder: "例如 E:/voices/ref.wav",
        hint: "至少 3 秒的干净 WAV，用于音色克隆",
      },
      {
        key: "zipvoice_prompt_text",
        label: "参考音频对应文本",
        type: "text",
        placeholder: "参考音频中说的话",
        hint: "参考音频的逐字转写，须与音频内容一致",
      },
      {
        key: "zipvoice_num_threads",
        label: "推理线程数",
        type: "number",
        placeholder: "2",
        hint: "ONNX Runtime CPU 线程数，默认 2",
      },
    ],
    ready: (c) =>
      !!c.zipvoice_dir &&
      !!c.zipvoice_prompt_wav &&
      !!c.zipvoice_prompt_text &&
      c.zipvoice_num_threads > 0,
  },
];

export function engineById(list: EngineDef[], id: string): EngineDef {
  return list.find((e) => e.id === id) ?? list[0];
}

/** 默认配置（与 Rust 端 AppConfig::default 对齐） */
// 模型路径默认值 = 本机已存在的模型目录（首次使用零配置，直接保存即生效）；
// 换机器/移动模型后按 README「模型路径速查表」修改；已保存配置会覆盖这些兜底值。
export const DEFAULT_CONFIG: AppConfig = {
  stt_engine: "sensevoice",
  sensevoice_dir: "E:/projects/test2voices_backup/sense-voice-int8",
  whisper_model_dir: "E:/projects/text2voices/CosyVoice/pretrained_models/whisper-large-v3-turbo",
  whisper_cli_path: "",
  whisper_model_path: "",
  whisper_use_gpu: false,
  openai_key: "",
  deepseek_key: "",
  deepseek_model: "deepseek-chat",
  deepseek_api_url: "https://api.deepseek.com/chat/completions",
  tts_engine: "supertonic",
  supertonic_dir: "E:/projects/pyvideotrans-3.98/Supertone/supertonic-3",
  supertonic_voice: "",
  cosyvoice_url: "http://127.0.0.1:50000",
  cosyvoice_key: "",
  cosyvoice_voice: "",
  cosyvoice_prompt_wav: "",
  cosyvoice_prompt_text: "",
  cosyvoice_sample_rate: 24000,
  zipvoice_dir: "E:/projects/test2voices_backup/sherpa-onnx-zipvoice-distill-int8-zh-en-emilia",
  // 参考音频与对应转写：模型包里 test_wavs/ 自带现成素材（news-female.wav 新闻女声 /
  // leijun-1.wav 男声），提示词必须与音频内容逐字一致（见 test_wavs/prompt.txt）。
  zipvoice_prompt_wav: "E:/projects/test2voices_backup/sherpa-onnx-zipvoice-distill-int8-zh-en-emilia/test_wavs/news-female.wav",
  zipvoice_prompt_text: "各位村民, 大家新年好! 近期, 湖北省武汉市等多个地区",
  zipvoice_num_threads: 2,
  tts_use_video_prompt: false,
  http_proxy: "",
  api_max_concurrent: 1,
  api_interval_ms: 1000,
  separation_enabled: false,
  separator_model_path: "",
  diarization_seg_model: "",
  diarization_embedding_model: "",
  diarization_num_speakers: -1,
  separation_denoise: false,
  separation_normalize: false,
  separation_fallback_no_bgm: true,
  generate_final_videos: true,
  output_naming: "source_variant",
  keep_original_audio_track: false,
  min_speed_percent: 85,
  max_speed_percent: 125,
  subtitle_mode: "none",
  output_dir: "",
};
