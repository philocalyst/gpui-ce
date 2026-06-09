pub type Result<T> = std::result::Result<T, XtaskError>;

#[derive(Debug, thiserror::Error)]
pub enum XtaskError {
    #[error("I/O error")]
    Io(#[from] std::io::Error),

    #[error("cargo metadata error")]
    CargoMetadata(#[from] cargo_metadata::Error),

    #[error("cargo manifest error")]
    CargoToml(#[from] cargo_toml::Error),

    #[error("TOML deserialization error")]
    TomlDeserialize(#[from] toml::de::Error),

    #[error("TOML edit error")]
    TomlEdit(#[from] toml_edit::TomlError),

    #[error("YAML error")]
    SerdeYaml(#[from] serde_yaml::Error),

    #[error("JSON error")]
    Json(#[from] serde_json::Error),

    #[error("workflow serialization failed")]
    WorkflowSerialize { path: String, details: String },

    #[error("command failed")]
    CommandFailed { command: String, exit_code: Option<i32> },

    #[error("task error")]
    Message { details: String },

    #[error("workflow check failed")]
    WorkflowCheckFailed,

    #[error("workflow parse failed")]
    WorkflowParseSummary { details: String },
}
