use thiserror::Error;
use tokio::task::JoinError;

#[derive(Debug, Error)]
pub enum TokioError {
    #[error("tokio task join failed")]
    Join(#[from] JoinError),
}

pub type Result<T> = std::result::Result<T, TokioError>;
