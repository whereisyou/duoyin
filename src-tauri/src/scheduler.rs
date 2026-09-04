//! 资源感知调度器 —— 全应用唯一知道「资源」的地方
//!
//! 设计（三模型会诊后定稿，权衡记录）：
//! - 不选中央 actor：资源维度只有两类（CPU 重活 / API 等待），并发才 2-3 个任务，
//!   actor 会多一个序列化瓶颈；「先占 CPU 许可再查内存」的顺序已避免死锁。
//! - 流水线只声明每个阶段的成本（Cost），调度器决定何时准入——这就是解耦：
//!   pipeline 不认识信号量，scheduler 不认识流水线。
//! - API 阶段（翻译等外部调用）成本为零直接放行：A 任务等 API 时，
//!   B 任务的重资源阶段自然被准入，无需任何显式编排。

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::memcheck;

/// 阶段资源画像
#[derive(Clone, Copy, Debug)]
pub struct Cost {
    /// CPU 重资源许可数（0 = 纯 IO/API 阶段，不占用调度）
    pub cpu: u32,
    /// 预估峰值内存（准入前按 commit 可用校验）
    pub ram_bytes: u64,
}

/// 多维资源画像。资源获取顺序固定为 CPU → GPU → process，避免交叉等待死锁。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ResourceCost {
    pub cpu_slots: u32,
    pub gpu_slots: u32,
    pub process_slots: u32,
    pub ram_bytes: u64,
    /// 预计新增磁盘占用；由 application 层结合任务卷做预审。
    pub disk_bytes: u64,
}

impl From<Cost> for ResourceCost {
    fn from(value: Cost) -> Self {
        Self {
            cpu_slots: value.cpu,
            process_slots: value.cpu.min(1),
            ram_bytes: value.ram_bytes,
            ..Self::default()
        }
    }
}

/// 纯 IO / 轻量阶段（translate、extract、split、srt）：不占用调度，直接放行
/// 预留的统一入口——目前轻量阶段不走 admit() 以省一行日志
#[allow(dead_code)]
pub const LIGHT: Cost = Cost {
    cpu: 0,
    ram_bytes: 0,
};

/// STT 成本按引擎区分（candle f32 是大象，sensevoice int8 很轻）
pub fn stt(engine: &str) -> Cost {
    match engine {
        "sensevoice" => Cost {
            cpu: 1,
            ram_bytes: 1200 * memcheck::MB,
        },
        // whisper_native（candle f32）/ whisper_local 等
        _ => Cost {
            cpu: 1,
            ram_bytes: 3900 * memcheck::MB,
        },
    }
}

/// TTS（ONNX 会话 + 波形缓冲）
pub const TTS: Cost = Cost {
    cpu: 1,
    ram_bytes: 1200 * memcheck::MB,
};

/// 背景音分离（预留：UVR-MDX-NET onnx ~67MB + STFT 缓冲）
#[allow(dead_code)]
pub const SEPARATE: Cost = Cost {
    cpu: 1,
    ram_bytes: 800 * memcheck::MB,
};

/// 重 CPU 阶段全局闸口：内存敏感的机器上，STT/TTS/分离 任一时刻只跑一个。
/// 许可数将来可按机型上调（配合 ram_bytes 校验防 OOM）。
static CPU: once_cell::sync::Lazy<Arc<tokio::sync::Semaphore>> =
    once_cell::sync::Lazy::new(|| Arc::new(tokio::sync::Semaphore::new(1)));
static GPU: once_cell::sync::Lazy<Arc<tokio::sync::Semaphore>> =
    once_cell::sync::Lazy::new(|| Arc::new(tokio::sync::Semaphore::new(1)));
static PROCESS: once_cell::sync::Lazy<Arc<tokio::sync::Semaphore>> =
    once_cell::sync::Lazy::new(|| Arc::new(tokio::sync::Semaphore::new(2)));

#[derive(Debug, Default)]
struct ApiState {
    in_flight: usize,
    tokens: f64,
    last_refill: Option<Instant>,
    configured_interval: Duration,
}

static API: once_cell::sync::Lazy<Mutex<ApiState>> =
    once_cell::sync::Lazy::new(|| Mutex::new(ApiState::default()));

/// 资源租约：RAII，panic/取消/正常结束出作用域即自动释放许可
pub struct Lease {
    _cpu: Option<tokio::sync::OwnedSemaphorePermit>,
    _gpu: Option<tokio::sync::OwnedSemaphorePermit>,
    _process: Option<tokio::sync::OwnedSemaphorePermit>,
}

/// 申请准入。cpu=0 的阶段立即放行；
/// 重资源阶段先占 CPU 许可（FIFO 排队位），再校验 commit 可用内存，
/// 不够则退避重试——宁可等待也不让 alloc abort 杀死进程。
pub async fn admit(cost: Cost) -> Lease {
    admit_resources(cost.into()).await
}

pub async fn admit_resources(cost: ResourceCost) -> Lease {
    loop {
        let cpu = acquire_slots(CPU.clone(), cost.cpu_slots).await;
        let gpu = acquire_slots(GPU.clone(), cost.gpu_slots).await;
        let process = acquire_slots(PROCESS.clone(), cost.process_slots).await;
        match memcheck::commit_available_bytes() {
            Some(avail) if avail < cost.ram_bytes => {
                log::warn!(
                    "[scheduler] commit 不足（需 {}MB / 可用 {}MB），退避等待",
                    cost.ram_bytes / memcheck::MB,
                    avail / memcheck::MB
                );
                drop(process);
                drop(gpu);
                drop(cpu);
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
            _ => {
                log::info!(
                    "[scheduler] 准入 cpu={} gpu={} process={} ram={}MB disk={}MB",
                    cost.cpu_slots,
                    cost.gpu_slots,
                    cost.process_slots,
                    cost.ram_bytes / memcheck::MB,
                    cost.disk_bytes / memcheck::MB,
                );
                return Lease {
                    _cpu: cpu,
                    _gpu: gpu,
                    _process: process,
                };
            }
        }
    }
}

async fn acquire_slots(
    semaphore: Arc<tokio::sync::Semaphore>,
    slots: u32,
) -> Option<tokio::sync::OwnedSemaphorePermit> {
    if slots == 0 {
        None
    } else {
        Some(
            semaphore
                .acquire_many_owned(slots)
                .await
                .expect("scheduler semaphore closed"),
        )
    }
}

/// API 请求租约：Drop 时释放 in-flight 计数
pub struct ApiLease;

impl Drop for ApiLease {
    fn drop(&mut self) {
        if let Ok(mut s) = API.lock() {
            s.in_flight = s.in_flight.saturating_sub(1);
        }
    }
}

/// 外部 API 准入：限制并发数 + 限制请求启动间隔。
/// 这里控制的是“请求开始时间”，不会强行 sleep 已经发出的请求。
pub struct LocalApiLease {
    _resources: Lease,
    _api: ApiLease,
}

/// localhost Ollama/CosyVoice 等同时消耗 API 配额和本地重资源。
pub async fn admit_local_api(
    cost: ResourceCost,
    max_concurrent: usize,
    interval_ms: u64,
) -> LocalApiLease {
    // 固定先拿本地资源，再拿 API；等待 API 时仍保留资源，避免另一任务抢占后反向死锁。
    // LocalApi 并发通常为 1，等待时间由 interval 限制且有上界。
    let resources = admit_resources(cost).await;
    let api = admit_api(max_concurrent, interval_ms).await;
    LocalApiLease {
        _resources: resources,
        _api: api,
    }
}

pub async fn admit_api(max_concurrent: usize, interval_ms: u64) -> ApiLease {
    let max_concurrent = max_concurrent.max(1);
    let interval = Duration::from_millis(interval_ms);
    loop {
        let wait = {
            let mut s = API.lock().expect("API limiter poisoned");
            let now = Instant::now();
            if s.configured_interval != interval {
                s.tokens = 1.0;
                s.last_refill = Some(now);
                s.configured_interval = interval;
            }
            if interval.is_zero() {
                s.tokens = 1.0;
                s.last_refill = Some(now);
            } else if let Some(last) = s.last_refill {
                let refill = now.duration_since(last).as_secs_f64() / interval.as_secs_f64();
                s.tokens = (s.tokens + refill).min(1.0);
                s.last_refill = Some(now);
            } else {
                s.tokens = 1.0;
                s.last_refill = Some(now);
            }
            if s.in_flight < max_concurrent && s.tokens >= 1.0 {
                s.in_flight += 1;
                s.tokens -= 1.0;
                log::debug!(
                    "[scheduler:api] 准入 in_flight={}/{} interval={}ms",
                    s.in_flight,
                    max_concurrent,
                    interval_ms
                );
                return ApiLease;
            }
            let token_wait = if interval.is_zero() {
                Duration::ZERO
            } else {
                interval.mul_f64((1.0 - s.tokens).max(0.0))
            };
            if s.in_flight >= max_concurrent {
                token_wait.max(Duration::from_millis(50))
            } else {
                token_wait.max(Duration::from_millis(1))
            }
        };
        tokio::time::sleep(wait).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cost_profiles() {
        assert_eq!(LIGHT.cpu, 0);
        assert!(stt("sensevoice").ram_bytes < stt("whisper_native").ram_bytes);
        assert_eq!(stt("sensevoice").cpu, 1);
        assert_eq!(TTS.cpu, 1);
        let local: ResourceCost = TTS.into();
        assert_eq!(local.cpu_slots, 1);
        assert_eq!(local.process_slots, 1);
        assert_eq!(local.ram_bytes, TTS.ram_bytes);
    }

    #[tokio::test]
    async fn local_api_holds_local_resource_lease() {
        reset_api_state();
        use std::sync::atomic::{AtomicUsize, Ordering};
        static ACTIVE: AtomicUsize = AtomicUsize::new(0);
        static MAX: AtomicUsize = AtomicUsize::new(0);
        let work = || async {
            let _lease = admit_local_api(
                ResourceCost {
                    cpu_slots: 1,
                    process_slots: 1,
                    ..ResourceCost::default()
                },
                2,
                0,
            )
            .await;
            let current = ACTIVE.fetch_add(1, Ordering::SeqCst) + 1;
            MAX.fetch_max(current, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(20)).await;
            ACTIVE.fetch_sub(1, Ordering::SeqCst);
        };
        tokio::join!(work(), work());
        assert_eq!(MAX.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_light_admits_immediately() {
        // 零成本阶段不占信号量：连续取两个也能同时成立
        let _a = admit(LIGHT).await;
        let _b = admit(LIGHT).await;
    }

    fn reset_api_state() {
        let mut s = API.lock().unwrap();
        s.in_flight = 0;
        s.tokens = 1.0;
        s.last_refill = None;
        s.configured_interval = Duration::ZERO;
    }

    #[tokio::test]
    async fn test_api_interval_is_enforced_between_request_starts() {
        reset_api_state();
        let _a = admit_api(2, 80).await;
        let t0 = Instant::now();
        let _b = admit_api(2, 80).await;
        assert!(t0.elapsed() >= Duration::from_millis(60));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_heavy_mutual_exclusion() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);
        static MAX_SEEN: AtomicUsize = AtomicUsize::new(0);

        let work = || async {
            // ram=0：只验证互斥，不依赖本机内存状况
            let _lease = admit(Cost {
                cpu: 1,
                ram_bytes: 0,
            })
            .await;
            let n = IN_FLIGHT.fetch_add(1, Ordering::SeqCst) + 1;
            MAX_SEEN.fetch_max(n, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(50)).await;
            IN_FLIGHT.fetch_sub(1, Ordering::SeqCst);
        };
        tokio::join!(work(), work(), work());
        assert_eq!(MAX_SEEN.load(Ordering::SeqCst), 1, "重资源阶段必须互斥");
    }
}
