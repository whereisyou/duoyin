use std::collections::BTreeMap;
use std::sync::Arc;

use crate::domain::ids::StageId;
use crate::pipeline::runner::{
    ExecuteError, ExecuteFuture, ExecutionContext, StageExecutor, StageRequest,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistryError {
    Duplicate(StageId),
}

#[derive(Default)]
pub struct StageRegistry {
    executors: BTreeMap<StageId, Arc<dyn StageExecutor>>,
}

impl StageRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(
        &mut self,
        stage: impl Into<StageId>,
        executor: Arc<dyn StageExecutor>,
    ) -> Result<(), RegistryError> {
        let stage = stage.into();
        if self.executors.contains_key(&stage) {
            return Err(RegistryError::Duplicate(stage));
        }
        self.executors.insert(stage, executor);
        Ok(())
    }

    /// 注册表查询（测试/诊断用），保留
    #[allow(dead_code)]
    pub fn contains(&self, stage: &StageId) -> bool {
        self.executors.contains_key(stage)
    }
}

impl StageExecutor for StageRegistry {
    fn version(&self, stage: &StageId) -> String {
        self.executors
            .get(stage)
            .map(|executor| executor.version(stage))
            .unwrap_or_else(|| "missing-executor".into())
    }

    fn execute<'a>(
        &'a self,
        request: StageRequest,
        context: ExecutionContext,
    ) -> ExecuteFuture<'a> {
        Box::pin(async move {
            let stage = request.node.id.clone();
            let executor = self
                .executors
                .get(&stage)
                .ok_or_else(|| ExecuteError::Failed(format!("节点 {} 没有注册执行器", stage.0)))?;
            executor.execute(request, context).await
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::artifact::ArtifactKind;
    use crate::pipeline::graph::{NodeScope, StageNode};
    use crate::pipeline::runner::{CancelToken, ExecutionOutcome, RunScope};

    struct NoopExecutor;

    impl StageExecutor for NoopExecutor {
        fn version(&self, _stage: &StageId) -> String {
            "noop-v1".into()
        }

        fn execute<'a>(
            &'a self,
            _request: StageRequest,
            _context: ExecutionContext,
        ) -> ExecuteFuture<'a> {
            Box::pin(async { Ok(ExecutionOutcome::Done(vec![])) })
        }
    }

    fn request(stage: &str) -> StageRequest {
        StageRequest {
            node: StageNode::new(stage, NodeScope::Parent, &[], vec![ArtifactKind::MediaInfo]),
            scope: RunScope::Parent,
            inputs: vec![],
        }
    }

    #[tokio::test]
    async fn dispatches_to_registered_executor() {
        let mut registry = StageRegistry::new();
        registry
            .register("media_probe", Arc::new(NoopExecutor))
            .unwrap();
        let result = registry
            .execute(
                request("media_probe"),
                ExecutionContext {
                    task_root: ".".into(),
                    cancel: CancelToken::default(),
                },
            )
            .await;
        assert!(matches!(result, Ok(ExecutionOutcome::Done(_))));
    }

    #[test]
    fn duplicate_registration_is_rejected() {
        let mut registry = StageRegistry::new();
        registry.register("stt", Arc::new(NoopExecutor)).unwrap();
        assert_eq!(
            registry.register("stt", Arc::new(NoopExecutor)),
            Err(RegistryError::Duplicate(StageId("stt".into())))
        );
    }

    #[tokio::test]
    async fn missing_executor_fails_explicitly() {
        let registry = StageRegistry::new();
        let result = registry
            .execute(
                request("stt"),
                ExecutionContext {
                    task_root: ".".into(),
                    cancel: CancelToken::default(),
                },
            )
            .await;
        assert!(matches!(result, Err(ExecuteError::Failed(_))));
    }
}
