//! 日志与崩溃捕获
//!
//! 设计要点：
//! ① 普通日志双输出：stderr（dev 期终端可见）+ 按日期命名的文件（持久留存）
//! ② panic hook 必须自己装：上次崩溃发生在 WebView2 回调里（extern "C" 不能 unwind，
//!    panic 后直接 abort），默认 hook 只写 stderr，窗口一闪就什么都没了。
//!    hook 会在 abort 前执行，所以把「消息 + 位置 + 完整 backtrace」写进 crash.log 并强制刷盘。
//! ③ 不用 async/缓冲型日志框架：崩溃瞬间没有 flush 机会，每条日志直接落盘。

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

use simplelog::{CombinedLogger, LevelFilter, SharedLogger, SimpleLogger, WriteLogger};
use time::OffsetDateTime;

/// 日志目录：%LOCALAPPDATA%\videotrans\logs，取不到时退回系统临时目录
pub fn log_dir() -> PathBuf {
    dirs_next::data_local_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("videotrans")
        .join("logs")
}

/// 截断长字符串供日志使用（按字符数而非字节数，避免 UTF-8 切半）
pub fn record_failure(task: &str, node: &str, stage: &str, error: &str) {
    // 测试进程不写用户真实日志：runner 的 FakeExecutor 失败用例会走
    // record_terminal → record_failure，落真实路径等于每次 cargo test
    // 都往 %LOCALAPPDATA%\videotrans\logs\failures.jsonl 灌假失败记录。
    let dir = if cfg!(test) {
        std::env::temp_dir().join("videotrans-test-logs")
    } else {
        log_dir()
    };
    if let Err(write_error) = write_failure(&dir.join("failures.jsonl"), task, node, stage, error) {
        eprintln!("[logger] 无法写入 failures.jsonl: {write_error}");
    }
}

fn write_failure(
    path: &std::path::Path,
    task: &str,
    node: &str,
    stage: &str,
    error: &str,
) -> std::io::Result<()> {
    let entry = serde_json::json!({
        "time": fmt_time(now(), "[year]-[month]-[day] [hour]:[minute]:[second]"),
        "task": task,
        "node": node,
        "stage": stage,
        "error": error,
    });
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "{entry}")?;
    file.flush()
}

pub fn snippet(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let head: String = s.chars().take(max_chars).collect();
    format!("{}…", head)
}

fn now() -> OffsetDateTime {
    let offset = time::UtcOffset::current_local_offset().unwrap_or(time::UtcOffset::UTC);
    OffsetDateTime::now_utc().to_offset(offset)
}

fn fmt_time(t: OffsetDateTime, f: &str) -> String {
    let desc = time::format_description::parse(f).expect("time format");
    t.format(&desc).unwrap_or_else(|_| "<time-error>".into())
}

/// 初始化日志系统 + 安装 panic hook，返回日志目录（供命令查询/前端展示）
pub fn init() -> PathBuf {
    // 测试进程（e2e 等）同样不落真实目录：此前 e2e 的 panic 现场
    // 直接把 backtrace 写进用户的 crash.log，严重误导排查。
    let dir = if cfg!(test) {
        std::env::temp_dir().join("videotrans-test-logs")
    } else {
        log_dir()
    };
    if let Err(e) = fs::create_dir_all(&dir) {
        eprintln!("[logger] 无法创建日志目录 {}: {}", dir.display(), e);
    }

    let level = if cfg!(debug_assertions) {
        LevelFilter::Debug
    } else {
        LevelFilter::Info
    };
    // simplelog 默认用 UTC，统一改成本地时区，和文件系统时间对得上
    let mut cb = simplelog::ConfigBuilder::new();
    cb.set_time_offset(time::UtcOffset::current_local_offset().unwrap_or(time::UtcOffset::UTC));
    // 屏蔽依赖库的事件循环/连接噪音（曾在 47 分钟任务里刷了几千行）
    for noisy in [
        "tao::",
        "wry::",
        "reqwest::",
        "hyper::",
        "rustls::",
        "webview2",
    ] {
        cb.add_filter_ignore_str(noisy);
    }
    let config = cb.build();

    let mut loggers: Vec<Box<dyn SharedLogger>> = vec![SimpleLogger::new(level, config.clone())];

    let file_name = format!("videotrans-{}.log", fmt_time(now(), "[year][month][day]"));
    match OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(&file_name))
    {
        Ok(f) => loggers.push(WriteLogger::new(level, config, f)),
        Err(e) => eprintln!("[logger] 无法打开日志文件: {}", e),
    }
    let _ = CombinedLogger::init(loggers);

    install_panic_hook(dir.join("crash.log"));
    dir
}

/// 崩溃捕获：任何线程 panic 都会先走到这里，写完 crash.log 再交给默认 hook
fn install_panic_hook(crash_path: PathBuf) {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let msg = info
            .payload()
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| info.payload().downcast_ref::<String>().map(String::as_str))
            .unwrap_or("<non-string payload>");
        let loc = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "<unknown>".into());
        // force_capture：不依赖 RUST_BACKTRACE 环境变量，崩溃现场一定有栈
        let bt = std::backtrace::Backtrace::force_capture();

        let entry = format!(
            "\n===== {} PANIC =====\nmessage : {}\nlocation: {}\nbacktrace:\n{}\n",
            fmt_time(now(), "[year]-[month]-[day] [hour]:[minute]:[second]"),
            msg,
            loc,
            bt
        );
        if let Ok(mut f) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&crash_path)
        {
            let _ = f.write_all(entry.as_bytes());
            let _ = f.sync_all(); // 进程可能马上 abort，必须越过 OS 缓存直接落盘
        }
        log::error!("PANIC at {}: {}", loc, msg);
        default_hook(info); // 保持 stderr 上的标准 panic 输出
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failure_index_is_append_only_json_lines() {
        let root = std::env::temp_dir().join(format!("failure-log-{}", uuid::Uuid::new_v4()));
        let path = root.join("failures.jsonl");
        write_failure(&path, "task-1", "target:zh:tts", "tts", "missing model").unwrap();
        write_failure(&path, "task-2", "parent:stt", "stt", "empty audio").unwrap();
        let lines: Vec<_> = std::fs::read_to_string(&path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["task"], "task-1");
        assert_eq!(lines[0]["node"], "target:zh:tts");
        assert_eq!(lines[0]["error"], "missing model");
        std::fs::remove_dir_all(root).unwrap();
    }
}
