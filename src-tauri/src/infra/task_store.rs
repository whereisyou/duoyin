use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::domain::config::PipelineConfig;
use crate::domain::ids::TaskId;
use crate::domain::manifest::TaskManifest;
use crate::domain::task::{ChildTask, ParentStatus, ParentTask};

const STORAGE_SCHEMA_VERSION: u32 = 1;
const TASKS_DIR: &str = "tasks";
const TASK_FILE: &str = "task.json";
const MANIFEST_FILE: &str = "manifest.json";
const INDEX_FILE: &str = "index.json";
const TEMP_SUFFIX: &str = ".tmp";
const BACKUP_SUFFIX: &str = ".bak";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskDocument {
    pub schema_version: u32,
    pub revision: u64,
    pub parent: ParentTask,
    pub children: Vec<ChildTask>,
    pub config: PipelineConfig,
}

impl TaskDocument {
    pub fn new(parent: ParentTask, children: Vec<ChildTask>, config: PipelineConfig) -> Self {
        Self {
            schema_version: STORAGE_SCHEMA_VERSION,
            revision: 0,
            parent,
            children,
            config,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ManifestDocument {
    schema_version: u32,
    revision: u64,
    manifest: TaskManifest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskIndexEntry {
    pub task_id: TaskId,
    pub status: ParentStatus,
    pub updated_at: i64,
    pub revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskIndex {
    pub schema_version: u32,
    pub tasks: BTreeMap<TaskId, TaskIndexEntry>,
}

impl Default for TaskIndex {
    fn default() -> Self {
        Self {
            schema_version: STORAGE_SCHEMA_VERSION,
            tasks: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedTask {
    pub task: TaskDocument,
    pub manifest: TaskManifest,
    pub recovered_from_backup: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveOutcome {
    pub revision: u64,
    /// 索引只是可重建缓存；false 不影响 task/manifest 已成功提交。
    pub index_updated: bool,
}

#[derive(Debug)]
pub enum TaskStoreError {
    InvalidTaskId,
    Inconsistent(String),
    UnsupportedSchema { found: u32, supported: u32 },
    NotFound(TaskId),
    Corrupt { path: PathBuf, message: String },
    Io { path: PathBuf, source: io::Error },
    LockPoisoned,
}

impl fmt::Display for TaskStoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidTaskId => f.write_str("任务 ID 包含不安全路径字符"),
            Self::Inconsistent(message) => write!(f, "任务数据不一致: {message}"),
            Self::UnsupportedSchema { found, supported } => {
                write!(f, "存储版本 {found} 高于当前支持版本 {supported}")
            }
            Self::NotFound(id) => write!(f, "任务不存在: {}", id.0),
            Self::Corrupt { path, message } => {
                write!(f, "任务文件损坏 {}: {message}", path.display())
            }
            Self::Io { path, source } => write!(f, "文件操作失败 {}: {source}", path.display()),
            Self::LockPoisoned => f.write_str("任务存储写锁已损坏"),
        }
    }
}

impl std::error::Error for TaskStoreError {}

#[derive(Debug)]
pub struct TaskStore {
    root: PathBuf,
    write_lock: Mutex<()>,
}

impl TaskStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            write_lock: Mutex::new(()),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn task_dir(&self, task_id: &TaskId) -> Result<PathBuf, TaskStoreError> {
        validate_task_id(&task_id.0)?;
        Ok(self.root.join(TASKS_DIR).join(&task_id.0))
    }

    pub fn save_bundle(
        &self,
        task: &mut TaskDocument,
        manifest: &TaskManifest,
    ) -> Result<SaveOutcome, TaskStoreError> {
        let _guard = self
            .write_lock
            .lock()
            .map_err(|_| TaskStoreError::LockPoisoned)?;
        validate_bundle(task, manifest)?;

        let revision = self
            .load_bundle_unlocked(&task.parent.id)
            .map(|loaded| loaded.task.revision.saturating_add(1))
            .or_else(|error| match error {
                TaskStoreError::NotFound(_) => Ok(1),
                other => Err(other),
            })?;

        let directory = self.task_dir(&task.parent.id)?;
        fs::create_dir_all(&directory).map_err(|error| io_error(&directory, error))?;

        let mut next_task = task.clone();
        next_task.revision = revision;
        let next_manifest = ManifestDocument {
            schema_version: STORAGE_SCHEMA_VERSION,
            revision,
            manifest: manifest.clone(),
        };

        let task_path = directory.join(TASK_FILE);
        let manifest_path = directory.join(MANIFEST_FILE);
        write_json_temp(&task_path, &next_task)?;
        write_json_temp(&manifest_path, &next_manifest)?;

        rotate_to_backup(&task_path)?;
        rotate_to_backup(&manifest_path)?;

        // Manifest 先提交，task.json 最后提交并作为 bundle 的提交点。
        commit_temp(&manifest_path)?;
        if let Err(error) = commit_temp(&task_path) {
            restore_backup_if_missing(&manifest_path)?;
            restore_backup_if_missing(&task_path)?;
            return Err(error);
        }

        *task = next_task;
        let index_updated = self.rebuild_index_unlocked().is_ok();
        Ok(SaveOutcome {
            revision,
            index_updated,
        })
    }

    pub fn load_bundle(&self, task_id: &TaskId) -> Result<LoadedTask, TaskStoreError> {
        self.load_bundle_unlocked(task_id)
    }

    /// 删除任务：移除任务目录，并同步从索引剔除条目（原子重写 index.json）。
    /// 任务目录不存在时同样清理索引残留（幂等）；index.json 缺失/损坏时跳过索引更新，
    /// 下次 load_index_or_rebuild 会从目录扫描重建干净索引。
    pub fn delete_task(&self, task_id: &TaskId) -> Result<(), TaskStoreError> {
        let _guard = self
            .write_lock
            .lock()
            .map_err(|_| TaskStoreError::LockPoisoned)?;
        let directory = self.task_dir(task_id)?;
        match fs::remove_dir_all(&directory) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(io_error(&directory, error)),
        }
        let index_path = self.root.join(INDEX_FILE);
        if index_path.is_file() {
            if let Ok(mut index) = read_json::<TaskIndex>(&index_path) {
                if index.schema_version <= STORAGE_SCHEMA_VERSION
                    && index.tasks.remove(task_id).is_some()
                {
                    write_json_temp(&index_path, &index)?;
                    rotate_to_backup(&index_path)?;
                    commit_temp(&index_path)?;
                }
            }
        }
        Ok(())
    }

    pub fn load_index_or_rebuild(&self) -> Result<TaskIndex, TaskStoreError> {
        match read_json::<TaskIndex>(&self.root.join(INDEX_FILE)) {
            Ok(index) if index.schema_version <= STORAGE_SCHEMA_VERSION => Ok(index),
            Ok(index) => Err(TaskStoreError::UnsupportedSchema {
                found: index.schema_version,
                supported: STORAGE_SCHEMA_VERSION,
            }),
            Err(TaskStoreError::NotFound(_)) | Err(TaskStoreError::Corrupt { .. }) => {
                let _guard = self
                    .write_lock
                    .lock()
                    .map_err(|_| TaskStoreError::LockPoisoned)?;
                self.rebuild_index_unlocked()
            }
            Err(error) => Err(error),
        }
    }

    /// 索引重建：测试用（load_index_or_rebuild 走内部 unlocked 版）；未来巡检/自愈用
    #[allow(dead_code)]
    pub fn rebuild_index(&self) -> Result<TaskIndex, TaskStoreError> {
        let _guard = self
            .write_lock
            .lock()
            .map_err(|_| TaskStoreError::LockPoisoned)?;
        self.rebuild_index_unlocked()
    }

    /// 临时文件清理：测试用；未接入启动自愈流程，保留
    #[allow(dead_code)]
    pub fn cleanup_orphan_temps(&self) -> Result<usize, TaskStoreError> {
        let _guard = self
            .write_lock
            .lock()
            .map_err(|_| TaskStoreError::LockPoisoned)?;
        let mut removed = 0;
        cleanup_temps_in(&self.root, &mut removed)?;
        Ok(removed)
    }

    fn load_bundle_unlocked(&self, task_id: &TaskId) -> Result<LoadedTask, TaskStoreError> {
        let directory = self.task_dir(task_id)?;
        if !directory.is_dir() {
            return Err(TaskStoreError::NotFound(task_id.clone()));
        }

        let task_paths = [
            directory.join(TASK_FILE),
            backup_path(&directory.join(TASK_FILE)),
        ];
        let manifest_paths = [
            directory.join(MANIFEST_FILE),
            backup_path(&directory.join(MANIFEST_FILE)),
        ];

        let tasks = read_candidates::<TaskDocument>(&task_paths);
        let manifests = read_candidates::<ManifestDocument>(&manifest_paths);
        let mut best: Option<LoadedTask> = None;

        for (task_path, task) in &tasks {
            for (manifest_path, manifest) in &manifests {
                if task.revision != manifest.revision {
                    continue;
                }
                validate_storage_schema(task.schema_version)?;
                validate_storage_schema(manifest.schema_version)?;
                validate_bundle(task, &manifest.manifest)?;
                if best
                    .as_ref()
                    .map(|loaded| loaded.task.revision >= task.revision)
                    .unwrap_or(false)
                {
                    continue;
                }
                best = Some(LoadedTask {
                    task: task.clone(),
                    manifest: manifest.manifest.clone(),
                    recovered_from_backup: task_path
                        .ends_with(format!("{TASK_FILE}{BACKUP_SUFFIX}"))
                        || manifest_path.ends_with(format!("{MANIFEST_FILE}{BACKUP_SUFFIX}")),
                });
            }
        }

        best.ok_or_else(|| TaskStoreError::Corrupt {
            path: directory,
            message: "找不到 revision 一致的 task/manifest 文件对".into(),
        })
    }

    fn rebuild_index_unlocked(&self) -> Result<TaskIndex, TaskStoreError> {
        let tasks_root = self.root.join(TASKS_DIR);
        fs::create_dir_all(&tasks_root).map_err(|error| io_error(&tasks_root, error))?;
        let mut index = TaskIndex::default();

        let entries = fs::read_dir(&tasks_root).map_err(|error| io_error(&tasks_root, error))?;
        for entry in entries {
            let entry = entry.map_err(|error| io_error(&tasks_root, error))?;
            if !entry
                .file_type()
                .map_err(|error| io_error(&entry.path(), error))?
                .is_dir()
            {
                continue;
            }
            let Some(task_id) = entry
                .file_name()
                .to_str()
                .map(|value| TaskId(value.to_owned()))
            else {
                continue;
            };
            if validate_task_id(&task_id.0).is_err() {
                continue;
            }
            let Ok(loaded) = self.load_bundle_unlocked(&task_id) else {
                continue;
            };
            index.tasks.insert(
                task_id.clone(),
                TaskIndexEntry {
                    task_id,
                    status: loaded.task.parent.status.clone(),
                    updated_at: loaded.task.parent.updated_at,
                    revision: loaded.task.revision,
                },
            );
        }

        fs::create_dir_all(&self.root).map_err(|error| io_error(&self.root, error))?;
        write_json_temp(&self.root.join(INDEX_FILE), &index)?;
        rotate_to_backup(&self.root.join(INDEX_FILE))?;
        commit_temp(&self.root.join(INDEX_FILE))?;
        Ok(index)
    }
}

fn validate_bundle(task: &TaskDocument, manifest: &TaskManifest) -> Result<(), TaskStoreError> {
    validate_storage_schema(task.schema_version)?;
    validate_task_id(&task.parent.id.0)?;
    if task.parent.id != manifest.parent_task_id {
        return Err(TaskStoreError::Inconsistent(
            "task 与 manifest 的 parent_task_id 不一致".into(),
        ));
    }
    if task.parent.source.fingerprint != manifest.source_fingerprint {
        return Err(TaskStoreError::Inconsistent(
            "task 与 manifest 的源文件指纹不一致".into(),
        ));
    }
    let declared_children: BTreeSet<_> = task.parent.children.iter().cloned().collect();
    let actual_children: BTreeSet<_> = task.children.iter().map(|child| child.id.clone()).collect();
    if declared_children.len() != task.parent.children.len()
        || actual_children.len() != task.children.len()
        || declared_children != actual_children
    {
        return Err(TaskStoreError::Inconsistent(
            "父任务 children 与实际子任务集合不一致或存在重复 ID".into(),
        ));
    }

    for child in &task.children {
        if child.parent_id != task.parent.id {
            return Err(TaskStoreError::Inconsistent(format!(
                "子任务 {} 指向了其他父任务",
                child.id.0
            )));
        }
    }

    let configured_variants: BTreeSet<_> = task
        .config
        .targets
        .iter()
        .map(|variant| variant.id.clone())
        .collect();
    let child_variants: BTreeSet<_> = task
        .children
        .iter()
        .map(|child| child.variant.id.clone())
        .collect();
    if configured_variants.len() != task.config.targets.len()
        || child_variants.len() != task.children.len()
        || configured_variants != child_variants
    {
        return Err(TaskStoreError::Inconsistent(
            "配置目标版本与子任务版本不一致或存在重复版本".into(),
        ));
    }

    task.config.validate().map_err(TaskStoreError::Inconsistent)
}

fn validate_storage_schema(schema_version: u32) -> Result<(), TaskStoreError> {
    if schema_version > STORAGE_SCHEMA_VERSION {
        Err(TaskStoreError::UnsupportedSchema {
            found: schema_version,
            supported: STORAGE_SCHEMA_VERSION,
        })
    } else {
        Ok(())
    }
}

fn validate_task_id(value: &str) -> Result<(), TaskStoreError> {
    let upper = value
        .split('.')
        .next()
        .unwrap_or(value)
        .to_ascii_uppercase();
    let reserved_device = matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (upper.len() == 4
            && (upper.starts_with("COM") || upper.starts_with("LPT"))
            && upper.as_bytes()[3].is_ascii_digit()
            && upper.as_bytes()[3] != b'0');
    if value.is_empty()
        || value == "."
        || value == ".."
        || value.contains(['/', '\\', ':', '<', '>', '"', '|', '?', '*'])
        || value.ends_with('.')
        || value.ends_with(' ')
        || reserved_device
    {
        return Err(TaskStoreError::InvalidTaskId);
    }
    Ok(())
}

fn read_candidates<T: DeserializeOwned + Clone>(paths: &[PathBuf]) -> Vec<(PathBuf, T)> {
    paths
        .iter()
        .filter_map(|path| read_json(path).ok().map(|value| (path.clone(), value)))
        .collect()
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T, TaskStoreError> {
    let file = File::open(path).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            TaskStoreError::NotFound(TaskId(path.display().to_string()))
        } else {
            io_error(path, error)
        }
    })?;
    serde_json::from_reader(BufReader::new(file)).map_err(|error| TaskStoreError::Corrupt {
        path: path.to_owned(),
        message: error.to_string(),
    })
}

fn write_json_temp<T: Serialize>(target: &Path, value: &T) -> Result<(), TaskStoreError> {
    let temp = temp_path(target);
    let parent = target
        .parent()
        .ok_or_else(|| TaskStoreError::Inconsistent("目标文件没有父目录".into()))?;
    fs::create_dir_all(parent).map_err(|error| io_error(parent, error))?;
    let file = File::create(&temp).map_err(|error| io_error(&temp, error))?;
    let mut writer = BufWriter::new(file);
    serde_json::to_writer_pretty(&mut writer, value).map_err(|error| TaskStoreError::Corrupt {
        path: temp.clone(),
        message: error.to_string(),
    })?;
    writer
        .write_all(b"\n")
        .map_err(|error| io_error(&temp, error))?;
    writer.flush().map_err(|error| io_error(&temp, error))?;
    writer
        .get_ref()
        .sync_all()
        .map_err(|error| io_error(&temp, error))
}

fn rotate_to_backup(target: &Path) -> Result<(), TaskStoreError> {
    if !target.exists() {
        return Ok(());
    }
    let backup = backup_path(target);
    if backup.exists() {
        fs::remove_file(&backup).map_err(|error| io_error(&backup, error))?;
    }
    fs::rename(target, &backup).map_err(|error| io_error(target, error))
}

fn commit_temp(target: &Path) -> Result<(), TaskStoreError> {
    let temp = temp_path(target);
    fs::rename(&temp, target).map_err(|error| io_error(&temp, error))
}

fn restore_backup_if_missing(target: &Path) -> Result<(), TaskStoreError> {
    let backup = backup_path(target);
    if !target.exists() && backup.exists() {
        fs::rename(&backup, target).map_err(|error| io_error(&backup, error))?;
    }
    Ok(())
}

/// 测试辅助（cleanup_orphan_temps 的清理实现）
#[allow(dead_code)]
fn cleanup_temps_in(path: &Path, removed: &mut usize) -> Result<(), TaskStoreError> {
    if !path.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(path).map_err(|error| io_error(path, error))? {
        let entry = entry.map_err(|error| io_error(path, error))?;
        let entry_path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|error| io_error(&entry_path, error))?;
        if file_type.is_dir() {
            cleanup_temps_in(&entry_path, removed)?;
        } else if entry_path
            .file_name()
            .and_then(|name| name.to_str())
            .map(|name| name.ends_with(TEMP_SUFFIX))
            .unwrap_or(false)
        {
            fs::remove_file(&entry_path).map_err(|error| io_error(&entry_path, error))?;
            *removed += 1;
        }
    }
    Ok(())
}

fn temp_path(target: &Path) -> PathBuf {
    target.with_file_name(format!(
        "{}{}",
        target.file_name().unwrap_or_default().to_string_lossy(),
        TEMP_SUFFIX
    ))
}

fn backup_path(target: &Path) -> PathBuf {
    target.with_file_name(format!(
        "{}{}",
        target.file_name().unwrap_or_default().to_string_lossy(),
        BACKUP_SUFFIX
    ))
}

fn io_error(path: &Path, source: io::Error) -> TaskStoreError {
    TaskStoreError::Io {
        path: path.to_owned(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::config::{EngineSelection, OutputConfig, SeparationConfig};
    use crate::domain::ids::ChildTaskId;
    use crate::domain::media::{SourceFingerprint, SourceVideo};
    use crate::domain::task::ChildStatus;
    use crate::domain::variant::TargetVariant;

    fn root() -> PathBuf {
        std::env::temp_dir().join(format!("videotrans-task-store-{}", uuid::Uuid::new_v4()))
    }

    fn fingerprint() -> SourceFingerprint {
        SourceFingerprint {
            size: 100,
            modified: 10,
            content_hash: Some("source-hash".into()),
            hash_algo_version: 1,
        }
    }

    fn bundle() -> (TaskDocument, TaskManifest) {
        let task_id = TaskId("parent-1".into());
        let child_id = ChildTaskId("parent-1-zh-CN".into());
        let variant = TargetVariant::zh_mandarin();
        let parent = ParentTask {
            id: task_id.clone(),
            source: SourceVideo {
                path: PathBuf::from(r"D:\videos\input.mp4"),
                fingerprint: fingerprint(),
            },
            status: ParentStatus::Pending,
            children: vec![child_id.clone()],
            created_at: 1,
            updated_at: 1,
        };
        let child = ChildTask {
            id: child_id,
            parent_id: task_id.clone(),
            variant: variant.clone(),
            status: ChildStatus::Pending,
            created_at: 1,
            updated_at: 1,
        };
        let config = PipelineConfig {
            source_language: None,
            targets: vec![variant],
            engines: EngineSelection {
                stt: "sensevoice".into(),
                translator: "openai-compatible".into(),
                tts: "cosyvoice3".into(),
                separator: None,
            },
            separation: SeparationConfig::default(),
            output: OutputConfig::default(),
        };
        (
            TaskDocument::new(parent, vec![child], config),
            TaskManifest::new(task_id, fingerprint()),
        )
    }

    #[test]
    fn saves_and_loads_consistent_bundle() {
        let root = root();
        let store = TaskStore::new(&root);
        let (mut task, manifest) = bundle();

        assert_eq!(
            store.save_bundle(&mut task, &manifest).unwrap(),
            SaveOutcome {
                revision: 1,
                index_updated: true,
            }
        );
        let loaded = store.load_bundle(&task.parent.id).unwrap();
        assert_eq!(loaded.task, task);
        assert_eq!(loaded.manifest, manifest);
        assert!(!loaded.recovered_from_backup);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn update_increments_revision_and_keeps_backup_pair() {
        let root = root();
        let store = TaskStore::new(&root);
        let (mut task, manifest) = bundle();
        store.save_bundle(&mut task, &manifest).unwrap();
        task.parent.updated_at = 2;
        assert_eq!(
            store.save_bundle(&mut task, &manifest).unwrap(),
            SaveOutcome {
                revision: 2,
                index_updated: true,
            }
        );

        let directory = store.task_dir(&task.parent.id).unwrap();
        assert!(backup_path(&directory.join(TASK_FILE)).is_file());
        assert!(backup_path(&directory.join(MANIFEST_FILE)).is_file());
        assert_eq!(store.load_bundle(&task.parent.id).unwrap().task.revision, 2);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn mismatched_current_revision_recovers_matching_backup_pair() {
        let root = root();
        let store = TaskStore::new(&root);
        let (mut task, manifest) = bundle();
        store.save_bundle(&mut task, &manifest).unwrap();
        task.parent.updated_at = 2;
        store.save_bundle(&mut task, &manifest).unwrap();

        let directory = store.task_dir(&task.parent.id).unwrap();
        let mut current_manifest: ManifestDocument =
            read_json(&directory.join(MANIFEST_FILE)).unwrap();
        current_manifest.revision = 99;
        write_json_temp(&directory.join(MANIFEST_FILE), &current_manifest).unwrap();
        fs::remove_file(directory.join(MANIFEST_FILE)).unwrap();
        commit_temp(&directory.join(MANIFEST_FILE)).unwrap();

        let loaded = store.load_bundle(&task.parent.id).unwrap();
        assert_eq!(loaded.task.revision, 1);
        assert!(loaded.recovered_from_backup);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_inconsistent_parent_ids_before_writing() {
        let root = root();
        let store = TaskStore::new(&root);
        let (mut task, mut manifest) = bundle();
        manifest.parent_task_id = TaskId("other".into());

        assert!(matches!(
            store.save_bundle(&mut task, &manifest),
            Err(TaskStoreError::Inconsistent(_))
        ));
        assert!(!root.exists());
    }

    #[test]
    fn rejects_unsafe_task_id() {
        let store = TaskStore::new(root());
        for value in ["../escape", "NUL", "COM1.json", "task:stream"] {
            assert!(matches!(
                store.task_dir(&TaskId(value.into())),
                Err(TaskStoreError::InvalidTaskId)
            ));
        }
    }

    #[test]
    fn rejects_parent_child_set_mismatch() {
        let root = root();
        let store = TaskStore::new(&root);
        let (mut task, manifest) = bundle();
        task.parent.children.clear();

        assert!(matches!(
            store.save_bundle(&mut task, &manifest),
            Err(TaskStoreError::Inconsistent(_))
        ));
        assert!(!root.exists());
    }

    #[test]
    fn rejects_config_target_child_variant_mismatch() {
        let root = root();
        let store = TaskStore::new(&root);
        let (mut task, manifest) = bundle();
        task.config.targets = vec![TargetVariant::zh_dialect("yue", "粤语", "请用广东话表达。")];

        assert!(matches!(
            store.save_bundle(&mut task, &manifest),
            Err(TaskStoreError::Inconsistent(_))
        ));
        assert!(!root.exists());
    }

    #[test]
    fn corrupt_index_is_rebuilt_from_task_directories() {
        let root = root();
        let store = TaskStore::new(&root);
        let (mut task, manifest) = bundle();
        store.save_bundle(&mut task, &manifest).unwrap();
        fs::write(root.join(INDEX_FILE), b"not-json").unwrap();

        let index = store.load_index_or_rebuild().unwrap();
        assert_eq!(index.tasks.len(), 1);
        assert_eq!(index.tasks[&task.parent.id].revision, 1);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn delete_removes_directory_and_index_entry_and_is_idempotent() {
        let root = root();
        let store = TaskStore::new(&root);
        let (mut task, manifest) = bundle();
        store.save_bundle(&mut task, &manifest).unwrap();

        // 删除后：任务目录消失、索引条目剔除
        store.delete_task(&task.parent.id).unwrap();
        assert!(!store.task_dir(&task.parent.id).unwrap().exists());
        let index = store.load_index_or_rebuild().unwrap();
        assert!(!index.tasks.contains_key(&task.parent.id));

        // 幂等：重复删除不报错，索引保持干净
        store.delete_task(&task.parent.id).unwrap();
        let index = store.load_index_or_rebuild().unwrap();
        assert!(!index.tasks.contains_key(&task.parent.id));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn index_rebuild_skips_corrupt_task_directory() {
        let root = root();
        let store = TaskStore::new(&root);
        let (mut task, manifest) = bundle();
        store.save_bundle(&mut task, &manifest).unwrap();
        let broken = root.join(TASKS_DIR).join("broken");
        fs::create_dir_all(&broken).unwrap();
        fs::write(broken.join(TASK_FILE), b"broken").unwrap();

        let index = store.rebuild_index().unwrap();
        assert_eq!(index.tasks.len(), 1);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cleanup_removes_only_temporary_files() {
        let root = root();
        let store = TaskStore::new(&root);
        fs::create_dir_all(root.join(TASKS_DIR)).unwrap();
        fs::write(root.join("orphan.json.tmp"), b"partial").unwrap();
        fs::write(root.join("keep.json.bak"), b"backup").unwrap();

        assert_eq!(store.cleanup_orphan_temps().unwrap(), 1);
        assert!(!root.join("orphan.json.tmp").exists());
        assert!(root.join("keep.json.bak").exists());

        fs::remove_dir_all(root).unwrap();
    }
}
