use crate::py_actor_system::PyActorSystemInner;
use crate::utils::py_value::PyValue;
use crate::utils::PyMessage;
use lyricore::{ActorRef, TokioRuntime};
use pyo3::{pyclass, pymethods, Bound, FromPyObject, PyAny, PyErr, PyObject, PyResult, Python};
use pyo3_async_runtimes::TaskLocals;
use std::sync::{Arc, Weak};

#[pyclass]
pub struct PyActorRef {
    inner: ActorRef,
    runtime: TokioRuntime,
    event_loop: Arc<TaskLocals>,
    system_inner: Option<Weak<PyActorSystemInner>>,
}

impl PyActorRef {
    pub fn new(
        actor_ref: ActorRef,
        runtime: TokioRuntime,
        event_loop: Arc<TaskLocals>,
        system_inner: Option<Weak<PyActorSystemInner>>,
    ) -> Self {
        Self {
            inner: actor_ref,
            runtime,
            event_loop,
            system_inner,
        }
    }

    pub fn self_ref(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            runtime: self.runtime.clone(),
            event_loop: self.event_loop.clone(),
            system_inner: self.system_inner.clone(),
        }
    }
    fn with_actor_system<F, R>(&self, f: F) -> Option<R>
    where
        F: FnOnce(&PyActorSystemInner) -> R,
    {
        self.system_inner
            .as_ref()
            .and_then(|weak| weak.upgrade())
            .map(|arc| f(&arc))
    }
}

#[pymethods]
impl PyActorRef {
    pub(crate) async fn tell(&self, message: PyObject) -> PyResult<()> {
        let inner = self.inner.clone();
        let rt = self.runtime.clone();
        rt.runtime
            .spawn(async move {
                let py_value = Python::with_gil(|py| PyValue::extract_bound(&message.bind(py)))?;
                let py_msg = PyMessage::new(py_value);
                inner.tell(py_msg).await.map_err(|e| {
                    PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(e.to_string())
                })?;
                Ok(())
            })
            .await
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(e.to_string()))?
    }
    pub(crate) async fn ask<'a>(
        &self,
        message: PyObject,
        timeout_ms: Option<u64>,
    ) -> PyResult<PyObject> {
        let inner = self.inner.clone();
        let rt = self.runtime.clone();
        let py_value = Python::with_gil(|py| PyValue::extract_bound(&message.bind(py)))?;
        let py_msg = PyMessage::new(py_value);
        let result = rt
            .runtime
            .spawn(async move {
                let result = if let Some(timeout) = timeout_ms {
                    tokio::time::timeout(
                        tokio::time::Duration::from_millis(timeout),
                        inner.ask(py_msg),
                    )
                    .await
                    .map_err(|_| {
                        PyErr::new::<pyo3::exceptions::PyTimeoutError, _>("Request timeout")
                    })?
                } else {
                    Ok(inner.ask(py_msg).await?)
                };
                result
            })
            .await
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(e.to_string()))??;
        Python::with_gil(|py| result.to_python(py))
    }

    fn stop<'a>(&self, py: Python<'a>) -> PyResult<Bound<'a, PyAny>> {
        match self.inner {
            ActorRef::Local(ref local_ref) => self
                .with_actor_system(|sys| sys.stop(py, local_ref.actor_id().clone()))
                .ok_or(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                    "Actor system is not available or has been dropped",
                ))?,
            ActorRef::Remote(_) => {
                // No-op for remote actors
                pyo3_async_runtimes::tokio::future_into_py(py, async move { Ok(()) })
            }
        }
    }

    #[getter]
    fn path(&self) -> String {
        self.inner.actor_path().full_path()
    }

    async fn async_ping(&self) -> bool {
        true
    }
    async fn async_ping_with_str(&self, s: String) -> String {
        s
    }
}
