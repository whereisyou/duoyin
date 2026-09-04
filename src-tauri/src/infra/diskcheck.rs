use std::path::Path;

pub const SAFETY_MARGIN: u64 = 256 * 1024 * 1024;

pub fn estimate_task_bytes(
    source_size: u64,
    target_count: usize,
    separation_enabled: bool,
    final_video_enabled: bool,
) -> u64 {
    let shared_audio = source_size.max(128 * 1024 * 1024);
    let separation = if separation_enabled {
        source_size.saturating_mul(2)
    } else {
        0
    };
    let per_target_audio = source_size / 2 + 64 * 1024 * 1024;
    let per_target_video = if final_video_enabled { source_size } else { 0 };
    shared_audio
        .saturating_add(separation)
        .saturating_add(
            (per_target_audio.saturating_add(per_target_video))
                .saturating_mul(target_count.max(1) as u64),
        )
        .saturating_add(SAFETY_MARGIN)
}

#[cfg(windows)]
pub fn available_bytes(path: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    wide.push(0);
    let mut available = 0u64;
    unsafe {
        GetDiskFreeSpaceExW(PCWSTR(wide.as_ptr()), Some(&mut available), None, None).ok()?;
    }
    Some(available)
}

#[cfg(not(windows))]
pub fn available_bytes(_path: &Path) -> Option<u64> {
    None
}

pub fn ensure_capacity(path: &Path, required: u64) -> Result<(), String> {
    if let Some(available) = available_bytes(path) {
        if available < required {
            return Err(format!(
                "任务磁盘空间不足：预计需要 {:.1} GB，可用 {:.1} GB",
                required as f64 / 1024.0 / 1024.0 / 1024.0,
                available as f64 / 1024.0 / 1024.0 / 1024.0,
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimate_grows_with_targets_video_and_separation() {
        let source = 100 * 1024 * 1024;
        let one = estimate_task_bytes(source, 1, false, false);
        let two = estimate_task_bytes(source, 2, false, false);
        let video = estimate_task_bytes(source, 2, false, true);
        let separated = estimate_task_bytes(source, 2, true, true);
        assert!(two > one);
        assert!(video > two);
        assert!(separated > video);
    }

    #[test]
    fn current_project_volume_reports_space_on_windows() {
        #[cfg(windows)]
        assert!(available_bytes(Path::new(".")).is_some());
    }
}
