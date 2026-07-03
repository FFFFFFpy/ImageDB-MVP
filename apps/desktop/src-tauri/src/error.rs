use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
#[allow(dead_code)]
pub enum AppError {
    #[error("internal error: {0}")]
    Internal(String),

    #[error("postgres not available: {0}")]
    PostgresUnavailable(String),

    #[error("image error: {0}")]
    ImageError(String),

    #[error("io error: {0}")]
    IoError(String),

    #[error(
        "incomplete transaction {0} detected; route to recovery instead of starting a new commit"
    )]
    ResumeRequired(Uuid),
}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        AppError::IoError(e.to_string())
    }
}

impl From<image::ImageError> for AppError {
    fn from(e: image::ImageError) -> Self {
        AppError::ImageError(e.to_string())
    }
}

impl serde::Serialize for AppError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}
