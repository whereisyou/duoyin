//! PipelineRunner 拆分后的 re-export 门面。
//! 外部代码仍走 `crate::pipeline::runner::{...}`，路径一字不变。

mod executor;
mod records;
#[cfg(test)]
mod tests;
mod types;

pub use executor::PipelineRunner;
pub use types::{
    ArtifactOutput, CancelToken, CheckpointFuture, ExecuteError, ExecuteFuture, ExecutionContext,
    ExecutionOutcome, PipelineCheckpoint, PipelineObserver, RunScope, StageExecutor, StageRequest,
    StageUpdate,
};
// ArtifactInput 仅测试在使用（adapters/media/output_stages 的真机冒烟 import 它做输入构造），
// 生产死代码分析视为未用；独立 use + allow 以免 lib 编译产生噪音，删除会破坏测试编译。
#[allow(unused_imports)]
pub use types::ArtifactInput;

// PipelineError / StageRunResult 仅 runner 子树内部使用（executor 与 tests 经 `use super::*` 拿）。
// 以「子树级可见性」 re-export：crate::pipeline::runner 之外的模块仍看不到（公共面不变），
// 但 runner/tests.rs 的 `use super::*` 可命中，且 lib 编译不产生 unused_imports 警告。
#[allow(unused_imports)]
pub(in crate::pipeline::runner) use types::{PipelineError, StageRunResult};
