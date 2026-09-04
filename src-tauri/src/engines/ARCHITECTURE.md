# engines/ —— 裸引擎命名空间

> 职责一句话：纯推理/纯 API 的「裸引擎」实现，不含 port 语义（无资源成本声明、无 stage 逻辑）——那些在 `adapters/` 注入。
> 全局位置见 `docs/ARCHITECTURE.md`；**engines 不依赖 ports/adapters**（实测零违规）。

## 1. 引擎清单

| 类别 | 文件 | 依赖 | 说明 |
|---|---|---|---|
| STT | `stt/sensevoice.rs` | sherpa-onnx（shared） | SenseVoice-Small int8 ~245MB；token 级时间戳 + ITN 标点；目录含 `silero_vad.onnx` 时启用 VAD 门控（防开头幻觉） |
| STT | `stt/whisper_native.rs` | candle（feature= inference） | large-v3-turbo f32；质量顶格但 ~3.5GB 内存，memcheck 常拒绝；30s 窗流式 + LID 复用 |
| STT | `stt/openai_api.rs` | reqwest | OpenAI 兼容 /audio/transcriptions |
| STT | `stt/whisper_cli.rs` | 子进程 | whisper.cpp CLI 包装 |
| 翻译 | `translate/deepseek.rs` | reqwest | DeepSeek/OpenAI 兼容 chat completions；分批 + JSON 对齐 |
| TTS | `tts/supertonic/{mod,helper,assets}.rs` | onnxruntime | Supertonic 3（英文等 31 语言）；`assets.rs` 校验目录/中文扩展缺失 |
| TTS | （zipvoice / cosyvoice3 在 adapters/tts/ 内联） | sherpa-onnx / HTTP | ZipVoice 直连 sherpa `OfflineTts`（无需 engines 层）；CosyVoice3 走远程 HTTP |

## 2. 接入新引擎（三步 + 一行成本）

以「新增 STT 引擎 X」为例：

1. **engines 加实现**：`engines/stt/x.rs`，导出 `transcribe(...) -> Result<Vec<Segment>, _>` 裸函数/结构体（对齐现有引擎风格）。
2. **adapters 加接线**：`ConfiguredSttEngine`（`adapters/stt/legacy.rs`）的 match 加分支 `"x" => ...`。
3. **前端加数据**：`src/lib/engines.ts` 的 `STT_ENGINES` 加一条（key/label/fields/ready 函数——数据驱动 UI，不加组件）。
4. **成本一行**：`scheduler::stt()` 的 match 若内存画像不同则加档。

TTS 引擎同理但注册点在 `pipeline_service::register_tts`。翻译引擎在 `adapters/translate/`。

## 3. 已知约束（勿踩）

- sherpa-onnx 必须 `features = ["shared"]`（静态库 MSVC 版本不匹配会链接失败）。
- sherpa-onnx-sys 的 build.rs 会用旧 onnxruntime.dll 覆盖程序目录——本项目 build.rs 无条件从本地 zip 恢复 1.28.0（ORT 向后兼容旧构建跑新 dll，反之不行）。
- candle whisper：位置嵌入从 0 起算（无 KV cache 偏移）→ 解码每步喂完整前缀；`final_linear` 必须传 `[1,1,dim]`；`pcm_to_mel` 帧数须 narrow 回真实值。
- f16 safetensors 按 f32 加载 = 常驻内存翻倍（1.6GB→3.2GB）。

## 4. 对外契约

- 上游只有 `adapters/`（和 legacy/，冻结区）；公开 API 签名视为冻结，改动需评审。
- 引擎不做资源准入（那是 adapters+scheduler 的事）、不落盘产物格式决策（那是 stage 的事）。
