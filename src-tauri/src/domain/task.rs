use serde::{Deserialize, Serialize};

use super::ids::{ChildTaskId, TaskId};
use super::media::SourceVideo;
use super::variant::TargetVariant;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ParentStatus {
    Pending,
    Running,
    Completed,
    PartiallyFailed,
    Failed,
    Canceled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChildStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Canceled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParentTask {
    pub id: TaskId,
    pub source: SourceVideo,
    pub status: ParentStatus,
    pub children: Vec<ChildTaskId>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChildTask {
    pub id: ChildTaskId,
    pub parent_id: TaskId,
    pub variant: TargetVariant,
    pub status: ChildStatus,
    pub created_at: i64,
    pub updated_at: i64,
}

/// 共享阶段已成功后，父状态由所有目标版本汇总；共享阶段失败应直接设为 Failed。
pub fn aggregate_child_statuses(statuses: &[ChildStatus]) -> ParentStatus {
    if statuses.is_empty() {
        return ParentStatus::Completed;
    }

    if statuses.iter().any(|s| *s == ChildStatus::Running) {
        return ParentStatus::Running;
    }
    if statuses.iter().any(|s| *s == ChildStatus::Pending) {
        return ParentStatus::Pending;
    }

    let completed = statuses
        .iter()
        .filter(|s| **s == ChildStatus::Completed)
        .count();
    let failed = statuses
        .iter()
        .filter(|s| **s == ChildStatus::Failed)
        .count();
    let canceled = statuses
        .iter()
        .filter(|s| **s == ChildStatus::Canceled)
        .count();

    if completed == statuses.len() {
        ParentStatus::Completed
    } else if failed == statuses.len() {
        ParentStatus::Failed
    } else if canceled == statuses.len() {
        ParentStatus::Canceled
    } else {
        ParentStatus::PartiallyFailed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_failed_variant_does_not_fail_whole_parent() {
        assert_eq!(
            aggregate_child_statuses(&[ChildStatus::Completed, ChildStatus::Failed]),
            ParentStatus::PartiallyFailed
        );
    }

    #[test]
    fn all_failed_variants_fail_parent() {
        assert_eq!(
            aggregate_child_statuses(&[ChildStatus::Failed, ChildStatus::Failed]),
            ParentStatus::Failed
        );
    }

    #[test]
    fn canceling_one_variant_does_not_cancel_others() {
        assert_eq!(
            aggregate_child_statuses(&[ChildStatus::Completed, ChildStatus::Canceled]),
            ParentStatus::PartiallyFailed
        );
    }

    #[test]
    fn all_canceled_variants_cancel_parent() {
        assert_eq!(
            aggregate_child_statuses(&[ChildStatus::Canceled, ChildStatus::Canceled]),
            ParentStatus::Canceled
        );
    }

    #[test]
    fn failure_is_not_final_while_another_variant_is_running() {
        assert_eq!(
            aggregate_child_statuses(&[ChildStatus::Failed, ChildStatus::Running]),
            ParentStatus::Running
        );
    }
}
