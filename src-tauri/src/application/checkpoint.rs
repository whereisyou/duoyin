use std::sync::Arc;

use tokio::sync::Mutex;

use crate::domain::ids::TaskId;
use crate::domain::manifest::TaskManifest;
use crate::domain::task::{ChildStatus, ParentStatus};
use crate::infra::task_store::{LoadedTask, TaskDocument, TaskStore};
use crate::pipeline::runner::{CheckpointFuture, PipelineCheckpoint};

pub struct TaskStoreCheckpoint {
    store: Arc<TaskStore>,
    task: Mutex<TaskDocument>,
}

pub fn recover_task(store: &TaskStore, task_id: &TaskId) -> Result<LoadedTask, String> {
    let mut loaded = store
        .load_bundle(task_id)
        .map_err(|error| error.to_string())?;
    let interrupted = loaded.manifest.recover_interrupted();
    let parent_running = loaded.task.parent.status == ParentStatus::Running;
    let child_running = loaded
        .task
        .children
        .iter()
        .any(|child| child.status == ChildStatus::Running);
    if interrupted == 0 && !parent_running && !child_running {
        return Ok(loaded);
    }

    if parent_running {
        loaded.task.parent.status = ParentStatus::Pending;
    }
    for child in &mut loaded.task.children {
        if child.status == ChildStatus::Running {
            child.status = ChildStatus::Pending;
        }
    }
    store
        .save_bundle(&mut loaded.task, &loaded.manifest)
        .map_err(|error| error.to_string())?;
    Ok(loaded)
}

impl TaskStoreCheckpoint {
    pub fn new(store: Arc<TaskStore>, task: TaskDocument) -> Self {
        Self {
            store,
            task: Mutex::new(task),
        }
    }
}

impl PipelineCheckpoint for TaskStoreCheckpoint {
    fn save<'a>(&'a self, manifest: TaskManifest) -> CheckpointFuture<'a> {
        Box::pin(async move {
            let mut task = self.task.lock().await;
            self.store
                .save_bundle(&mut task, &manifest)
                .map(|_| ())
                .map_err(|error| error.to_string())
        })
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::domain::config::{EngineSelection, OutputConfig, PipelineConfig, SeparationConfig};
    use crate::domain::ids::{ChildTaskId, TaskId};
    use crate::domain::manifest::{StageStatus, TaskManifest};
    use crate::domain::media::{SourceFingerprint, SourceVideo};
    use crate::domain::task::{ChildStatus, ChildTask, ParentStatus, ParentTask};
    use crate::domain::variant::TargetVariant;
    use crate::pipeline::graph::PipelineGraph;
    use crate::pipeline::runner::{
        ArtifactOutput, CancelToken, ExecuteFuture, ExecutionContext, ExecutionOutcome,
        PipelineRunner, RunScope, StageExecutor, StageRequest,
    };

    struct EmptyExecutor;

    impl StageExecutor for EmptyExecutor {
        fn version(&self, _stage: &crate::domain::ids::StageId) -> String {
            "empty-v1".into()
        }

        fn execute<'a>(
            &'a self,
            _request: StageRequest,
            _context: ExecutionContext,
        ) -> ExecuteFuture<'a> {
            Box::pin(async { Ok(ExecutionOutcome::Done(Vec::<ArtifactOutput>::new())) })
        }
    }

    fn fingerprint() -> SourceFingerprint {
        SourceFingerprint {
            size: 1,
            modified: 1,
            content_hash: Some("source".into()),
            hash_algo_version: 1,
        }
    }

    fn task_document() -> TaskDocument {
        let task_id = TaskId("p1".into());
        let child_id = ChildTaskId("p1-zh-CN".into());
        let variant = TargetVariant::zh_mandarin();
        TaskDocument::new(
            ParentTask {
                id: task_id.clone(),
                source: SourceVideo {
                    path: PathBuf::from("source.mp4"),
                    fingerprint: fingerprint(),
                },
                status: ParentStatus::Running,
                children: vec![child_id.clone()],
                created_at: 1,
                updated_at: 1,
            },
            vec![ChildTask {
                id: child_id,
                parent_id: task_id,
                variant: variant.clone(),
                status: ChildStatus::Pending,
                created_at: 1,
                updated_at: 1,
            }],
            PipelineConfig {
                source_language: None,
                targets: vec![variant],
                engines: EngineSelection {
                    stt: "fake".into(),
                    translator: "fake".into(),
                    tts: "fake".into(),
                    separator: None,
                },
                separation: SeparationConfig::default(),
                output: OutputConfig::default(),
            },
        )
    }

    #[tokio::test]
    async fn startup_recovers_running_state_as_interrupted_and_pending() {
        let root = std::env::temp_dir().join(format!("recover-{}", uuid::Uuid::new_v4()));
        let store = TaskStore::new(&root);
        let mut task = task_document();
        task.children[0].status = ChildStatus::Running;
        let mut manifest = TaskManifest::new(TaskId("p1".into()), fingerprint());
        let mut stage = crate::domain::manifest::StageRecord::done("stt", "h", vec![]);
        stage.status = StageStatus::Running;
        manifest.add_stage(stage);
        store.save_bundle(&mut task, &manifest).unwrap();

        let recovered = recover_task(&store, &TaskId("p1".into())).unwrap();

        assert_eq!(recovered.task.parent.status, ParentStatus::Pending);
        assert_eq!(recovered.task.children[0].status, ChildStatus::Pending);
        assert_eq!(
            recovered.manifest.stages[&crate::domain::ids::StageId("stt".into())].status,
            StageStatus::Interrupted
        );
        assert_eq!(
            TaskStore::new(&root)
                .load_bundle(&TaskId("p1".into()))
                .unwrap()
                .task
                .revision,
            2
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn runner_stage_boundaries_are_persisted_to_task_store() {
        let root = std::env::temp_dir().join(format!("checkpoint-{}", uuid::Uuid::new_v4()));
        let store = Arc::new(TaskStore::new(&root));
        let task = task_document();
        let checkpoint = Arc::new(TaskStoreCheckpoint::new(store.clone(), task.clone()));
        let runner = PipelineRunner::new(
            PipelineGraph::video_translation(),
            task.config.clone(),
            TaskManifest::new(TaskId("p1".into()), fingerprint()),
            Arc::new(EmptyExecutor),
        )
        .with_checkpoint(checkpoint);

        runner
            .run_named(RunScope::Parent, "media_probe", &CancelToken::default())
            .await
            .unwrap();

        let reopened = TaskStore::new(&root)
            .load_bundle(&TaskId("p1".into()))
            .unwrap();
        assert_eq!(reopened.task.revision, 2);
        assert_eq!(
            reopened.manifest.stages[&crate::domain::ids::StageId("media_probe".into())].status,
            StageStatus::Done
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
