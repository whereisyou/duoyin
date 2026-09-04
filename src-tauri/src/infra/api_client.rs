use std::collections::BTreeMap;
use std::fmt;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use reqwest::{Method, StatusCode};

use crate::scheduler::{self, ResourceCost};

#[derive(Debug, Clone, Copy)]
/// 本地 API 执行能力（scheduler::admit_local_api 已实现，当前翻译适配器只用 Remote）
#[allow(dead_code)]
pub enum ApiExecution {
    Remote,
    Local { cost: ResourceCost },
}

#[derive(Debug, Clone)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub base_delay: Duration,
    pub max_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            base_delay: Duration::from_millis(250),
            max_delay: Duration::from_secs(5),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ApiRequest {
    pub provider_id: String,
    pub execution: ApiExecution,
    pub method: Method,
    pub url: String,
    /// header 值只用于发送，永不进入 Debug/日志。
    pub headers: BTreeMap<String, String>,
    pub body: serde_json::Value,
    pub deadline: Duration,
    pub log_label: String,
}

#[derive(Debug, Clone)]
/// 重试/鉴权排查字段（响应日志用，当前重试策略默认关闭所以不读）
#[allow(dead_code)]
pub struct ApiResponse {
    pub status: StatusCode,
    pub body: String,
    pub trace_id: String,
    pub attempts: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiErrorKind {
    InvalidRequest,
    DeadlineExceeded,
    Network,
    Http(u16),
}

#[derive(Debug, Clone)]
/// 错误分类字段（当前错误正文直接透传，分类留作上层策略）
#[allow(dead_code)]
pub struct ApiError {
    pub kind: ApiErrorKind,
    pub message: String,
    pub trace_id: String,
    pub attempts: u32,
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} [trace_id={}, attempts={}]",
            self.message, self.trace_id, self.attempts
        )
    }
}

impl std::error::Error for ApiError {}

#[derive(Debug, Clone)]
pub struct ApiClient {
    http: reqwest::Client,
    max_concurrent: usize,
    interval_ms: u64,
    retry: RetryPolicy,
}

impl ApiClient {
    /// 本地服务（Ollama 等）/ 严格重试策略构造，当前未启用，保留
    #[allow(dead_code)]
    pub fn new(max_concurrent: usize, interval_ms: u64) -> Result<Self, ApiError> {
            
        Self::new_with_proxy(max_concurrent, interval_ms, None)
    }

    pub fn new_with_proxy(
        max_concurrent: usize,
        interval_ms: u64,
        proxy: Option<&str>,
    ) -> Result<Self, ApiError> {
        let trace_id = new_trace_id();
        let mut builder = reqwest::Client::builder();
        if let Some(proxy) = proxy.filter(|value| !value.trim().is_empty()) {
            builder =
                builder.proxy(reqwest::Proxy::all(proxy.trim()).map_err(|error| ApiError {
                    kind: ApiErrorKind::InvalidRequest,
                    message: format!("代理地址无效: {error}"),
                    trace_id: trace_id.clone(),
                    attempts: 0,
                })?);
        }
        let http = builder.build().map_err(|error| ApiError {
            kind: ApiErrorKind::InvalidRequest,
            message: format!("创建 HTTP 客户端失败: {error}"),
            trace_id,
            attempts: 0,
        })?;
        Ok(Self {
            http,
            max_concurrent: max_concurrent.max(1),
            interval_ms,
            retry: RetryPolicy::default(),
        })
    }

    #[allow(dead_code)]
    pub fn with_retry_policy(mut self, retry: RetryPolicy) -> Self {
        self.retry = retry;
        self
    }

    pub async fn execute(&self, request: ApiRequest) -> Result<ApiResponse, ApiError> {
        let trace_id = new_trace_id();
        let parsed_url = reqwest::Url::parse(request.url.trim()).map_err(|error| ApiError {
            kind: ApiErrorKind::InvalidRequest,
            message: format!("API URL 不合法: {error}"),
            trace_id: trace_id.clone(),
            attempts: 0,
        })?;
        if !matches!(parsed_url.scheme(), "http" | "https") || parsed_url.host_str().is_none() {
            return Err(ApiError {
                kind: ApiErrorKind::InvalidRequest,
                message: "API URL 必须是含主机名的 http/https 地址".into(),
                trace_id,
                attempts: 0,
            });
        }

        let started = Instant::now();
        let body_bytes = request.body.to_string().len();
        let host = parsed_url.host_str().unwrap_or("unknown").to_owned();
        let max_attempts = self.retry.max_attempts.max(1);

        for attempt in 1..=max_attempts {
            let remaining = request.deadline.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                return Err(deadline_error(trace_id, attempt.saturating_sub(1)));
            }

            let _admission = match request.execution {
                ApiExecution::Remote => ApiAdmission::Remote(
                    scheduler::admit_api(self.max_concurrent, self.interval_ms).await,
                ),
                ApiExecution::Local { cost } => ApiAdmission::Local(
                    scheduler::admit_local_api(cost, self.max_concurrent, self.interval_ms).await,
                ),
            };

            log::info!(
                "[api:{}] → {} {} host={} provider={} attempt={}/{} body={}B trace_id={}",
                request.log_label,
                request.method,
                parsed_url.path(),
                host,
                request.provider_id,
                attempt,
                max_attempts,
                body_bytes,
                trace_id,
            );
            let attempt_started = Instant::now();
            let mut builder = self
                .http
                .request(request.method.clone(), parsed_url.clone())
                .json(&request.body);
            for (name, value) in &request.headers {
                builder = builder.header(name, value);
            }

            let response = match tokio::time::timeout(remaining, builder.send()).await {
                Err(_) => return Err(deadline_error(trace_id, attempt)),
                Ok(Err(error)) => {
                    log::warn!(
                        "[api:{}] 网络错误 attempt={} elapsed={}ms trace_id={}: {}",
                        request.log_label,
                        attempt,
                        attempt_started.elapsed().as_millis(),
                        trace_id,
                        error,
                    );
                    if attempt >= max_attempts {
                        return Err(ApiError {
                            kind: ApiErrorKind::Network,
                            message: format!("API 网络请求失败: {error}"),
                            trace_id,
                            attempts: attempt,
                        });
                    }
                    sleep_with_deadline(
                        retry_delay(&self.retry, attempt, None, &trace_id),
                        started,
                        request.deadline,
                        &trace_id,
                        attempt,
                    )
                    .await?;
                    continue;
                }
                Ok(Ok(response)) => response,
            };

            let status = response.status();
            let retry_after =
                parse_retry_after(response.headers().get(reqwest::header::RETRY_AFTER));
            let remaining = request.deadline.saturating_sub(started.elapsed());
            let body = match tokio::time::timeout(remaining, response.text()).await {
                Err(_) => return Err(deadline_error(trace_id, attempt)),
                Ok(Err(error)) => {
                    return Err(ApiError {
                        kind: ApiErrorKind::Network,
                        message: format!("读取 API 响应失败: {error}"),
                        trace_id,
                        attempts: attempt,
                    })
                }
                Ok(Ok(body)) => body,
            };
            log::info!(
                "[api:{}] ← status={} elapsed={}ms response={}B trace_id={}",
                request.log_label,
                status.as_u16(),
                attempt_started.elapsed().as_millis(),
                body.len(),
                trace_id,
            );

            if status.is_success() {
                return Ok(ApiResponse {
                    status,
                    body,
                    trace_id,
                    attempts: attempt,
                });
            }

            if !is_retryable_status(status) || attempt >= max_attempts {
                return Err(ApiError {
                    kind: ApiErrorKind::Http(status.as_u16()),
                    message: format!(
                        "API HTTP {}: {}",
                        status.as_u16(),
                        crate::logger::snippet(&body, 200)
                    ),
                    trace_id,
                    attempts: attempt,
                });
            }
            sleep_with_deadline(
                retry_delay(&self.retry, attempt, retry_after, &trace_id),
                started,
                request.deadline,
                &trace_id,
                attempt,
            )
            .await?;
        }

        unreachable!("attempt loop always returns")
    }
}

enum ApiAdmission {
    Remote(scheduler::ApiLease),
    /// 本地执行租约（与 ApiExecution::Local 配套，当前未启用）
    #[allow(dead_code)]
    Local(scheduler::LocalApiLease),
}

fn is_retryable_status(status: StatusCode) -> bool {
    status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error()
}

fn parse_retry_after(value: Option<&reqwest::header::HeaderValue>) -> Option<Duration> {
    value?
        .to_str()
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
        .map(Duration::from_secs)
}

fn retry_delay(
    policy: &RetryPolicy,
    attempt: u32,
    retry_after: Option<Duration>,
    trace_id: &str,
) -> Duration {
    if let Some(value) = retry_after {
        return value;
    }
    let exponent = attempt.saturating_sub(1).min(16);
    let multiplier = 1u32 << exponent;
    let base = policy.base_delay.saturating_mul(multiplier);
    let jitter_ceiling = (base.as_millis() / 4).max(1) as u64;
    let jitter = trace_id.bytes().fold(attempt as u64, |state, byte| {
        state.wrapping_mul(31).wrapping_add(byte as u64)
    }) % jitter_ceiling;
    base.saturating_add(Duration::from_millis(jitter))
        .min(policy.max_delay)
}

async fn sleep_with_deadline(
    delay: Duration,
    started: Instant,
    deadline: Duration,
    trace_id: &str,
    attempts: u32,
) -> Result<(), ApiError> {
    let remaining = deadline.saturating_sub(started.elapsed());
    if delay >= remaining {
        return Err(deadline_error(trace_id.to_owned(), attempts));
    }
    tokio::time::sleep(delay).await;
    Ok(())
}

fn deadline_error(trace_id: String, attempts: u32) -> ApiError {
    ApiError {
        kind: ApiErrorKind::DeadlineExceeded,
        message: "API 请求超过全链路 deadline".into(),
        trace_id,
        attempts,
    }
}

fn new_trace_id() -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_nanos())
        .unwrap_or_default();
    format!("api-{timestamp:x}-{}", std::process::id())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    fn mock_server(
        responses: Vec<(
            &'static str,
            Vec<(&'static str, &'static str)>,
            &'static str,
        )>,
    ) -> (String, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_thread = calls.clone();
        std::thread::spawn(move || {
            for (status, headers, body) in responses {
                let (mut stream, _) = listener.accept().unwrap();
                let mut buffer = [0u8; 4096];
                let _ = stream.read(&mut buffer);
                calls_thread.fetch_add(1, Ordering::SeqCst);
                let mut response = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n",
                    body.len()
                );
                for (name, value) in headers {
                    response.push_str(&format!("{name}: {value}\r\n"));
                }
                response.push_str("\r\n");
                response.push_str(body);
                stream.write_all(response.as_bytes()).unwrap();
            }
        });
        (format!("http://{address}/chat"), calls)
    }

    fn request(url: String) -> ApiRequest {
        ApiRequest {
            provider_id: "test".into(),
            execution: ApiExecution::Remote,
            method: Method::POST,
            url,
            headers: BTreeMap::from([("Authorization".into(), "Bearer secret".into())]),
            body: serde_json::json!({"safe":"value"}),
            deadline: Duration::from_secs(3),
            log_label: "test".into(),
        }
    }

    #[tokio::test]
    async fn retries_500_then_returns_success() {
        let (url, calls) = mock_server(vec![
            ("500 Internal Server Error", vec![], "failed"),
            ("200 OK", vec![], "{\"ok\":true}"),
        ]);
        let client = ApiClient::new(1, 0)
            .unwrap()
            .with_retry_policy(RetryPolicy {
                max_attempts: 2,
                base_delay: Duration::from_millis(1),
                max_delay: Duration::from_millis(10),
            });
        let response = client.execute(request(url)).await.unwrap();
        assert_eq!(response.attempts, 2);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn does_not_retry_normal_4xx() {
        let (url, calls) = mock_server(vec![("400 Bad Request", vec![], "bad")]);
        let client = ApiClient::new(1, 0).unwrap();
        let error = client.execute(request(url)).await.unwrap_err();
        assert_eq!(error.kind, ApiErrorKind::Http(400));
        assert_eq!(error.attempts, 1);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn retry_after_is_bounded_by_deadline() {
        let (url, calls) = mock_server(vec![(
            "429 Too Many Requests",
            vec![("Retry-After", "5")],
            "slow down",
        )]);
        let mut req = request(url);
        req.deadline = Duration::from_millis(100);
        let client = ApiClient::new(1, 0).unwrap();
        let error = client.execute(req).await.unwrap_err();
        assert_eq!(error.kind, ApiErrorKind::DeadlineExceeded);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn retry_policy_only_retries_429_and_5xx() {
        assert!(is_retryable_status(StatusCode::TOO_MANY_REQUESTS));
        assert!(is_retryable_status(StatusCode::BAD_GATEWAY));
        assert!(!is_retryable_status(StatusCode::UNAUTHORIZED));
        assert!(!is_retryable_status(StatusCode::NOT_FOUND));
    }
}
