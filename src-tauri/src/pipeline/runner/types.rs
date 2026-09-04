use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::domain::artifact::{ArtifactKind, RetentionPolicy};
use crate::domain::ids::{ArtifactId, StageId, VariantId};
use crate::domain::manifest::{FallbackRecord, StageStatus, TaskManifest};
use crate::pipeline::graph::{GraphError, StageNode};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum RunScope {
    Parent,
    Target(VariantId),
}

impl RunScope {
    // 拆分前是 runner.rs 内的私有方法；executor/records 两个兄弟模块都要调用，
    // 故可见性精确限定在 runner 子树内（不升 pub(crate)，避免泄漏到整个 crate）。
    pub(in crate::pipeline::runner) fn node_key(&self, stage: &StageId) -> String {
        match self {
            Self::Parent => format!("parent:{}", stage.0),
            Self::Target(variant) => format!("target:{}:{}", variant.0, stage.0),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    pub fn is_canceled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactInput {
    pub id: ArtifactId,
    pub kind: ArtifactKind,
    pub path: PathBuf,
    pub content_hash: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ExecutionContext {
    pub task_root: PathBuf,
    pub cancel: CancelToken,
}

impl ExecutionContext {
    pub fn work_dir(&self, node_key: &str) -> PathBuf {
        self.task_root
            .join(".staging")
            .join(node_key.replace(':', "-"))
    }
}

#[derive(Debug, Clone)]
pub struct StageRequest {
    pub node: StageNode,
    pub scope: RunScope,
    pub inputs: Vec<ArtifactInput>,
}

impl StageRequest {
    pub fn input(&self, kind: ArtifactKind) -> Option<&ArtifactInput> {
        self.inputs.iter().find(|input| input.kind == kind)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactOutput {
    pub id: ArtifactId,
    pub kind: ArtifactKind,
    pub relative_path: String,
    pub size: u64,
    pub modified: i64,
    pub content_hash: String,
    pub media_type: Option<String>,
    pub retention: RetentionPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecutionOutcome {
    Done(Vec<ArtifactOutput>),
    /// 降级产物（separation 无 BGM 回退等）——当前分离降级走 Skipped 状态，保留
    #[allow(dead_code)]
    Degraded {
        outputs: Vec<ArtifactOutput>,
        fallback: FallbackRecord,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecuteError {
    Canceled,
    Failed(String),
}

pub type ExecuteFuture<'a> =
    Pin<Box<dyn Future<Output = Result<ExecutionOutcome, ExecuteError>> + Send + 'a>>;
pub type CheckpointFuture<'a> = Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>>;

pub trait PipelineCheckpoint: Send + Sync {
    fn save<'a>(&'a self, manifest: TaskManifest) -> CheckpointFuture<'a>;
}

#[derive(Debug, Clone)]
pub struct StageUpdate {
    pub scope: RunScope,
    pub stage_id: StageId,
    pub status: StageStatus,
    pub error: Option<String>,
}

pub trait PipelineObserver: Send + Sync {
    fn on_stage_update(&self, update: StageUpdate);
}

pub trait StageExecutor: Send + Sync {
    fn version(&self, stage: &StageId) -> String;

    fn execute<'a>(&'a self, request: StageRequest, context: ExecutionContext)
        -> ExecuteFuture<'a>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StageRunResult {
    Executed,
    Reused,
    Skipped,
    Degraded,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PipelineError {
    Graph(String),
    ScopeMismatch(String),
    DependencyNotReady(String),
    Canceled(String),
    StageFailed { node_key: String, message: String },
    Checkpoint(String),
}

impl From<GraphError> for PipelineError {
    fn from(value: GraphError) -> Self {
        Self::Graph(value.to_string())
    }
}
