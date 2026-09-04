use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use reqwest::Method;

use crate::domain::variant::TargetVariant;
use crate::infra::api_client::{ApiClient, ApiExecution, ApiRequest};
use crate::pipeline::runner::CancelToken;
use crate::ports::translator::{validate_translation, TranslateError, TranslateFuture, Translator};
use crate::scheduler::ResourceCost;
use crate::types::Segment;

#[derive(Debug, Clone)]
pub struct OpenAiCompatibleTranslator {
    api_key: String,
    model: String,
    api_url: String,
    execution: ApiExecution,
    client: Arc<ApiClient>,
}

impl OpenAiCompatibleTranslator {
    /// 以下构造器：new_with_limits=new+限流，new_local=本地服务走 ApiExecution::Local 租约。
    /// 当前适配器装配走 with_client 直连 Remote，这些保留为复用/本地服务入口。
    #[allow(dead_code)]
    pub fn new(
        api_key: impl Into<String>,
        model: impl Into<String>,
        api_url: impl Into<String>,
    ) -> Result<Self, TranslateError> {
        Self::new_with_limits(api_key, model, api_url, 1, 1000)
    }

    #[allow(dead_code)]
    pub fn new_with_limits(
        api_key: impl Into<String>,
        model: impl Into<String>,
        api_url: impl Into<String>,
        max_concurrent: usize,
        interval_ms: u64,
    ) -> Result<Self, TranslateError> {
        Self::new_with_limits_and_proxy(api_key, model, api_url, max_concurrent, interval_ms, None)
    }

    pub fn new_with_limits_and_proxy(
        api_key: impl Into<String>,
        model: impl Into<String>,
        api_url: impl Into<String>,
        max_concurrent: usize,
        interval_ms: u64,
        proxy: Option<&str>,
    ) -> Result<Self, TranslateError> {
        Self::with_client(
            api_key,
            model,
            api_url,
            ApiExecution::Remote,
            ApiClient::new_with_proxy(max_concurrent, interval_ms, proxy)
                .map(Arc::new)
                .map_err(|error| TranslateError::Engine(error.to_string()))?,
        )
    }

    #[allow(dead_code)]
    pub fn new_local(
        api_key: impl Into<String>,
        model: impl Into<String>,
        api_url: impl Into<String>,
        cost: ResourceCost,
        max_concurrent: usize,
        interval_ms: u64,
    ) -> Result<Self, TranslateError> {
        Self::with_client(
            api_key,
            model,
            api_url,
            ApiExecution::Local { cost },
            ApiClient::new(max_concurrent, interval_ms)
                .map(Arc::new)
                .map_err(|error| TranslateError::Engine(error.to_string()))?,
        )
    }

    pub fn with_client(
        api_key: impl Into<String>,
        model: impl Into<String>,
        api_url: impl Into<String>,
        execution: ApiExecution,
        client: Arc<ApiClient>,
    ) -> Result<Self, TranslateError> {
        let api_url = api_url.into();
        let api_url = if api_url.trim().is_empty() {
            "https://api.deepseek.com/chat/completions".into()
        } else {
            api_url
        };
        Ok(Self {
            api_key: api_key.into(),
            model: model.into(),
            api_url,
            execution,
            client,
        })
    }
}

impl Translator for OpenAiCompatibleTranslator {
    fn version(&self) -> String {
        format!("openai-compatible:{}", self.model)
    }

    fn translate<'a>(
        &'a self,
        segments: &'a [Segment],
        source_language: Option<&'a str>,
        target: &'a TargetVariant,
        cancel: &'a CancelToken,
    ) -> TranslateFuture<'a> {
        Box::pin(async move {
            if cancel.is_canceled() {
                return Err(TranslateError::Canceled);
            }
            if segments.is_empty() {
                return Err(TranslateError::InvalidInput("没有待翻译字幕段".into()));
            }
            let target_prompt = if target.translate_style.trim().is_empty() {
                target.display_name.clone()
            } else {
                format!(
                    "{}；用词风格：{}",
                    target.display_name, target.translate_style
                )
            };
            let lines: Vec<String> = segments
                .iter()
                .map(|segment| format!("[{}] {}", segment.idx, segment.text))
                .collect();
            let body = serde_json::json!({
                "model": self.model,
                "messages": [
                    {
                        "role": "system",
                        "content": format!(
                            "将以下字幕从 {} 翻译为 {}。返回 JSON 对象，格式 \
                             {{\"translations\":[{{\"idx\":0,\"translated\":\"...\"}}]}}。\
                             数量和顺序必须与输入一致。只返回 JSON。",
                            source_language.unwrap_or("auto"), target_prompt
                        ),
                    },
                    {"role": "user", "content": lines.join("\n")},
                ],
                "response_format": {"type": "json_object"},
            });
            let request = self.client.execute(ApiRequest {
                provider_id: "openai-compatible-translate".into(),
                execution: self.execution,
                method: Method::POST,
                url: self.api_url.clone(),
                headers: BTreeMap::from([(
                    "Authorization".into(),
                    format!("Bearer {}", self.api_key.trim()),
                )]),
                body,
                deadline: Duration::from_secs(120),
                log_label: "translate".into(),
            });
            tokio::pin!(request);
            let response = loop {
                if cancel.is_canceled() {
                    return Err(TranslateError::Canceled);
                }
                tokio::select! {
                    result = &mut request => break result.map_err(|error| TranslateError::Engine(error.to_string()))?,
                    _ = tokio::time::sleep(Duration::from_millis(20)) => {}
                }
            };
            let envelope: serde_json::Value = serde_json::from_str(&response.body)
                .map_err(|error| TranslateError::Engine(format!("解析 API 响应失败: {error}")))?;
            let content = envelope["choices"][0]["message"]["content"]
                .as_str()
                .ok_or_else(|| TranslateError::Engine("API 响应缺少 message.content".into()))?;
            let translated = crate::engines::translate::deepseek::parse_translated(content, segments)
                .map_err(TranslateError::Engine)?;
            validate_translation(segments, &translated)?;
            Ok(translated)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn canceled_request_does_not_touch_network() {
        let translator =
            OpenAiCompatibleTranslator::new("secret", "model", "http://127.0.0.1:1").unwrap();
        let cancel = CancelToken::default();
        cancel.cancel();
        assert!(matches!(
            translator
                .translate(
                    &[Segment {
                        idx: 0,
                        start: 0.0,
                        end: 1.0,
                        text: "hello".into(),
                        translated: String::new(),
                    }],
                    None,
                    &TargetVariant::zh_mandarin(),
                    &cancel,
                )
                .await,
            Err(TranslateError::Canceled)
        ));
    }
}
