use thiserror::Error;

pub type Result<T> = std::result::Result<T, LyricoreActorError>;

#[derive(Error, Debug)]
pub enum LyricoreActorError {
    #[error("Actor error: {0}")]
    Actor(#[from] ActorError),

    #[error("Communication error: {0}")]
    Communication(#[from] CommunicationError),

    #[error("System error: {0}")]
    System(#[from] SystemError),

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Configuration error: {0}")]
    Configuration(String),

    #[error("Runtime error: {0}")]
    Runtime(String),

    #[error("Python error: {0}")]
    Python(String),
}

#[derive(Error, Debug)]
pub enum ActorError {
    #[error("Actor not found: {0}")]
    ActorNotFound(String),

    #[error("Actor stopped: {0}")]
    ActorStopped(String),

    #[error("Message send failed: {0}")]
    MessageSendFailed(String),

    #[error("Actor creation failed: {0}")]
    ActorCreationFailed(String),

    #[error("Context error: {0}")]
    ContextError(String),

    #[error("Invalid actor state: {0}")]
    InvalidState(String),

    #[error("RPC error: {0}")]
    RpcError(String),
}

#[derive(Error, Debug)]
pub enum CommunicationError {
    #[error("Connection failed: {0}")]
    ConnectionFailed(String),

    #[error("Serialization error: {0}")]
    SerializationError(String),

    #[error("Deserialization error: {0}")]
    DeserializationError(String),

    #[error("Timeout: {0}")]
    Timeout(String),

    #[error("Remote error: {0}")]
    RemoteError(String),

    #[error("Message routing failed: {0}")]
    RoutingFailed(String),
}

#[derive(Error, Debug)]
pub enum SystemError {
    #[error("Configuration error: {0}")]
    ConfigurationError(String),

    #[error("Resource exhausted: {0}")]
    ResourceExhausted(String),

    #[error("System shutdown")]
    SystemShutdown,

    #[error("Internal error: {0}")]
    InternalError(String),

    #[error("Runtime error: {0}")]
    RuntimeError(String),
}

impl From<tonic::Status> for LyricoreActorError {
    fn from(status: tonic::Status) -> Self {
        LyricoreActorError::Communication(CommunicationError::RemoteError(
            status.message().to_string(),
        ))
    }
}

impl From<serde_json::Error> for LyricoreActorError {
    fn from(err: serde_json::Error) -> Self {
        LyricoreActorError::Serialization(err.to_string())
    }
}

impl From<std::io::Error> for LyricoreActorError {
    fn from(err: std::io::Error) -> Self {
        LyricoreActorError::System(SystemError::InternalError(err.to_string()))
    }
}

impl From<tonic::transport::Error> for LyricoreActorError {
    fn from(err: tonic::transport::Error) -> Self {
        LyricoreActorError::Communication(CommunicationError::ConnectionFailed(err.to_string()))
    }
}

impl From<std::net::AddrParseError> for LyricoreActorError {
    fn from(err: std::net::AddrParseError) -> Self {
        LyricoreActorError::System(SystemError::ConfigurationError(err.to_string()))
    }
}

#[cfg(feature = "pyo3")]
mod pyo3_conversions {
    use super::*;
    use pyo3::PyErr;

    // PyErr -> LyricoreActorError
    impl From<PyErr> for LyricoreActorError {
        fn from(py_err: PyErr) -> Self {
            LyricoreActorError::Runtime(py_err.to_string())
        }
    }

    // PyErr -> ActorError
    impl From<PyErr> for ActorError {
        fn from(py_err: PyErr) -> Self {
            ActorError::RpcError(py_err.to_string())
        }
    }

    // PyErr -> SystemError
    impl From<PyErr> for SystemError {
        fn from(py_err: PyErr) -> Self {
            SystemError::RuntimeError(py_err.to_string())
        }
    }

    // PyErr -> CommunicationError
    impl From<PyErr> for CommunicationError {
        fn from(py_err: PyErr) -> Self {
            CommunicationError::SerializationError(py_err.to_string())
        }
    }

    // LyricoreActorError -> PyErr
    impl From<LyricoreActorError> for PyErr {
        fn from(lyric_err: LyricoreActorError) -> Self {
            pyo3::exceptions::PyRuntimeError::new_err(lyric_err.to_string())
        }
    }

    impl From<ActorError> for PyErr {
        fn from(actor_err: ActorError) -> Self {
            pyo3::exceptions::PyRuntimeError::new_err(actor_err.to_string())
        }
    }
}

#[cfg(feature = "pyo3")]
#[macro_export]
macro_rules! py_try {
    ($expr:expr) => {
        $expr?
    };
}

#[cfg(feature = "pyo3")]
#[macro_export]
macro_rules! py_context {
    ($expr:expr, $context:literal) => {
        $expr
            .map_err(|e: pyo3::PyErr| LyricoreActorError::Runtime(format!("{}: {}", $context, e)))?
    };
}
