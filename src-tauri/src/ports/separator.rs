use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;

use crate::pipeline::runner::CancelToken;

pub type SeparatorFuture<'a> =
    Pin<Box<dyn Future<Output = Result<SeparationOutput, SeparatorError>> + Send + 'a>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeparationOutput {
    pub vocals: PathBuf,
    pub background: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SeparatorError {
    Canceled,
    InvalidInput(String),
    ModelUnavailable(String),
    Engine(String),
}

pub trait AudioSeparator: Send + Sync {
    fn version(&self) -> String;

    fn resource_cost(&self) -> crate::scheduler::ResourceCost {
        crate::scheduler::ResourceCost::default()
    }

    fn separate<'a>(
        &'a self,
        input: &'a Path,
        staging_dir: &'a Path,
        cancel: &'a CancelToken,
    ) -> SeparatorFuture<'a>;
}

pub fn validate_separation_output(
    output: &SeparationOutput,
    staging_dir: &Path,
) -> Result<(), SeparatorError> {
    for path in [&output.vocals, &output.background] {
        if !path.starts_with(staging_dir) {
            return Err(SeparatorError::Engine("分离产物逃逸 staging 目录".into()));
        }
        let metadata =
            std::fs::metadata(path).map_err(|error| SeparatorError::Engine(error.to_string()))?;
        if !metadata.is_file() || metadata.len() <= 44 {
            return Err(SeparatorError::Engine("分离产物为空或不是文件".into()));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_outputs_outside_transaction_staging() {
        let root = std::env::temp_dir().join(format!("separator-port-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let outside = root.parent().unwrap().join("outside.wav");
        std::fs::write(&outside, vec![0u8; 64]).unwrap();
        let output = SeparationOutput {
            vocals: outside.clone(),
            background: outside.clone(),
        };
        assert!(validate_separation_output(&output, &root).is_err());
        std::fs::remove_file(outside).unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }
}
