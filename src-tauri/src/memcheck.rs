//! 内存预审与窗口自适应
//!
//! 背景（实测踩坑）：
//! - candle 加载 f16 safetensors 会转成 f32 → 常驻内存 ≈ 模型文件 ×2（1.6GB → 3.2GB）
//! - encoder 自注意力 30s 窗口单次分配 [1,20,1500,1500] f32 = 180MB
//! - alloc 失败不走 panic hook，直接 abort（无法 try/catch）
//! 所以唯一安全的做法是开工前预判：commit 可用内存够不够，不够就降窗口或拒绝。

pub const MB: u64 = 1024 * 1024;
const GB: u64 = 1024 * MB;

/// Windows：commit 可用字节数（GlobalMemoryStatusEx.ullAvailPageFile）。
/// 注意：物理可用内存（sysinfo 的 available_memory）不是正确的判据——
/// Windows 上 malloc/VirtualAlloc 的闸门是 commit 限额（RAM + 页面文件）。
#[cfg(windows)]
pub fn commit_available_bytes() -> Option<u64> {
    use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut s = MEMORYSTATUSEX {
        dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    unsafe { GlobalMemoryStatusEx(&mut s).ok()? };
    Some(s.ullAvailPageFile)
}

#[cfg(not(windows))]
pub fn commit_available_bytes() -> Option<u64> {
    None
}

/// 选择 STT 推理窗口（秒）。model_file_bytes 取模型文件实际大小（f16 → f32 ×2）。
/// 30s 窗口注意力峰值 180MB，15s 窗口降到 ~45MB；都不够用就给出可操作报错。
pub fn plan_window(model_file_bytes: u64, avail: Option<u64>) -> Result<usize, String> {
    let Some(avail) = avail else {
        return Ok(30); // 查不到就不拦（非 Windows / 调用失败）
    };
    // 常驻：f32 权重 ≈ 文件×2；加载期最大单张量（词表嵌入 265MB）+ WebView/系统余量
    let base = model_file_bytes * 2 + 565 * MB;
    let spike30 = 180 * MB;
    let spike15 = 45 * MB;
    if avail >= base + spike30 {
        Ok(30)
    } else if avail >= base + spike15 {
        Ok(15)
    } else {
        Err(format!(
            "内存不足：Whisper 推理预计需要约 {:.1}GB 可用（当前仅 {:.1}GB）。\
             建议：关闭其他程序 / 增大系统页面文件，\
             或后续切换到 whisper.cpp 量化引擎（内存占用约 1/3）",
            (base + spike15) as f64 / GB as f64,
            avail as f64 / GB as f64,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MODEL: u64 = 1617824864; // large-v3-turbo 实际文件大小

    #[test]
    fn test_plan_window_tiers() {
        // 阈值（精确）：base = MODEL×2 + 565MB ≈ 3651MB
        // 30s 窗口要求 avail ≥ base+180MB ≈ 3831MB；15s 要求 ≥ base+45MB ≈ 3696MB
        assert_eq!(plan_window(MODEL, Some(8 * GB)).unwrap(), 30);
        assert_eq!(plan_window(MODEL, Some(3850 * MB)).unwrap(), 30);
        // 只够 15s 窗口
        assert_eq!(plan_window(MODEL, Some(3750 * MB)).unwrap(), 15);
        assert_eq!(plan_window(MODEL, Some(3700 * MB)).unwrap(), 15);
        // 不足 → 明确报错而非崩溃
        assert!(plan_window(MODEL, Some(3 * GB)).is_err());
        assert!(plan_window(MODEL, Some(3500 * MB)).is_err());
        // 查不到可用内存 → 不拦
        assert_eq!(plan_window(MODEL, None).unwrap(), 30);
    }
}
