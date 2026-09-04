use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

use crate::domain::artifact::{ArtifactKind, ArtifactRecord, ArtifactStatus};
use crate::domain::ids::VariantId;
use sha2::{Digest, Sha256};

const STAGING_DIR: &str = ".staging";
const TEMP_SUFFIX: &str = ".tmp";

#[derive(Debug, Clone, PartialEq, Eq)]
/// 产物作用域（共享/目标）——当前写入路径统一任务目录内，保留作未来外置共享产物
#[allow(dead_code)]
pub enum ArtifactScope {
    Shared,
    Target(VariantId),
}

/// 路径安全校验层（重解析点/越界/变体名校验）。当前未被生产调用——
/// 写入路径重构后集中在 executor 的 commit_file，本层保留作安全纵深（FUNCTION_CHECKLIST 已登记）。
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub enum ArtifactPathError {
    Empty,
    Absolute,
    ParentTraversal,
    Prefix,
    CurrentDirectory,
    ReservedInternalPath,
    InvalidWindowsName,
    InvalidVariantId,
    ReparsePoint,
    NotDirectory,
    EscapesRoot,
    Io(String),
}

impl fmt::Display for ArtifactPathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::Empty => "产物路径不能为空",
            Self::Absolute => "产物路径不能是绝对路径",
            Self::ParentTraversal => "产物路径不能包含父目录跳转",
            Self::Prefix => "产物路径不能包含磁盘或 UNC 前缀",
            Self::CurrentDirectory => "产物路径不能包含当前目录分量",
            Self::ReservedInternalPath => "产物路径不能使用内部 staging/tmp 名称",
            Self::InvalidWindowsName => "产物路径包含 Windows 不安全名称",
            Self::InvalidVariantId => "目标版本 ID 不能包含路径分隔符或特殊目录名",
            Self::ReparsePoint => "产物路径不能经过符号链接、junction 或 reparse point",
            Self::NotDirectory => "产物父路径不是目录",
            Self::EscapesRoot => "产物路径解析后逃逸任务目录",
            Self::Io(message) => return f.write_str(message),
        };
        f.write_str(message)
    }
}

impl std::error::Error for ArtifactPathError {}

#[derive(Debug, Clone, PartialEq, Eq)]
/// staging 布局（事务式写入用）——当前提交路径不走它，保留
#[allow(dead_code)]
pub struct StagingLayout {
    pub transaction_id: String,
    pub root: PathBuf,
}

impl StagingLayout {
    #[allow(dead_code)]
    pub fn path_for(&self, relative_path: &Path) -> Result<PathBuf, ArtifactPathError> {
        validate_relative_path(relative_path)?;
        Ok(self.root.join(relative_path))
    }
}

#[derive(Debug, Clone)]
pub struct ArtifactStore {
    root: PathBuf,
}

impl ArtifactStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    // 以下安全路径 API 与 StagingLayout 同属安全纵深/预留面（当前写入走 executor commit_file），
    // 已在 FUNCTION_CHECKLIST 登记待接线复查。
    #[allow(dead_code)]
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn resolve(&self, relative_path: &Path) -> Result<PathBuf, ArtifactPathError> {
        validate_relative_path(relative_path)?;
        Ok(self.root.join(relative_path))
    }

    /// 创建目标父目录并逐级拒绝 symlink/junction/reparse point。
    /// 返回值只代表路径边界安全，不代表文件内容已经提交到 Manifest。
    #[allow(dead_code)]
    pub fn prepare_target(&self, relative_path: &Path) -> Result<PathBuf, ArtifactPathError> {
        validate_relative_path(relative_path)?;
        fs::create_dir_all(&self.root).map_err(ArtifactPathError::from_io)?;
        reject_reparse_point(&self.root)?;
        let canonical_root = fs::canonicalize(&self.root).map_err(ArtifactPathError::from_io)?;

        let parent = relative_path.parent().ok_or(ArtifactPathError::Empty)?;
        let mut current = self.root.clone();
        for component in parent.components() {
            let Component::Normal(name) = component else {
                return Err(ArtifactPathError::ParentTraversal);
            };
            current.push(name);
            match fs::symlink_metadata(&current) {
                Ok(metadata) => {
                    if is_reparse_or_symlink(&metadata) {
                        return Err(ArtifactPathError::ReparsePoint);
                    }
                    if !metadata.is_dir() {
                        return Err(ArtifactPathError::NotDirectory);
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    fs::create_dir(&current).map_err(ArtifactPathError::from_io)?;
                }
                Err(error) => return Err(ArtifactPathError::from_io(error)),
            }

            let canonical = fs::canonicalize(&current).map_err(ArtifactPathError::from_io)?;
            if !canonical.starts_with(&canonical_root) {
                return Err(ArtifactPathError::EscapesRoot);
            }
        }

        Ok(self.root.join(relative_path))
    }

    #[allow(dead_code)]
    pub fn scoped_path(
        &self,
        scope: &ArtifactScope,
        file_name: &Path,
    ) -> Result<PathBuf, ArtifactPathError> {
        validate_relative_path(file_name)?;
        let relative = match scope {
            ArtifactScope::Shared => PathBuf::from("shared").join(file_name),
            ArtifactScope::Target(variant) => {
                validate_variant_id(&variant.0)?;
                PathBuf::from("targets").join(&variant.0).join(file_name)
            }
        };
        self.resolve(&relative)
    }

    /// 临时文件与目标文件位于同一目录，因此最终 rename 必然同卷。
    #[allow(dead_code)]
    pub fn temp_path_for(&self, target: &Path) -> Result<PathBuf, ArtifactPathError> {
        let relative = target
            .strip_prefix(&self.root)
            .map_err(|_| ArtifactPathError::Absolute)?;
        validate_relative_path(relative)?;

        let file_name = target
            .file_name()
            .ok_or(ArtifactPathError::Empty)?
            .to_string_lossy();
        Ok(target.with_file_name(format!("{file_name}{TEMP_SUFFIX}")))
    }

    /// staging 固定建在任务根目录内，保证和所有目标产物同卷。
    #[allow(dead_code)]
    pub fn staging_layout(&self, transaction_id: &str) -> Result<StagingLayout, ArtifactPathError> {
        validate_internal_name(transaction_id)?;
        Ok(StagingLayout {
            transaction_id: transaction_id.to_owned(),
            root: self.root.join(STAGING_DIR).join(transaction_id),
        })
    }

    #[allow(dead_code)]
    pub fn staging_root(&self) -> PathBuf {
        self.root.join(STAGING_DIR)
    }

    #[allow(dead_code)]
    pub fn is_orphan_candidate(&self, path: &Path) -> bool {
        path.starts_with(&self.root)
            && (path.starts_with(self.staging_root())
                || path
                    .file_name()
                    .and_then(OsStr::to_str)
                    .map(|name| name.ends_with(TEMP_SUFFIX))
                    .unwrap_or(false))
    }

    /// 读前廉价校验：路径安全、文件存在、size/mtime 一致。
    /// content_hash 只在元数据变化或调用方要求严格校验时重算，避免大媒体反复读取。
    pub fn inspect(&self, artifact: &ArtifactRecord) -> ArtifactStatus {
        let Ok(path) = self.resolve(&artifact.relative_path) else {
            return ArtifactStatus::Invalidated;
        };
        let Ok(metadata) = fs::metadata(path) else {
            return ArtifactStatus::Missing;
        };
        if !metadata.is_file() {
            return ArtifactStatus::Invalidated;
        }
        let modified = metadata.modified().ok().and_then(system_time_millis);
        if metadata.len() != artifact.size || modified != Some(artifact.modified) {
            ArtifactStatus::Stale
        } else {
            ArtifactStatus::Valid
        }
    }

    pub fn hash_file(&self, relative_path: &Path) -> Result<String, ArtifactPathError> {
        let path = self.resolve(relative_path)?;
        let mut file = fs::File::open(&path).map_err(ArtifactPathError::from_io)?;
        let mut hasher = Sha256::new();
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let read = file.read(&mut buffer).map_err(ArtifactPathError::from_io)?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
        }
        Ok(format!("sha256:{:x}", hasher.finalize()))
    }

    pub fn accept_external_edit(&self, artifact: &mut ArtifactRecord) -> bool {
        if !matches!(
            artifact.kind,
            ArtifactKind::Segments | ArtifactKind::TranslatedSegments | ArtifactKind::SubtitleSrt
        ) {
            return false;
        }
        let Ok(path) = self.resolve(&artifact.relative_path) else {
            return false;
        };
        let Ok(metadata) = fs::metadata(path) else {
            return false;
        };
        let Ok(hash) = self.hash_file(&artifact.relative_path) else {
            return false;
        };
        artifact.size = metadata.len();
        artifact.modified = metadata
            .modified()
            .ok()
            .and_then(system_time_millis)
            .unwrap_or(0);
        artifact.content_hash = Some(hash);
        artifact.status = ArtifactStatus::Valid;
        true
    }

    pub fn refresh_statuses<'a>(
        &self,
        artifacts: impl IntoIterator<Item = &'a mut ArtifactRecord>,
    ) -> usize {
        let mut changed = 0;
        for artifact in artifacts {
            let status = self.inspect(artifact);
            if artifact.status != status {
                artifact.status = status;
                changed += 1;
            }
        }
        changed
    }
}

impl ArtifactPathError {
    fn from_io(error: io::Error) -> Self {
        Self::Io(format!("文件系统操作失败: {error}"))
    }
}

/// 安全纵深：重解析点防护（路径安全层，当前未接线，保留）
#[allow(dead_code)]
fn is_reparse_or_symlink(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }

    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }

    #[cfg(not(windows))]
    false
}

fn system_time_millis(value: std::time::SystemTime) -> Option<i64> {
    value
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
}

#[allow(dead_code)]
fn reject_reparse_point(path: &Path) -> Result<(), ArtifactPathError> {
    let metadata = fs::symlink_metadata(path).map_err(ArtifactPathError::from_io)?;
    if is_reparse_or_symlink(&metadata) {
        Err(ArtifactPathError::ReparsePoint)
    } else if !metadata.is_dir() {
        Err(ArtifactPathError::NotDirectory)
    } else {
        Ok(())
    }
}

pub fn validate_relative_path(path: &Path) -> Result<(), ArtifactPathError> {
    if path.as_os_str().is_empty() {
        return Err(ArtifactPathError::Empty);
    }
    if path.is_absolute() {
        return Err(ArtifactPathError::Absolute);
    }

    let mut has_normal_component = false;
    for component in path.components() {
        match component {
            Component::Normal(name) => {
                has_normal_component = true;
                let name = name.to_string_lossy();
                if name == STAGING_DIR || name.ends_with(TEMP_SUFFIX) {
                    return Err(ArtifactPathError::ReservedInternalPath);
                }
                if is_invalid_windows_component(&name) {
                    return Err(ArtifactPathError::InvalidWindowsName);
                }
            }
            Component::ParentDir => return Err(ArtifactPathError::ParentTraversal),
            Component::Prefix(_) => return Err(ArtifactPathError::Prefix),
            Component::RootDir => return Err(ArtifactPathError::Absolute),
            Component::CurDir => return Err(ArtifactPathError::CurrentDirectory),
        }
    }

    has_normal_component
        .then_some(())
        .ok_or(ArtifactPathError::Empty)
}

fn is_invalid_windows_component(value: &str) -> bool {
    if value.contains(['<', '>', ':', '"', '|', '?', '*'])
        || value.ends_with(' ')
        || value.ends_with('.')
    {
        return true;
    }

    let stem = value.split('.').next().unwrap_or(value);
    let upper = stem.to_ascii_uppercase();
    matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (upper.len() == 4
            && (upper.starts_with("COM") || upper.starts_with("LPT"))
            && upper.as_bytes()[3].is_ascii_digit()
            && upper.as_bytes()[3] != b'0')
}

#[allow(dead_code)]
fn validate_variant_id(value: &str) -> Result<(), ArtifactPathError> {
    if value.is_empty()
        || value == "."
        || value == ".."
        || value.contains('/')
        || value.contains('\\')
        || is_invalid_windows_component(value)
    {
        return Err(ArtifactPathError::InvalidVariantId);
    }
    Ok(())
}

#[allow(dead_code)]
fn validate_internal_name(value: &str) -> Result<(), ArtifactPathError> {
    validate_variant_id(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> ArtifactStore {
        ArtifactStore::new(PathBuf::from(r"C:\tasks\parent-1"))
    }

    #[test]
    fn resolves_valid_relative_path_under_root() {
        let resolved = store().resolve(Path::new("shared/audio.wav")).unwrap();
        assert_eq!(
            resolved,
            PathBuf::from(r"C:\tasks\parent-1\shared\audio.wav")
        );
    }

    #[test]
    fn rejects_parent_traversal() {
        assert_eq!(
            store().resolve(Path::new("targets/../secret.txt")),
            Err(ArtifactPathError::ParentTraversal)
        );
    }

    #[test]
    fn rejects_windows_absolute_and_prefixed_paths() {
        assert!(store().resolve(Path::new(r"D:\escape.wav")).is_err());
        assert!(store()
            .resolve(Path::new(r"\\server\share\escape.wav"))
            .is_err());
        assert!(store().resolve(Path::new(r"\\?\D:\escape.wav")).is_err());
    }

    #[test]
    fn rejects_internal_names_from_manifest_paths() {
        assert_eq!(
            store().resolve(Path::new(".staging/tx/audio.wav")),
            Err(ArtifactPathError::ReservedInternalPath)
        );
        assert_eq!(
            store().resolve(Path::new("audio.wav.tmp")),
            Err(ArtifactPathError::ReservedInternalPath)
        );
    }

    #[test]
    fn rejects_windows_ads_and_device_names() {
        assert_eq!(
            store().resolve(Path::new("audio.wav:payload")),
            Err(ArtifactPathError::InvalidWindowsName)
        );
        assert_eq!(
            store().resolve(Path::new("NUL.wav")),
            Err(ArtifactPathError::InvalidWindowsName)
        );
        assert_eq!(
            store().resolve(Path::new("targets/COM1/file.wav")),
            Err(ArtifactPathError::InvalidWindowsName)
        );
    }

    #[test]
    fn target_scope_allocates_stable_variant_directory() {
        let path = store()
            .scoped_path(
                &ArtifactScope::Target(VariantId("zh-yue".into())),
                Path::new("dub.wav"),
            )
            .unwrap();
        assert_eq!(
            path,
            PathBuf::from(r"C:\tasks\parent-1\targets\zh-yue\dub.wav")
        );
    }

    #[test]
    fn target_scope_rejects_variant_path_injection() {
        let result = store().scoped_path(
            &ArtifactScope::Target(VariantId("../other".into())),
            Path::new("dub.wav"),
        );
        assert_eq!(result, Err(ArtifactPathError::InvalidVariantId));
    }

    #[test]
    fn temp_file_is_sibling_of_target() {
        let target = PathBuf::from(r"C:\tasks\parent-1\shared\audio.wav");
        let temp = store().temp_path_for(&target).unwrap();
        assert_eq!(
            temp,
            PathBuf::from(r"C:\tasks\parent-1\shared\audio.wav.tmp")
        );
        assert_eq!(temp.parent(), target.parent());
    }

    #[test]
    fn temp_path_rejects_target_outside_store() {
        assert!(store()
            .temp_path_for(Path::new(r"D:\other\audio.wav"))
            .is_err());
    }

    #[test]
    fn transaction_staging_is_inside_task_root() {
        let layout = store().staging_layout("separate-attempt-1").unwrap();
        assert_eq!(
            layout.root,
            PathBuf::from(r"C:\tasks\parent-1\.staging\separate-attempt-1")
        );
        assert_eq!(
            layout.path_for(Path::new("vocals.raw.wav")).unwrap(),
            layout.root.join("vocals.raw.wav")
        );
    }

    #[test]
    fn staging_rejects_transaction_name_injection() {
        assert!(store().staging_layout("../escape").is_err());
        assert!(store().staging_layout(r"D:\escape").is_err());
    }

    #[test]
    fn cleanup_candidates_include_tmp_and_staging_only() {
        let store = store();
        assert!(store.is_orphan_candidate(&store.staging_root().join("old-tx")));
        assert!(store.is_orphan_candidate(Path::new(r"C:\tasks\parent-1\shared\audio.wav.tmp")));
        assert!(!store.is_orphan_candidate(Path::new(r"C:\tasks\parent-1\shared\audio.wav")));
        assert!(!store.is_orphan_candidate(Path::new(r"D:\other\audio.wav.tmp")));
    }

    #[test]
    fn prepare_target_creates_only_directories_inside_root() {
        let root =
            std::env::temp_dir().join(format!("videotrans-artifacts-{}", uuid::Uuid::new_v4()));
        let store = ArtifactStore::new(&root);
        let target = store
            .prepare_target(Path::new("targets/zh-yue/dub.wav"))
            .unwrap();

        assert_eq!(target, root.join("targets/zh-yue/dub.wav"));
        assert!(root.join("targets/zh-yue").is_dir());
        assert!(!target.exists());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn inspect_detects_valid_stale_and_missing_artifacts() {
        let root =
            std::env::temp_dir().join(format!("videotrans-artifacts-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let store = ArtifactStore::new(&root);
        let path = root.join("dub.wav");
        fs::write(&path, b"audio").unwrap();
        let metadata = fs::metadata(&path).unwrap();
        let modified = metadata
            .modified()
            .ok()
            .and_then(system_time_millis)
            .unwrap();
        let mut artifact = ArtifactRecord::valid_required(
            "dub",
            crate::domain::artifact::ArtifactKind::DubAudio,
            "tts",
            "dub.wav",
        );
        artifact.size = metadata.len();
        artifact.modified = modified;

        assert_eq!(store.inspect(&artifact), ArtifactStatus::Valid);
        fs::write(&path, b"changed-audio").unwrap();
        assert_eq!(store.inspect(&artifact), ArtifactStatus::Stale);
        fs::remove_file(&path).unwrap();
        assert_eq!(store.inspect(&artifact), ArtifactStatus::Missing);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn refresh_statuses_updates_manifest_records() {
        let root =
            std::env::temp_dir().join(format!("videotrans-artifacts-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let store = ArtifactStore::new(&root);
        let mut artifact = ArtifactRecord::valid_required(
            "missing",
            crate::domain::artifact::ArtifactKind::DubAudio,
            "tts",
            "missing.wav",
        );

        assert_eq!(store.refresh_statuses([&mut artifact]), 1);
        assert_eq!(artifact.status, ArtifactStatus::Missing);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn prepare_target_rejects_file_used_as_parent_directory() {
        let root =
            std::env::temp_dir().join(format!("videotrans-artifacts-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("targets"), b"not a directory").unwrap();
        let store = ArtifactStore::new(&root);

        assert_eq!(
            store.prepare_target(Path::new("targets/zh-yue/dub.wav")),
            Err(ArtifactPathError::NotDirectory)
        );

        fs::remove_dir_all(root).unwrap();
    }
}
