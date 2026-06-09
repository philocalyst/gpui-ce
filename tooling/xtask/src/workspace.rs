use crate::error::Result;
use cargo_metadata::{Metadata, MetadataCommand};

/// Returns the Cargo workspace.
pub fn load_workspace() -> Result<Metadata> {
    Ok(MetadataCommand::new().exec()?)
}
