//! API 连通性测试命令：chat/completions 端点与通用可达性探测。

use std::sync::Mutex;

use crate::logger;
use crate::scheduler;
use crate::types::AppConfig;

/// 校验 API 地址：去空格、必须是合法 http/https URL 且含主机名
fn validate_api_url(url: &str) -> Result<&str, String> {
    let url = url.trim();
    let parsed = reqwest::Url::parse(url).map_err(|e| format!("URL 不合法: {}", e))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("仅支持 http/https 地址".into());
    }
    if parsed.host_str().is_none() {
        return Err("URL 缺少主机名".into());
    }
    Ok(url)
}

/// 测试 API 端点连通性（OpenAI 兼容 chat/completions 接口通用）
/// 发送最小化请求（max_tokens=1），返回耗时；网络/鉴权/模型错误原样上报
#[tauri::command]
pub async fn test_api_endpoint(
    state: tauri::State<'_, Mutex<AppConfig>>,
    url: String,
    api_key: String,
    model: String,
) -> Result<String, String> {
    let url = validate_api_url(&url)?;
    let cfg = state.lock().map_err(|e| e.to_string())?.clone();
    let _api = scheduler::admit_api(cfg.api_max_concurrent, cfg.api_interval_ms).await;

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?;

    let body = serde_json::json!({
        "model": if model.trim().is_empty() { "default" } else { model.trim() },
        "messages": [{"role": "user", "content": "hi"}],
        "max_tokens": 1,
    });

    log::info!("[api:test] → POST {} model={}", url, body["model"]);
    let start = std::time::Instant::now();
    let resp = client
        .post(url)
        .header("Authorization", format!("Bearer {}", api_key.trim()))
        .json(&body)
        .send()
        .await
        .map_err(|e| {
            log::error!("[api:test] 连接失败: {}", e);
            format!("连接失败: {}", e)
        })?;

    let ms = start.elapsed().as_millis();
    let status = resp.status();
    if status.is_success() {
        log::info!("[api:test] ← {} {}ms", status.as_u16(), ms);
        Ok(format!("{} ms", ms))
    } else {
        let text = resp.text().await.unwrap_or_default();
        log::error!(
            "[api:test] ← {} {}ms: {}",
            status.as_u16(),
            ms,
            logger::snippet(&text, 300)
        );
        Err(format!(
            "HTTP {}：{}",
            status.as_u16(),
            logger::snippet(&text, 200)
        ))
    }
}

/// 通用可达性测试（适用于非 chat 接口：Whisper/CosyVoice 等）：
/// 发 GET（带可选 Bearer），只要服务器回了任何 HTTP 响应就算「通路」（DNS+TLS+存活）；
/// 只有连接层失败（DNS/TLS/拒连）才算不通。状态码原样呈报，让用户自己判鉴权。
#[tauri::command]
pub async fn test_api_reachable(
    state: tauri::State<'_, Mutex<AppConfig>>,
    url: String,
    api_key: String,
) -> Result<String, String> {
    let url = validate_api_url(&url)?;
    let cfg = state.lock().map_err(|e| e.to_string())?.clone();
    let _api = scheduler::admit_api(cfg.api_max_concurrent, cfg.api_interval_ms).await;
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?;

    let mut req = client.get(url);
    if !api_key.trim().is_empty() {
        req = req.header("Authorization", format!("Bearer {}", api_key.trim()));
    }

    log::info!("[api:reachable] → GET {}", url);
    let start = std::time::Instant::now();
    let resp = req.send().await.map_err(|e| {
        log::error!("[api:reachable] 连接失败: {}", e);
        format!("连接失败: {}", e)
    })?;
    let ms = start.elapsed().as_millis();
    let status = resp.status().as_u16();
    log::info!("[api:reachable] ← {} {}ms", status, ms);
    // 任何 HTTP 响应都证明通路；2xx/3xx 更好，4xx 多为鉴权/路径问题但网络是通的
    if (200..400).contains(&status) {
        Ok(format!("通路正常 · HTTP {} · {}ms", status, ms))
    } else {
        Ok(format!(
            "可达（HTTP {}，请核对地址/鉴权）· {}ms",
            status, ms
        ))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_validate_api_url() {
        // 前后空格自动去除
        assert_eq!(
            super::validate_api_url("  https://api.deepseek.com/chat/completions  ").unwrap(),
            "https://api.deepseek.com/chat/completions"
        );
        // 合法 http
        assert!(super::validate_api_url("http://localhost:8080/v1/chat").is_ok());
        // 非 http/https 拒绝
        assert!(super::validate_api_url("ftp://example.com").is_err());
        // 完全不是 URL
        assert!(super::validate_api_url("not a url").is_err());
        // 空串
        assert!(super::validate_api_url("   ").is_err());
    }
}
