//! 运行中任务登记表（全局唯一）。
//!
//! 任务句柄与取消令牌集中在这里，commands 层通过它查询/取消运行中的任务。
//! 只存运行态；持久化状态在 infra::task_store。

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::sync::Mutex;

use tokio::task::JoinHandle;

use crate::domain::ids::{TaskId, VariantId};
use crate::pipeline::runner::CancelToken;

pub struct RunningTask {
    pub handle: JoinHandle<()>,
    pub cancel: Option<CancelToken>,
    pub task_id: TaskId,
    pub child_cancels: BTreeMap<VariantId, CancelToken>,
}

pub static TASKS: once_cell::sync::Lazy<Mutex<HashMap<String, RunningTask>>> =
    once_cell::sync::Lazy::new(|| Mutex::new(HashMap::new()));
