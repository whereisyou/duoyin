use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;

use crate::domain::artifact::ArtifactKind;
use crate::domain::ids::StageId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeScope {
    Parent,
    Target,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageNode {
    pub id: StageId,
    pub scope: NodeScope,
    pub depends_on: Vec<StageId>,
    pub outputs: Vec<ArtifactKind>,
    pub optional: bool,
}

impl StageNode {
    pub fn new(
        id: &str,
        scope: NodeScope,
        depends_on: &[&str],
        outputs: Vec<ArtifactKind>,
    ) -> Self {
        Self {
            id: StageId(id.into()),
            scope,
            depends_on: depends_on
                .iter()
                .map(|value| StageId((*value).into()))
                .collect(),
            outputs,
            optional: false,
        }
    }

    pub fn optional(mut self) -> Self {
        self.optional = true;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GraphError {
    DuplicateStage(StageId),
    MissingDependency { stage: StageId, dependency: StageId },
    ScopeViolation { stage: StageId, dependency: StageId },
    Cycle,
    UnknownStage(StageId),
}

impl fmt::Display for GraphError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateStage(id) => write!(f, "重复流水线节点: {}", id.0),
            Self::MissingDependency { stage, dependency } => {
                write!(f, "节点 {} 缺少依赖 {}", stage.0, dependency.0)
            }
            Self::ScopeViolation { stage, dependency } => write!(
                f,
                "父任务节点 {} 不能依赖子任务节点 {}",
                stage.0, dependency.0
            ),
            Self::Cycle => f.write_str("流水线 DAG 存在环"),
            Self::UnknownStage(id) => write!(f, "未知流水线节点: {}", id.0),
        }
    }
}

impl std::error::Error for GraphError {}

#[derive(Debug, Clone)]
pub struct PipelineGraph {
    nodes: BTreeMap<StageId, StageNode>,
    topological: Vec<StageId>,
}

impl PipelineGraph {
    pub fn new(nodes: Vec<StageNode>) -> Result<Self, GraphError> {
        let mut by_id = BTreeMap::new();
        for node in nodes {
            if by_id.insert(node.id.clone(), node).is_some() {
                return Err(GraphError::DuplicateStage(
                    by_id.keys().next_back().unwrap().clone(),
                ));
            }
        }

        for node in by_id.values() {
            for dependency in &node.depends_on {
                let Some(upstream) = by_id.get(dependency) else {
                    return Err(GraphError::MissingDependency {
                        stage: node.id.clone(),
                        dependency: dependency.clone(),
                    });
                };
                if node.scope == NodeScope::Parent && upstream.scope == NodeScope::Target {
                    return Err(GraphError::ScopeViolation {
                        stage: node.id.clone(),
                        dependency: dependency.clone(),
                    });
                }
            }
        }

        let topological = topological_sort(&by_id)?;
        Ok(Self {
            nodes: by_id,
            topological,
        })
    }

    pub fn video_translation() -> Self {
        Self::new(vec![
            StageNode::new(
                "media_probe",
                NodeScope::Parent,
                &[],
                vec![ArtifactKind::MediaInfo],
            ),
            StageNode::new(
                "extract_audio",
                NodeScope::Parent,
                &["media_probe"],
                vec![ArtifactKind::ExtractedAudio],
            ),
            StageNode::new(
                "stt",
                NodeScope::Parent,
                &["extract_audio"],
                vec![ArtifactKind::Segments],
            ),
            StageNode::new(
                "separation",
                NodeScope::Parent,
                &["media_probe"],
                vec![ArtifactKind::VocalsRaw, ArtifactKind::BackgroundRaw],
            )
            .optional(),
            StageNode::new(
                "translate",
                NodeScope::Target,
                &["stt"],
                vec![ArtifactKind::TranslatedSegments],
            ),
            StageNode::new(
                "tts",
                NodeScope::Target,
                &["translate"],
                vec![ArtifactKind::DubAudio],
            ),
            StageNode::new(
                "mix",
                NodeScope::Target,
                &["tts", "separation"],
                vec![ArtifactKind::MixedAudio],
            ),
            StageNode::new(
                "srt",
                NodeScope::Target,
                &["translate"],
                vec![ArtifactKind::SubtitleSrt],
            ),
            StageNode::new(
                "final_video",
                NodeScope::Target,
                &["mix", "srt"],
                vec![ArtifactKind::FinalVideo],
            )
            .optional(),
        ])
        .expect("内置视频翻译 DAG 必须有效")
    }

    pub fn node(&self, id: &StageId) -> Result<&StageNode, GraphError> {
        self.nodes
            .get(id)
            .ok_or_else(|| GraphError::UnknownStage(id.clone()))
    }

    /// 拓扑序视图：测试与诊断用（executor 内部直接用字段），保留
    #[allow(dead_code)]
    pub fn topological(&self) -> &[StageId] {
        &self.topological
    }

    pub fn descendants(&self, stage_id: &StageId) -> Result<Vec<StageId>, GraphError> {
        self.node(stage_id)?;
        let mut descendants = BTreeSet::new();
        let mut queue = VecDeque::from([stage_id.clone()]);
        while let Some(current) = queue.pop_front() {
            for node in self.nodes.values() {
                if node.depends_on.contains(&current) && descendants.insert(node.id.clone()) {
                    queue.push_back(node.id.clone());
                }
            }
        }
        Ok(self
            .topological
            .iter()
            .filter(|id| descendants.contains(*id))
            .cloned()
            .collect())
    }
}

fn topological_sort(nodes: &BTreeMap<StageId, StageNode>) -> Result<Vec<StageId>, GraphError> {
    let mut indegree: BTreeMap<_, usize> = nodes
        .iter()
        .map(|(id, node)| (id.clone(), node.depends_on.len()))
        .collect();
    let mut ready: VecDeque<_> = indegree
        .iter()
        .filter(|(_, degree)| **degree == 0)
        .map(|(id, _)| id.clone())
        .collect();
    let mut sorted = Vec::with_capacity(nodes.len());

    while let Some(current) = ready.pop_front() {
        sorted.push(current.clone());
        for node in nodes.values() {
            if node.depends_on.contains(&current) {
                let degree = indegree.get_mut(&node.id).expect("node exists");
                *degree -= 1;
                if *degree == 0 {
                    ready.push_back(node.id.clone());
                }
            }
        }
    }

    if sorted.len() != nodes.len() {
        Err(GraphError::Cycle)
    } else {
        Ok(sorted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_graph_is_topological_and_final_depends_on_outputs() {
        let graph = PipelineGraph::video_translation();
        let order = graph.topological();
        let position = |name: &str| order.iter().position(|id| id.0 == name).unwrap();
        assert!(position("extract_audio") < position("stt"));
        assert!(position("translate") < position("tts"));
        assert!(position("mix") < position("final_video"));
        assert!(position("srt") < position("final_video"));
    }

    #[test]
    fn descendants_are_derived_from_graph_not_hardcoded() {
        let graph = PipelineGraph::video_translation();
        let descendants = graph.descendants(&StageId("translate".into())).unwrap();
        let names: BTreeSet<_> = descendants.iter().map(|id| id.0.as_str()).collect();
        assert_eq!(names, BTreeSet::from(["tts", "srt", "mix", "final_video"]));
        let position = |name: &str| descendants.iter().position(|id| id.0 == name).unwrap();
        assert!(position("tts") < position("mix"));
        assert!(position("mix") < position("final_video"));
        assert!(position("srt") < position("final_video"));
    }

    #[test]
    fn rejects_cycle() {
        let result = PipelineGraph::new(vec![
            StageNode::new("a", NodeScope::Parent, &["b"], vec![]),
            StageNode::new("b", NodeScope::Parent, &["a"], vec![]),
        ]);
        assert!(matches!(result, Err(GraphError::Cycle)));
    }

    #[test]
    fn parent_cannot_depend_on_target_node() {
        let result = PipelineGraph::new(vec![
            StageNode::new("child", NodeScope::Target, &[], vec![]),
            StageNode::new("parent", NodeScope::Parent, &["child"], vec![]),
        ]);
        assert!(matches!(result, Err(GraphError::ScopeViolation { .. })));
    }
}
