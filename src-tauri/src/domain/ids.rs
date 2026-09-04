//! 稳定 ID 类型：先用 String newtype，避免到处裸 String。
//! 后续可以把生成/校验规则收敛到这里。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TaskId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ChildTaskId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct VariantId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct StageId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ArtifactId(pub String);

impl From<&str> for TaskId {
    fn from(v: &str) -> Self {
        Self(v.into())
    }
}
impl From<&str> for ChildTaskId {
    fn from(v: &str) -> Self {
        Self(v.into())
    }
}
impl From<&str> for VariantId {
    fn from(v: &str) -> Self {
        Self(v.into())
    }
}
impl From<&str> for StageId {
    fn from(v: &str) -> Self {
        Self(v.into())
    }
}
impl From<&str> for ArtifactId {
    fn from(v: &str) -> Self {
        Self(v.into())
    }
}
