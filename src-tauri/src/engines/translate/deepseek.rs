//! DeepSeek（OpenAI 兼容）字幕翻译
//! 排障约定：请求前记入参摘要，响应记状态/耗时/字节数，
//! 失败时记响应体片段 —— 外部接口问题全部可回查（API key 永不落日志）。

use crate::types::Segment;

/// 解析模型返回的 JSON 内容并回填译文（纯函数，可无网络测试）
pub(crate) fn parse_translated(
    content: &str,
    segments: &[Segment],
) -> Result<Vec<Segment>, String> {
    let value: serde_json::Value =
        serde_json::from_str(content).map_err(|e| format!("模型返回不是合法 JSON: {}", e))?;

    // 兼容三类常见返回：
    // 1) [{idx, translated}, ...]（旧提示词要求）
    // 2) {"translations":[...]}（json_object 正确形态）
    // 3) {"idx":0,"translated":"..."}（模型只回单段时常见）
    let items: Vec<serde_json::Value> = match value {
        serde_json::Value::Array(a) => a,
        serde_json::Value::Object(mut o) => {
            if o.contains_key("idx") && o.contains_key("translated") {
                vec![serde_json::Value::Object(o)]
            } else {
                let arr = o
                    .remove("translations")
                    .or_else(|| o.remove("items"))
                    .or_else(|| o.remove("results"))
                    .or_else(|| o.remove("data"))
                    .ok_or("模型返回缺少 translations/items/results/data 数组")?;
                arr.as_array()
                    .ok_or("模型返回的 translations 不是数组")?
                    .clone()
            }
        }
        _ => return Err("模型返回 JSON 类型不支持，应为对象或数组".into()),
    };

    let mut out = segments.to_vec();
    let mut filled = 0usize;
    for item in items {
        let idx = item["idx"].as_u64().ok_or("返回项缺少 idx 字段")? as usize;
        let text = item["translated"]
            .as_str()
            .ok_or("返回项缺少 translated 字段")?;
        if let Some(s) = out.iter_mut().find(|s| s.idx == idx) {
            s.translated = text.to_string();
            filled += 1;
        }
    }
    if filled < segments.len() {
        log::warn!(
            "[api:translate] 模型只回填了 {}/{} 段，其余保留原文",
            filled,
            segments.len()
        );
    }
    Ok(out)
}

/// 调用 OpenAI 兼容 chat/completions 接口翻译字幕
pub async fn translate(
    segments: &[Segment],
    source: &str,
    target: &str,
    api_key: &str,
    model: &str,
    api_url: &str,
) -> Result<Vec<Segment>, String> {
    let lines: Vec<String> = segments
        .iter()
        .map(|s| format!("[{}] {}", s.idx, s.text))
        .collect();

    let body = serde_json::json!({
        "model": model,
        "messages": [
            {
                "role": "system",
                "content": format!(
                    "将以下字幕从 {} 翻译为 {}。\
                     返回 JSON 对象，格式 {{\"translations\":[{{\"idx\":0,\"translated\":\"...\"}}]}}。\
                     数量和顺序必须与输入一致。只返回 JSON，不要其他内容。",
                    source, target
                ),
            },
            {
                "role": "user",
                "content": lines.join("\n"),
            },
        ],
        "response_format": {"type": "json_object"},
    });

    let url = if api_url.trim().is_empty() {
        "https://api.deepseek.com/chat/completions"
    } else {
        api_url.trim()
    };

    log::info!(
        "[api:translate] → POST {} model={} segments={} body={}B",
        url,
        model,
        segments.len(),
        body.to_string().len()
    );
    let t0 = std::time::Instant::now();

    let resp = reqwest::Client::new()
        .post(url)
        .header("Authorization", format!("Bearer {}", api_key.trim()))
        .json(&body)
        .send()
        .await
        .map_err(|e| {
            log::error!("[api:translate] 连接失败: {}", e);
            format!("request failed: {}", e)
        })?;

    let status = resp.status();
    let text = resp.text().await.map_err(|e| e.to_string())?;
    log::info!(
        "[api:translate] ← {} {}ms {}B",
        status.as_u16(),
        t0.elapsed().as_millis(),
        text.len()
    );
    if !status.is_success() {
        // 报错三要素齐全：状态码 + 请求的模型名 + 主机，一眼定位是谁拒绝了谁
        let host = reqwest::Url::parse(url)
            .ok()
            .and_then(|u| u.host_str().map(String::from))
            .unwrap_or_else(|| url.to_string());
        log::error!(
            "[api:translate] 错误响应: {}",
            crate::logger::snippet(&text, 300)
        );
        return Err(format!(
            "HTTP {} [模型 {} @ {}]：{}",
            status.as_u16(),
            model,
            host,
            crate::logger::snippet(&text, 200)
        ));
    }

    let resp: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("parse response failed: {}", e))?;
    let content = resp["choices"][0]["message"]["content"]
        .as_str()
        .ok_or("no content in response")?;
    log::debug!(
        "[api:translate] 模型原文: {}",
        crate::logger::snippet(content, 500)
    );

    parse_translated(content, segments)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segs() -> Vec<Segment> {
        vec![
            Segment {
                idx: 0,
                start: 0.0,
                end: 1.0,
                text: "hello".into(),
                translated: String::new(),
            },
            Segment {
                idx: 1,
                start: 1.0,
                end: 2.0,
                text: "world".into(),
                translated: String::new(),
            },
        ]
    }

    #[test]
    fn test_parse_translated_happy_path() {
        let content = r#"[{"idx":0,"translated":"你好"},{"idx":1,"translated":"世界"}]"#;
        let out = parse_translated(content, &segs()).unwrap();
        assert_eq!(out[0].translated, "你好");
        assert_eq!(out[1].translated, "世界");
    }

    #[test]
    fn test_parse_translated_partial_fill_keeps_original() {
        // 模型漏了一段：不报错，该段 translated 保持空（写 SRT 时回退原文）
        let content = r#"[{"idx":0,"translated":"你好"}]"#;
        let out = parse_translated(content, &segs()).unwrap();
        assert_eq!(out[0].translated, "你好");
        assert_eq!(out[1].translated, "");
    }

    #[test]
    fn test_parse_translated_wrapped_object() {
        let content =
            r#"{"translations":[{"idx":0,"translated":"你好"},{"idx":1,"translated":"世界"}]}"#;
        let out = parse_translated(content, &segs()).unwrap();
        assert_eq!(out[0].translated, "你好");
        assert_eq!(out[1].translated, "世界");
    }

    #[test]
    fn test_parse_translated_single_object() {
        let content = r#"{"idx":0,"translated":"在很久很久以前"}"#;
        let out = parse_translated(content, &segs()).unwrap();
        assert_eq!(out[0].translated, "在很久很久以前");
        assert_eq!(out[1].translated, "");
    }

    #[test]
    fn test_parse_translated_bad_json() {
        assert!(parse_translated("这不是JSON", &segs()).is_err());
    }

    #[test]
    fn test_parse_translated_missing_field() {
        assert!(parse_translated(r#"[{"idx":0}]"#, &segs()).is_err());
    }
}
