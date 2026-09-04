//! 【冻结区】旧单目标流程，仅紧急回滚使用；禁止新代码依赖它。
//!
//! 新代码请走 `commands::task::start_multi_target_task` + pipeline（DAG）。
//! 本区删除需满足 `docs/BACKEND_ARCHITECTURE.md` §6 的全部移除标准；
//! 见 `docs/FUNCTION_CHECKLIST.md` 的「冻结，仅紧急回滚」标注。

pub mod command;
pub(in crate::legacy) mod ffmpeg;
pub(in crate::legacy) mod process;
