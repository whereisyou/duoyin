use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::domain::variant::TargetVariant;
use crate::pipeline::runner::CancelToken;
use crate::ports::tts::{
    validate_tts_input, TtsAlignment, TtsEngine, TtsError, TtsFuture, TtsOutput,
};
use crate::scheduler::ResourceCost;
use crate::types::Segment;

/// CosyVoice3 FastAPI 端点
const ENDPOINT_INSTRUCT2: &str = "/inference_instruct2";
const ENDPOINT_ZERO_SHOT: &str = "/inference_zero_shot";

/// 参考音频最小时长（秒）；CosyVoice3 要求至少 3 秒
const MIN_PROMPT_DURATION_SECS: f64 = 3.0;

#[derive(Debug, Clone)]
pub struct CosyVoice3Engine {
    base_url: String,
    api_key: String,
    prompt_wav: PathBuf,
    /// 参考音频的文本内容（zero_shot 模式必需，instruct2 方言模式可留空）
    prompt_text: String,
    sample_rate: u32,
    max_concurrent: usize,
    interval_ms: u64,
    client: reqwest::Client,
}

impl CosyVoice3Engine {
    pub fn new(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        prompt_wav: impl Into<PathBuf>,
        prompt_text: impl Into<String>,
        sample_rate: u32,
        max_concurrent: usize,
        interval_ms: u64,
    ) -> Result<Self, TtsError> {
        if sample_rate == 0 {
            return Err(TtsError::InvalidInput("CosyVoice3 采样率不能为 0".into()));
        }
        Ok(Self {
            base_url: base_url.into().trim_end_matches('/').to_owned(),
            api_key: api_key.into(),
            prompt_wav: prompt_wav.into(),
            prompt_text: prompt_text.into(),
            sample_rate,
            max_concurrent: max_concurrent.max(1),
            interval_ms,
            client: reqwest::Client::builder()
                .build()
                .map_err(|error| TtsError::Engine(error.to_string()))?,
        })
    }

    /// 判断是否走 instruct2（方言模式）还是 zero_shot（普通话/非方言）
    fn is_dialect_mode(target: &TargetVariant) -> bool {
        // 中文且方言不是普通话/空 → instruct2 方言模式
        target.language == "zh"
            && target
                .dialect
                .as_deref()
                .map(|d| d != "mandarin")
                .unwrap_or(false)
    }

    /// 读取参考音频文件，返回原始字节
    async fn read_prompt_wav(&self) -> Result<Vec<u8>, TtsError> {
        tokio::fs::read(&self.prompt_wav)
            .await
            .map_err(|error| {
                TtsError::InvalidInput(format!(
                    "读取 CosyVoice3 参考音频失败: {} ({})",
                    self.prompt_wav.display(),
                    error
                ))
            })
    }

    /// 调用 FastAPI 合成一段语音，返回 PCM16 样本
    async fn synthesize_segment(
        &self,
        text: &str,
        instruct: &str,
        cancel: &CancelToken,
    ) -> Result<Vec<i16>, TtsError> {
        let prompt_bytes = self.read_prompt_wav().await?;

        let cost = ResourceCost {
            cpu_slots: 1,
            gpu_slots: 1,
            process_slots: 1,
            ram_bytes: 2 * 1024 * 1024 * 1024,
            disk_bytes: 0,
        };
        let _lease =
            crate::scheduler::admit_local_api(cost, self.max_concurrent, self.interval_ms).await;

        let (endpoint, form) = if instruct.is_empty() {
            // 无 instruct → zero_shot 模式（用参考音频+文本复刻音色）
            if self.prompt_text.trim().is_empty() {
                return Err(TtsError::InvalidInput(
                    "CosyVoice3 零样本模式需要参考音频文本（cosyvoice_prompt_text），请在设置中填写".into(),
                ));
            }
            let form = reqwest::multipart::Form::new()
                .text("tts_text", text.to_owned())
                .text("prompt_text", self.prompt_text.trim().to_owned())
                .part(
                    "prompt_wav",
                    reqwest::multipart::Part::bytes(prompt_bytes)
                        .file_name("prompt.wav")
                        .mime_str("audio/wav")
                        .map_err(|error| TtsError::Engine(error.to_string()))?,
                );
            (ENDPOINT_ZERO_SHOT, form)
        } else {
            // 有 instruct → instruct2 模式（参考音频提供音色，instruct 控制方言）
            let form = reqwest::multipart::Form::new()
                .text("tts_text", text.to_owned())
                .text("instruct_text", instruct.to_owned())
                .part(
                    "prompt_wav",
                    reqwest::multipart::Part::bytes(prompt_bytes)
                        .file_name("prompt.wav")
                        .mime_str("audio/wav")
                        .map_err(|error| TtsError::Engine(error.to_string()))?,
                );
            (ENDPOINT_INSTRUCT2, form)
        };

        let url = format!("{}{}", self.base_url, endpoint);
        let mut request = self
            .client
            .post(&url)
            .multipart(form)
            .timeout(Duration::from_secs(600));
        if !self.api_key.trim().is_empty() {
            request = request.bearer_auth(self.api_key.trim());
        }

        // 带取消的 HTTP 请求
        let future = request.send();
        tokio::pin!(future);
        let response = loop {
            if cancel.is_canceled() {
                return Err(TtsError::Canceled);
            }
            tokio::select! {
                result = &mut future => {
                    break result.map_err(|error| {
                        TtsError::Engine(format!("CosyVoice3 连接失败 ({url}): {error}"))
                    })?
                }
                _ = tokio::time::sleep(Duration::from_millis(20)) => {}
            }
        };

        let status = response.status();
        let bytes = response
            .bytes()
            .await
            .map_err(|error| TtsError::Engine(format!("CosyVoice3 读取响应失败: {error}")))?;

        if !status.is_success() {
            let body = crate::logger::snippet(&String::from_utf8_lossy(&bytes), 300);
            return Err(TtsError::Engine(format!(
                "CosyVoice3 HTTP {} ({url}): {body}",
                status.as_u16()
            )));
        }

        // 响应可能是裸 PCM16 或带 WAV header 的字节
        parse_pcm_or_wav(&bytes, self.sample_rate)
    }
}

/// 解析服务端返回的音频数据：裸 PCM16 或 WAV（带 header）
fn parse_pcm_or_wav(bytes: &[u8], expected_sample_rate: u32) -> Result<Vec<i16>, TtsError> {
    if bytes.len() < 2 {
        return Err(TtsError::Engine(
            "CosyVoice3 返回的音频数据过短（<2 字节）".into(),
        ));
    }

    // 检测 WAV header: "RIFF" .... "WAVE"
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WAVE" {
        // 解析 WAV：跳过 header 找到 data chunk
        let samples = parse_wav_data_chunk(bytes)?;
        if samples.is_empty() {
            return Err(TtsError::Engine(
                "CosyVoice3 返回的 WAV 不含有效音频数据".into(),
            ));
        }
        log::info!(
            "CosyVoice3 返回 WAV 格式，{} 个样本",
            samples.len()
        );
        return Ok(samples);
    }

    // 裸 PCM16：字节长度必须为偶数
    if bytes.len() % 2 != 0 {
        return Err(TtsError::Engine(format!(
            "CosyVoice3 返回的 PCM16 字节长度为奇数 ({})，数据可能损坏",
            bytes.len()
        )));
    }

    let samples: Vec<i16> = bytes
        .chunks_exact(2)
        .map(|pair| i16::from_le_bytes([pair[0], pair[1]]))
        .collect();

    log::debug!(
        "CosyVoice3 返回裸 PCM16，{} 字节 → {} 个样本（假设 {} Hz）",
        bytes.len(),
        samples.len(),
        expected_sample_rate
    );

    Ok(samples)
}

/// 从 WAV 字节中提取 data chunk 的 PCM16 样本
fn parse_wav_data_chunk(bytes: &[u8]) -> Result<Vec<i16>, TtsError> {
    let mut pos = 12; // 跳过 RIFF header
    while pos + 8 <= bytes.len() {
        let chunk_id = &bytes[pos..pos + 4];
        let chunk_size = u32::from_le_bytes([
            bytes[pos + 4],
            bytes[pos + 5],
            bytes[pos + 6],
            bytes[pos + 7],
        ]) as usize;
        pos += 8;

        if chunk_id == b"data" {
            let end = (pos + chunk_size).min(bytes.len());
            let data = &bytes[pos..end];
            return Ok(data
                .chunks_exact(2)
                .map(|pair| i16::from_le_bytes([pair[0], pair[1]]))
                .collect());
        }
        pos += chunk_size;
        // WAV chunks 按偶数对齐
        if chunk_size % 2 != 0 {
            pos += 1;
        }
    }
    Err(TtsError::Engine(
        "CosyVoice3 WAV 中未找到 data chunk".into(),
    ))
}

impl TtsEngine for CosyVoice3Engine {
    fn version(&self) -> String {
        format!("cosyvoice3-fastapi:{}", self.sample_rate)
    }

    fn synthesize<'a>(
        &'a self,
        segments: &'a [Segment],
        target: &'a TargetVariant,
        output_dir: &'a Path,
        alignment: TtsAlignment,
        cancel: &'a CancelToken,
    ) -> TtsFuture<'a> {
        Box::pin(async move {
            validate_tts_input(segments)?;
            if !self.prompt_wav.is_file() {
                return Err(TtsError::InvalidInput(format!(
                    "CosyVoice3 参考音频不存在: {}",
                    self.prompt_wav.display()
                )));
            }

            // 检查参考音频时长是否 ≥3 秒
            check_prompt_duration(&self.prompt_wav).await?;

            tokio::fs::create_dir_all(output_dir)
                .await
                .map_err(|error| TtsError::Engine(error.to_string()))?;

            let mut timeline =
                crate::tts_dub::TimelineWriter::new(output_dir, self.sample_rate)
                    .map_err(TtsError::Engine)?;

            for segment in segments {
                if cancel.is_canceled() {
                    return Err(TtsError::Canceled);
                }
                let text = segment.translated.trim();
                if text.is_empty() {
                    continue;
                }

                // 确定调用的端点和参数
                let instruct = if Self::is_dialect_mode(target) {
                    target.tts_accent.trim()
                } else {
                    "" // 普通话/非中文 → zero_shot 模式
                };

                // 带重试的合成（可重试错误重试 2 次，退避 2s/4s）
                let samples = self
                    .synthesize_segment_with_retry(text, instruct, cancel)
                    .await?;

                let target_duration = (segment.end - segment.start).max(0.0);
                let actual_duration = samples.len() as f64 / self.sample_rate as f64;
                let samples = crate::tts_dub::align_i16_to_duration(
                    samples,
                    actual_duration,
                    target_duration,
                    alignment.max_speed_percent,
                    self.sample_rate,
                    output_dir,
                    segment.idx,
                )
                .map_err(TtsError::Engine)?;
                timeline
                    .push(segment.start, &samples)
                    .map_err(TtsError::Engine)?;
            }
            let dub_path = timeline.finalize().map_err(TtsError::Engine)?;
            Ok(TtsOutput {
                dub_audio: dub_path,
                segment_dir: None,
            })
        })
    }
}

impl CosyVoice3Engine {
    /// 带重试的段合成
    async fn synthesize_segment_with_retry(
        &self,
        text: &str,
        instruct: &str,
        cancel: &CancelToken,
    ) -> Result<Vec<i16>, TtsError> {
        let max_retries = 2;
        let mut last_error: Option<TtsError> = None;

        for attempt in 0..=max_retries {
            if cancel.is_canceled() {
                return Err(TtsError::Canceled);
            }
            if attempt > 0 {
                let backoff = Duration::from_secs(2u64.pow(attempt as u32)); // 2s, 4s
                log::warn!(
                    "CosyVoice3 合成重试 {}/{}，等待 {:?}",
                    attempt,
                    max_retries,
                    backoff
                );
                tokio::select! {
                    _ = tokio::time::sleep(backoff) => {}
                    _ = async {
                        loop {
                            tokio::time::sleep(Duration::from_millis(50)).await;
                            if cancel.is_canceled() { break; }
                        }
                    } => {
                        if cancel.is_canceled() {
                            return Err(TtsError::Canceled);
                        }
                    }
                }
            }

            match self.synthesize_segment(text, instruct, cancel).await {
                Ok(samples) => return Ok(samples),
                Err(TtsError::Canceled) => return Err(TtsError::Canceled),
                Err(TtsError::InvalidInput(ref _msg)) => {
                    // 配置错误不重试
                    return Err(last_error.unwrap_or_else(|| {
                        TtsError::InvalidInput("CosyVoice3 配置错误".into())
                    }));
                }
                Err(ref e) => {
                    log::warn!(
                        "CosyVoice3 合成失败 (尝试 {}/{}): {}",
                        attempt + 1,
                        max_retries + 1,
                        e
                    );
                    last_error = Some(e.clone());
                }
            }
        }

        Err(last_error.unwrap_or_else(|| {
            TtsError::Engine("CosyVoice3 合成失败，已用尽重试次数".into())
        }))
    }
}

/// 检查参考音频时长是否 ≥3 秒
async fn check_prompt_duration(path: &Path) -> Result<(), TtsError> {
    // 读取文件头部解析 WAV 时长
    let bytes = tokio::fs::read(path)
        .await
        .map_err(|e| TtsError::InvalidInput(format!("读取参考音频失败: {e}")))?;

    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" {
        // 非 WAV 文件，无法检测时长，放行让服务端处理
        log::warn!(
            "CosyVoice3 参考音频 {} 不是标准 WAV，跳过时长检查",
            path.display()
        );
        return Ok(());
    }

    // 解析 WAV fmt chunk 获取采样率和 data chunk 获取数据大小
    let mut pos = 12;
    let mut sample_rate: u32 = 0;
    let mut data_size: usize = 0;
    let mut bits_per_sample: u16 = 16;

    while pos + 8 <= bytes.len() {
        let chunk_id = &bytes[pos..pos + 4];
        let chunk_size = u32::from_le_bytes([
            bytes[pos + 4],
            bytes[pos + 5],
            bytes[pos + 6],
            bytes[pos + 7],
        ]) as usize;
        pos += 8;

        match chunk_id {
            b"fmt " => {
                if pos + 16 <= bytes.len() {
                    sample_rate = u32::from_le_bytes([
                        bytes[pos + 4],
                        bytes[pos + 5],
                        bytes[pos + 6],
                        bytes[pos + 7],
                    ]);
                    bits_per_sample = u16::from_le_bytes([bytes[pos + 14], bytes[pos + 15]]);
                }
            }
            b"data" => {
                data_size = chunk_size;
            }
            _ => {}
        }
        pos += chunk_size;
        if chunk_size % 2 != 0 {
            pos += 1;
        }
    }

    if sample_rate == 0 || data_size == 0 || bits_per_sample == 0 {
        log::warn!("CosyVoice3 参考音频 WAV 解析失败，跳过时长检查");
        return Ok(());
    }

    let bytes_per_sample = (bits_per_sample / 8) as usize;
    if bytes_per_sample == 0 {
        return Ok(());
    }
    let num_samples = data_size / bytes_per_sample;
    let duration = num_samples as f64 / sample_rate as f64;

    if duration < MIN_PROMPT_DURATION_SECS {
        return Err(TtsError::InvalidInput(format!(
            "CosyVoice3 参考音频时长 {:.1}s 不足，需要至少 {}s（文件: {}）",
            duration,
            MIN_PROMPT_DURATION_SECS,
            path.display()
        )));
    }

    log::debug!(
        "CosyVoice3 参考音频: {}s, {} Hz, {} bit",
        duration,
        sample_rate,
        bits_per_sample
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// 构造一个最小的合法 WAV (PCM16, 1ch, 8000Hz, 5s 静音)
    fn make_test_wav(path: &Path) {
        let sample_rate = 8000u32;
        let num_samples = sample_rate as usize * 5; // 5 秒
        let data_size = num_samples * 2; // PCM16
        let mut buf = Vec::new();
        // RIFF header
        buf.extend_from_slice(b"RIFF");
        buf.extend_from_slice(&(36 + data_size as u32).to_le_bytes());
        buf.extend_from_slice(b"WAVE");
        // fmt chunk
        buf.extend_from_slice(b"fmt ");
        buf.extend_from_slice(&16u32.to_le_bytes());
        buf.extend_from_slice(&1u16.to_le_bytes()); // PCM
        buf.extend_from_slice(&1u16.to_le_bytes()); // mono
        buf.extend_from_slice(&sample_rate.to_le_bytes());
        buf.extend_from_slice(&(sample_rate * 2).to_le_bytes()); // byte rate
        buf.extend_from_slice(&2u16.to_le_bytes()); // block align
        buf.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
        // data chunk
        buf.extend_from_slice(b"data");
        buf.extend_from_slice(&(data_size as u32).to_le_bytes());
        buf.extend(std::iter::repeat(0u8).take(data_size));
        std::fs::write(path, &buf).unwrap();
    }

    #[tokio::test]
    async fn instruct2_pcm_response_becomes_valid_wav() {
        let root = std::env::temp_dir().join(format!("cosyvoice3-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let prompt = root.join("prompt.wav");
        make_test_wav(&prompt);

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            // 循环接受连接以支持重试
            for _ in 0..3 {
                let Ok((mut stream, _)) = listener.accept() else { 
                    break 
                };
                // 读取完整请求（循环读到无数据可读）
                let mut request = Vec::new();
                stream.set_read_timeout(Some(std::time::Duration::from_millis(200))).ok();
                loop {
                    let mut buf = [0u8; 4096];
                    match stream.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => request.extend_from_slice(&buf[..n]),
                    }
                    if request.len() > 200_000 { break; } // 安全上限
                }
                let request = String::from_utf8_lossy(&request);
                assert!(request.contains("inference_instruct2"));
                assert!(request.contains("instruct_text"));
                let pcm: Vec<u8> = [0i16, 100, -100, 50]
                    .into_iter()
                    .flat_map(i16::to_le_bytes)
                    .collect();
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    pcm.len()
                )
                .unwrap();
                stream.write_all(&pcm).unwrap();
            }
        });

        let engine = CosyVoice3Engine::new(
            format!("http://{address}"),
            "",
            &prompt,
            "",
            22050,
            1,
            0,
        )
        .unwrap();
        let output = engine
            .synthesize(
                &[Segment {
                    idx: 0,
                    start: 0.0,
                    end: 1.0,
                    text: "你好".into(),
                    translated: "你好".into(),
                }],
                &TargetVariant::zh_dialect("yue", "粤语", "请用广东话表达。"),
                &root.join("out"),
                TtsAlignment {
                    min_speed_percent: 85,
                    max_speed_percent: 125,
                },
                &CancelToken::default(),
            )
            .await
            .unwrap();
        let reader = hound::WavReader::open(output.dub_audio).unwrap();
        assert_eq!(reader.spec().sample_rate, 22050);
        assert_eq!(reader.into_samples::<i16>().count(), 4);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn zero_shot_mode_uses_correct_endpoint() {
        let root = std::env::temp_dir().join(format!("cosyvoice3-zs-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let prompt = root.join("prompt.wav");
        make_test_wav(&prompt);

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for _ in 0..3 {
                let Ok((mut stream, _)) = listener.accept() else { break };
                let mut request = Vec::new();
                stream.set_read_timeout(Some(std::time::Duration::from_millis(200))).ok();
                loop {
                    let mut buf = [0u8; 4096];
                    match stream.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => request.extend_from_slice(&buf[..n]),
                    }
                    if request.len() > 200_000 { break; }
                }
                let request = String::from_utf8_lossy(&request);
                // 普通话模式应走 zero_shot 端点，包含 prompt_text
                assert!(
                    request.contains("inference_zero_shot"),
                    "expected zero_shot endpoint, got: {}",
                    &request[..200.min(request.len())]
                );
                assert!(request.contains("prompt_text"));
                assert!(!request.contains("instruct_text"));
                let pcm: Vec<u8> = [0i16, 50, -50]
                    .into_iter()
                    .flat_map(i16::to_le_bytes)
                    .collect();
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    pcm.len()
                )
                .unwrap();
                stream.write_all(&pcm).unwrap();
            }
        });

        let engine = CosyVoice3Engine::new(
            format!("http://{address}"),
            "",
            &prompt,
            "这是参考音频的文本内容",
            24000,
            1,
            0,
        )
        .unwrap();
        // 普通话（非方言）→ zero_shot 模式
        let output = engine
            .synthesize(
                &[Segment {
                    idx: 0,
                    start: 0.0,
                    end: 1.0,
                    text: "你好世界".into(),
                    translated: "你好世界".into(),
                }],
                &TargetVariant::zh_mandarin(),
                &root.join("out"),
                TtsAlignment {
                    min_speed_percent: 85,
                    max_speed_percent: 125,
                },
                &CancelToken::default(),
            )
            .await
            .unwrap();
        let reader = hound::WavReader::open(output.dub_audio).unwrap();
        assert_eq!(reader.spec().sample_rate, 24000);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn short_prompt_audio_rejected() {
        let root = std::env::temp_dir().join(format!("cosyvoice3-short-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let prompt = root.join("short.wav");

        // 构造一个 1 秒的 WAV（不够 3 秒要求）
        let sample_rate = 8000u32;
        let num_samples = sample_rate as usize; // 1 秒
        let data_size = num_samples * 2;
        let mut buf = Vec::new();
        buf.extend_from_slice(b"RIFF");
        buf.extend_from_slice(&(36 + data_size as u32).to_le_bytes());
        buf.extend_from_slice(b"WAVE");
        buf.extend_from_slice(b"fmt ");
        buf.extend_from_slice(&16u32.to_le_bytes());
        buf.extend_from_slice(&1u16.to_le_bytes());
        buf.extend_from_slice(&1u16.to_le_bytes());
        buf.extend_from_slice(&sample_rate.to_le_bytes());
        buf.extend_from_slice(&(sample_rate * 2).to_le_bytes());
        buf.extend_from_slice(&2u16.to_le_bytes());
        buf.extend_from_slice(&16u16.to_le_bytes());
        buf.extend_from_slice(b"data");
        buf.extend_from_slice(&(data_size as u32).to_le_bytes());
        buf.extend(std::iter::repeat(0u8).take(data_size));
        std::fs::write(&prompt, &buf).unwrap();

        let engine = CosyVoice3Engine::new(
            "http://127.0.0.1:59999", // 不会连接
            "",
            &prompt,
            "参考文本",
            24000,
            1,
            0,
        )
        .unwrap();
        let result = engine
            .synthesize(
                &[Segment {
                    idx: 0,
                    start: 0.0,
                    end: 1.0,
                    text: "测试".into(),
                    translated: "测试".into(),
                }],
                &TargetVariant::zh_mandarin(),
                &root.join("out"),
                TtsAlignment {
                    min_speed_percent: 85,
                    max_speed_percent: 125,
                },
                &CancelToken::default(),
            )
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("3") || err.contains("时长"),
            "error should mention duration: {err}"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn parse_wav_with_header() {
        // 构造一个包含 4 个样本的 WAV
        let samples: Vec<i16> = vec![100, -200, 300, -400];
        let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        wav.extend_from_slice(b"WAVE");
        wav.extend_from_slice(b"fmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
        wav.extend_from_slice(&1u16.to_le_bytes()); // mono
        wav.extend_from_slice(&8000u32.to_le_bytes());
        wav.extend_from_slice(&16000u32.to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&(data.len() as u32).to_le_bytes());
        wav.extend_from_slice(&data);

        let parsed = parse_pcm_or_wav(&wav, 8000).unwrap();
        assert_eq!(parsed, samples);
    }

    #[test]
    fn parse_bare_pcm16() {
        let samples: Vec<i16> = vec![100, -200, 300];
        let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        let parsed = parse_pcm_or_wav(&bytes, 24000).unwrap();
        assert_eq!(parsed, samples);
    }

    #[test]
    fn dialect_mode_detection() {
        // 普通话 → 非方言模式
        assert!(!CosyVoice3Engine::is_dialect_mode(&TargetVariant::zh_mandarin()));
        // 粤语 → 方言模式
        assert!(CosyVoice3Engine::is_dialect_mode(&TargetVariant::zh_dialect("yue", "粤语", "请用广东话表达。")));
        // 英语 → 非方言模式
        assert!(!CosyVoice3Engine::is_dialect_mode(&TargetVariant::language("en").unwrap()));
    }
}
