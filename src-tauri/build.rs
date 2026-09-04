use std::path::{Path, PathBuf};

fn main() {
    tauri_build::build();
    ensure_onnxruntime_dll();
    sync_runtime_dlls_to_deps();
}

/// cargo test 的测试 exe 在 target/debug/deps/ 下运行，Windows 按 exe 目录
/// 优先查找 DLL——把运行时 DLL 同步一份过去，否则测试进程启动即崩
/// （实测：0xc000007b STATUS_INVALID_IMAGE_FORMAT）
fn sync_runtime_dlls_to_deps() {
    let Ok(out_dir) = std::env::var("OUT_DIR") else { return };
    let Some(profile_dir) = std::path::PathBuf::from(out_dir).ancestors().nth(3).map(|p| p.to_path_buf()) else {
        return;
    };
    let deps = profile_dir.join("deps");
    for name in [
        "onnxruntime.dll",
        "onnxruntime_providers_shared.dll",
        "sherpa-onnx-c-api.dll",
        "sherpa-onnx-cxx-api.dll",
    ] {
        let src = profile_dir.join(name);
        if src.is_file() {
            let _ = std::fs::create_dir_all(&deps);
            let _ = std::fs::copy(&src, deps.join(name));
        }
    }
}

/// ort 采用 load-dynamic（运行时加载 DLL，规避预编译静态库与本机 MSVC 版本不一致的链接冲突）。
/// 这里负责把与 ort-sys 相同版本（ms@1.28.0）的 onnxruntime.dll 放到可执行文件旁。
fn ensure_onnxruntime_dll() {
    println!("cargo:rerun-if-changed=build.rs");
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    // OUT_DIR = target/<profile>/build/<pkg>-<hash>/out → 取 target/<profile>
    let Some(profile_dir) = out_dir.ancestors().nth(3).map(|p| p.to_path_buf()) else {
        return;
    };
    let dll = profile_dir.join("onnxruntime.dll");
    const ORT_VERSION: &str = "1.28.0";
    let url = format!(
        "https://github.com/microsoft/onnxruntime/releases/download/v{0}/onnxruntime-win-x64-{0}.zip",
        ORT_VERSION
    );
    let work = std::env::temp_dir().join(format!("videotrans-ort-{}", ORT_VERSION));
    let zip = work.join("ort.zip");
    let _ = std::fs::create_dir_all(&work);

    // 优先用项目根目录下手动放置的 zip（cargo clean 后重建免于重复下载，网络不稳时兜底）
    let local_zip = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join(format!("onnxruntime-win-x64-{}.zip", ORT_VERSION));
    if local_zip.is_file() {
        // 无条件重新解压覆盖：sherpa-onnx-sys 的 build.rs 会把它自带的旧版
        // onnxruntime.dll 复制到程序目录，我们必须恢复 1.28.0（ORT 向后兼容：
        // sherpa 用旧版构建可在 1.28 上跑，反过来不行）
        if let Err(e) = std::fs::copy(&local_zip, &zip) {
            println!("cargo:warning=复制本地压缩包失败: {}", e);
            return;
        }
    } else if dll.is_file() {
        return; // 无本地 zip 且 dll 已在：不重复下载
    } else {
        println!("cargo:warning=下载 ONNX Runtime {} ...", ORT_VERSION);
        let ok = std::process::Command::new("curl.exe")
            .args(["-fSL", "-o"])
            .arg(&zip)
            .arg(&url)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !ok {
            println!(
                "cargo:warning=下载失败：请手动下载 {} 并解压出 onnxruntime.dll 放到程序目录",
                url
            );
            return;
        }
    }

    // Windows 10+ 自带 bsdtar，可解 zip
    let ok = std::process::Command::new("tar.exe")
        .arg("-xf")
        .arg(&zip)
        .arg("-C")
        .arg(&work)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !ok {
        println!("cargo:warning=解压失败：{}", zip.display());
        return;
    }

    match find_file(&work, "onnxruntime.dll") {
        Some(src) => {
            if let Err(e) = std::fs::copy(&src, &dll) {
                println!("cargo:warning=复制 DLL 失败: {}", e);
            } else {
                println!("cargo:warning=onnxruntime.dll 已就绪 → {}", dll.display());
            }
        }
        None => println!("cargo:warning=压缩包内未找到 onnxruntime.dll"),
    }
}

fn find_file(dir: &Path, name: &str) -> Option<PathBuf> {
    for entry in std::fs::read_dir(dir).ok()? {
        let entry = entry.ok()?;
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_file(&path, name) {
                return Some(found);
            }
        } else if entry.file_name().to_string_lossy().eq_ignore_ascii_case(name) {
            return Some(path);
        }
    }
    None
}
