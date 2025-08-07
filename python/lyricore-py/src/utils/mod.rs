use lyricore::error::LyricoreActorError;
use lyricore::{ActorError, Message};
use py_value::PyValue;
use pyo3::prelude::PyAnyMethods;
use pyo3::{Bound, FromPyObject, IntoPyObject, IntoPyObjectExt, PyAny, PyObject, PyResult, Python};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub mod py_value;

// A task for constructing a Python actor
#[derive(Clone, Debug)]
pub struct PyActorConstructionTask {
    pub constructor_func: String, // base64 encoded constructor function
    pub constructor_args: Vec<PyValue>, // The positional arguments for the constructor function
    pub constructor_kwargs: HashMap<String, PyValue>, // The keyword arguments for the constructor function
    pub function_hash: String,                        // The hash of the function to be called
    pub module_name: String,                          // Module name
    pub class_name: String,                           // Class name
    pub capture_globals: Option<String>,              // The globals to be captured
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PyActorDescriptor {
    pub module_name: String,
    pub class_name: String,
    pub qualname: String,
    pub actor_id: String,
    pub checkpoint_data: Option<Vec<u8>>,

    pub serialized_class_b64: String, // base64 encoded serialized class
    pub execution_context_b64: Option<String>, // base64 encoded execution context
    pub is_local_class: bool,         // Whether the class is a local class
    pub class_hash: String,           // The hash of the class, used for deduplication

    pub init_args: Vec<PyValue>,
    pub init_kwargs: HashMap<String, PyValue>,

    #[serde(default)]
    pub constructor_func: String,
    #[serde(default)]
    pub constructor_args: Vec<PyValue>,
    #[serde(default)]
    pub constructor_kwargs: HashMap<String, PyValue>,
    #[serde(default)]
    pub function_hash: String,
    #[serde(default)]
    pub capture_globals: Option<String>,
}

impl PyActorDescriptor {
    pub fn from_construction_task(task: PyActorConstructionTask) -> Self {
        Self {
            module_name: task.module_name.clone(),
            class_name: task.class_name.clone(),
            qualname: task.class_name.clone(),
            actor_id: uuid::Uuid::new_v4().to_string(),
            checkpoint_data: None,

            // Building from a construction task
            constructor_func: task.constructor_func,
            constructor_args: task.constructor_args,
            constructor_kwargs: task.constructor_kwargs,
            function_hash: task.function_hash.clone(),
            capture_globals: task.capture_globals,

            // TODO: Maybe we can delete these fields
            serialized_class_b64: String::new(),
            execution_context_b64: None,
            is_local_class: task.class_name.contains(".<locals>."),
            class_hash: task.function_hash.clone(),
            init_args: vec![], // All arguments are passed through the constructor function
            init_kwargs: HashMap::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PyMessage {
    inner: PyValue,
}

impl PyMessage {
    pub fn new(value: PyValue) -> Self {
        Self { inner: value }
    }

    pub fn to_python(&self, py: Python) -> lyricore::Result<PyObject> {
        match self.inner.clone().into_pyobject(py) {
            Ok(obj) => Ok(obj.into_py_any(py)?),
            Err(e) => Err(LyricoreActorError::Actor(ActorError::RpcError(
                e.to_string(),
            ))),
        }
    }
}

impl Message for PyMessage {
    type Response = PyResponse;
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct PyResponse {
    inner: Option<PyValue>,
}

impl PyResponse {
    pub fn none() -> Self {
        Self { inner: None }
    }

    pub fn from_python(obj: Bound<PyAny>) -> lyricore::Result<Self> {
        if obj.is_none() {
            Ok(Self::none())
        } else {
            let value = PyValue::extract_bound(&obj)?;
            Ok(Self { inner: Some(value) })
        }
    }

    pub fn to_python(&self, py: Python) -> PyResult<PyObject> {
        match self.inner.clone() {
            Some(value) => value.into_pyobject(py).map(|obj| obj.into_py_any(py))?,
            None => Ok(py.None().into_py_any(py)?),
        }
    }
}
