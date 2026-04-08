use thiserror::Error;

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("runtime feature is not available yet: {0}")]
    Unsupported(&'static str),
}

pub type RuntimeResult<T> = std::result::Result<T, RuntimeError>;
