use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("internal error: {0}")]
    Internal(String),
}
