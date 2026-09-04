//! OpenAI Whisper API 语音识别
//! 排障约定与 api_deepseek 相同：请求/响应双向日志，key 不落盘。

use crate::types::Segment;

/// 解析 verbose_json 响应为字幕段（纯函数，可无网络测试）
fn parse_segments(resp: &serde_json::Value) -> Result<Vec<Segment>, String> {
    let segments = resp["segments"]
        .as_array()
        .ok_or("no segments in response")?;

    segments
        .iter()
        .enumerate()
        .map(|(i, s)| {
            Ok(Segment {
                idx: i,
                start: s["start"].as_f64().ok_or("missing start".to_string())?,
                end: s["end"].as_f64().ok_or("missing end".to_string())?,
                text: s["text"]
                    .as_str()
                    .ok_or("missing text".to_string())?
                    .to_string(),
                translated: String::new(),
            })
        })
        .collect()
}

pub async fn transcribe(
    audio: &std::path::Path,
    lang: &str,
    api_key: &str,
) -> Result<Vec<Segment>, String> {
    let data = tokio::fs::read(audio)
        .await
        .map_err(|e| format!("read audio failed: {}", e))?;

    let form = reqwest::multipart::Form::new()
        .part(
            "file",
            reqwest::multipart::Part::bytes(data)
                .file_name("audio.wav")
                .mime_str("audio/wav")
                .map_err(|e| e.to_string())?,
        )
        .text("model", "whisper-1")
        .text("language", lang.to_owned())
        .text("response_format", "verbose_json");

    let url = "https://api.openai.com/v1/audio/transcriptions";
    let file_kb = audio.metadata().map(|m| m.len() / 1024).unwrap_or(0);
    log::info!("[api:stt] → POST {} lang={} file={}KB", url, lang, file_kb);
    let t0 = std::time::Instant::now();

    let resp = reqwest::Client::new()
        .post(url)
        .header("Authorization", format!("Bearer {}", api_key.trim()))
        .multipart(form)
        .send()
        .await
        .map_err(|e| {
            log::error!("[api:stt] 连接失败: {}", e);
            format!("request failed: {}", e)
        })?;

    let status = resp.status();
    let text = resp.text().await.map_err(|e| e.to_string())?;
    log::info!(
        "[api:stt] ← {} {}ms {}B",
        status.as_u16(),
        t0.elapsed().as_millis(),
        text.len()
    );
    if !status.is_success() {
        log::error!("[api:stt] 错误响应: {}", crate::logger::snippet(&text, 300));
        return Err(format!(
            "HTTP {}：{}",
            status.as_u16(),
            crate::logger::snippet(&text, 200)
        ));
    }

    let resp: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("parse response failed: {}", e))?;
    let segs = parse_segments(&resp)?;
    log::info!("[api:stt] 解析出 {} 段", segs.len());
    Ok(segs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_segments_happy_path() {
        let resp = serde_json::json!({
            "text": "hello world",
            "segments": [
                {"start": 0.0, "end": 1.5, "text": " hello"},
                {"start": 1.5, "end": 3.0, "text": " world"}
            ]
        });
        let segs = parse_segments(&resp).unwrap();
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].start, 0.0);
        assert_eq!(segs[1].text, " world");
    }

    #[test]
    fn test_parse_segments_missing_segments() {
        let resp = serde_json::json!({"text": "no segments here"});
        assert!(parse_segments(&resp).is_err());
    }

    #[test]
    fn test_parse_segments_missing_field() {
        let resp = serde_json::json!({"segments": [{"start": 0.0}]});
        assert!(parse_segments(&resp).is_err());
    }
}
