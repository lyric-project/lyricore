use crate::object_store::PyObjectStore;
use crate::py_actor_ref::PyActorRef;
use crate::py_actor_system::PyActorSystemInner;
use lyricore::error::LyricoreActorError;
use lyricore::{ActorContext, ActorError, ActorRef, TokioRuntime};
use pyo3::types::{PyDict, PyTuple};
use pyo3::{pyclass, pymethods, Bound, PyAny, PyErr, PyObject, PyResult, Python};
use pyo3_async_runtimes::TaskLocals;
use std::sync::{Arc, Weak};

#[pyclass]
#[derive(Clone)]
pub struct PyInnerContext {
    system_inner: Option<Weak<PyActorSystemInner>>,
}

impl PyInnerContext {
    pub fn new(system_inner: Option<Weak<PyActorSystemInner>>) -> Self {
        Self { system_inner }
    }

    pub fn with_actor_system<F, R>(&self, f: F) -> Option<R>
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
impl PyInnerContext {
    fn actor_of<'a>(&self, py: Python<'a>, path: String) -> PyResult<Bound<'a, PyAny>> {
        self.with_actor_system(|sys| sys.actor_of(py, path))
            .ok_or(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                "Actor system is not available or has been dropped",
            ))?
    }

    fn get_store(&self) -> PyResult<PyObjectStore> {
        self.with_actor_system(|sys| sys.store.clone())
            .ok_or(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                "Actor system is not available or has been dropped",
            ))
    }
}

#[pyclass]
#[derive(Clone)]
pub struct PyActorContext {
    pub(crate) actor_id: String,
    pub(crate) actor_ref: Arc<PyActorRef>,
    inner_ctx: PyInnerContext,
    system_inner: Option<Weak<PyActorSystemInner>>,
}

impl PyActorContext {
    pub fn new(
        ctx: &ActorContext,
        runtime: TokioRuntime,
        event_loop: Arc<TaskLocals>,
        inner_ctx: PyInnerContext,
        system_inner: Option<Weak<PyActorSystemInner>>,
    ) -> Self {
        let self_ref =
            PyActorRef::new(ActorRef::Local(ctx.self_ref().clone()), runtime, event_loop);
        PyActorContext {
            actor_id: ctx.actor_id().to_string(),
            actor_ref: Arc::new(self_ref),
            inner_ctx,
            system_inner,
        }
    }

    fn get_actor_system(&self) -> Option<Arc<PyActorSystemInner>> {
        self.system_inner.as_ref().and_then(|weak| weak.upgrade())
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
impl PyActorContext {
    #[getter]
    fn actor_id(&self) -> &str {
        &self.actor_id
    }

    async fn tell_self(&self, message: PyObject) -> PyResult<()> {
        self.actor_ref.tell(message).await
    }

    #[getter]
    fn self_ref(&self) -> PyActorRef {
        self.actor_ref.self_ref()
    }

    #[pyo3(signature = (actor_class, path, args=None, kwargs=None))]
    fn spawn<'a>(
        &self,
        py: Python<'a>,
        actor_class: PyObject,
        path: String,
        args: Option<&Bound<PyTuple>>,
        kwargs: Option<&Bound<PyDict>>,
    ) -> PyResult<Bound<'a, PyAny>> {
        self.with_actor_system(|sys| sys.spawn(py, actor_class, path, args, kwargs))
            .ok_or(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                "Actor system is not available or has been dropped",
            ))?
    }
    fn actor_of<'a>(&self, py: Python<'a>, path: String) -> PyResult<Bound<'a, PyAny>> {
        self.with_actor_system(|sys| sys.actor_of(py, path))
            .ok_or(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                "Actor system is not available or has been dropped",
            ))?
    }

    fn spawn_from_construction_task<'a>(
        &self,
        py: Python<'a>,
        task_dict: &Bound<PyDict>,
        path: String,
    ) -> PyResult<Bound<'a, PyAny>> {
        self.with_actor_system(|sys| sys.spawn_from_construction_task(py, task_dict, path))
            .ok_or(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                "Actor system is not available or has been dropped",
            ))?
    }
    fn get_store(&self) -> PyResult<PyObjectStore> {
        self.with_actor_system(|sys| sys.store.clone())
            .ok_or(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                "Actor system is not available or has been dropped",
            ))
    }

    fn get_inner_ctx(&self) -> PyInnerContext {
        self.inner_ctx.clone()
    }
}
