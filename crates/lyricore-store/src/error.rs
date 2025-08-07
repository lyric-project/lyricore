use super::ObjectId;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("Object not found: {0:?}")]
    ObjectNotFound(ObjectId),

    #[error("Object too large: {size} bytes (max: {max_size})")]
    ObjectTooLarge { size: usize, max_size: usize },

    #[error("Out of memory: using {current} bytes (max: {max})")]
    OutOfMemory { current: usize, max: usize },

    #[error("Internal error: {0}")]
    Internal(String),
}

pub type Result<T> = std::result::Result<T, StoreError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        let id = ObjectId::new();
        let error = StoreError::ObjectNotFound(id);
        assert!(error.to_string().contains(&id.to_string()));

        let error = StoreError::ObjectTooLarge {
            size: 1024,
            max_size: 512,
        };
        assert!(error.to_string().contains("1024"));
        assert!(error.to_string().contains("512"));

        let error = StoreError::OutOfMemory {
            current: 2048,
            max: 1024,
        };
        assert!(error.to_string().contains("2048"));
        assert!(error.to_string().contains("1024"));

        let error = StoreError::Internal("test error".to_string());
        assert!(error.to_string().contains("test error"));
    }
}
