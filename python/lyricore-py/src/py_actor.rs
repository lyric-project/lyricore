use pyo3::prelude::*;
use pyo3::types::{PyDict, PyTuple};
use pyo3_async_runtimes::TaskLocals;
use std::sync::{Arc, Weak};
use tokio::sync::{mpsc, Mutex};

use crate::py_actor_context::{PyActorContext, PyInnerContext};
use crate::py_actor_system::{LangWorkerMessage, PyActorSystemInner};
use crate::utils::{PyActorConstructionTask, PyActorDescriptor, PyMessage, PyResponse};
use lyricore::error::{ActorError, LyricoreActorError, Result};
use lyricore::{Actor, ActorContext, TokioRuntime};

#[derive(Clone)]
struct CachedMethods {
    on_start: Option<PyObject>,
    on_message: Option<PyObject>,
    handle_message: Option<PyObject>,
    on_stop: Option<PyObject>,
    get_state: Option<PyObject>,
    set_state: Option<PyObject>,
}

fn cache_actor_methods_and_instance(actor_instance: &Bound<PyAny>) -> Result<CachedMethods> {
    // Cache the methods from the actor instance
    let methods = CachedMethods {
        on_start: actor_instance.getattr("on_start").ok().map(|m| m.unbind()),
        on_message: actor_instance
            .getattr("on_message")
            .ok()
            .map(|m| m.unbind()),
        handle_message: actor_instance
            .getattr("handle_message")
            .ok()
            .map(|m| m.unbind()),
        on_stop: actor_instance.getattr("on_stop").ok().map(|m| m.unbind()),
        get_state: actor_instance.getattr("get_state").ok().map(|m| m.unbind()),
        set_state: actor_instance.getattr("set_state").ok().map(|m| m.unbind()),
    };
    Ok(methods)
}
fn execute_construction_task(
    descriptor: &PyActorDescriptor,
    py: Python,
) -> Result<(PyObject, CachedMethods)> {
    tracing::debug!("Executing construction task for actor");

    // Import the serialization module
    let serialization_module = py.import("lyricore.serialization").map_err(|e| {
        LyricoreActorError::Actor(ActorError::RpcError(format!(
            "Failed to import serialization module: {}",
            e
        )))
    })?;

    let task_dict = PyDict::new(py);
    task_dict
        .set_item("constructor_func", &descriptor.constructor_func)
        .unwrap();
    task_dict
        .set_item("constructor_args", &descriptor.constructor_args)
        .unwrap();
    task_dict
        .set_item("constructor_kwargs", &descriptor.constructor_kwargs)
        .unwrap();
    task_dict
        .set_item("function_hash", &descriptor.function_hash)
        .unwrap();
    task_dict
        .set_item("module_name", &descriptor.module_name)
        .unwrap();
    task_dict
        .set_item("class_name", &descriptor.class_name)
        .unwrap();

    if let Some(ref globals) = descriptor.capture_globals {
        task_dict.set_item("capture_globals", globals).unwrap();
    }

    // Get the deserialize_and_create_actor function from the module
    let create_actor_func = serialization_module
        .getattr("deserialize_and_create_actor")
        .map_err(|e| {
            LyricoreActorError::Actor(ActorError::RpcError(format!(
                "Failed to get deserialize_and_create_actor function: {}",
                e
            )))
        })?;

    tracing::debug!("Calling deserialize_and_create_actor with task");

    // Execute the construction task to create the actor instance
    let actor_instance = create_actor_func.call1((task_dict,)).map_err(|e| {
        LyricoreActorError::Actor(ActorError::RpcError(format!(
            "Failed to execute construction task: {}",
            e
        )))
    })?;

    tracing::debug!("Successfully created actor instance");

    // Cache the methods from the actor instance
    let methods = cache_actor_methods_and_instance(&actor_instance)?;
    Ok((actor_instance.unbind(), methods))
}
#[derive(Clone)]
pub struct PyActorWrapper {
    descriptor: PyActorDescriptor,
    python_actor: Arc<Mutex<Option<PyObject>>>,
    cached_methods: Arc<Mutex<Option<CachedMethods>>>,
    runtime: TokioRuntime,
    event_loop: Arc<TaskLocals>,
    tx_task: Option<mpsc::UnboundedSender<LangWorkerMessage>>,
    system_inner: Option<Weak<PyActorSystemInner>>,
}

impl PyActorWrapper {
    pub fn new(
        descriptor: PyActorDescriptor,
        runtime: TokioRuntime,
        event_loop: Arc<TaskLocals>,
        tx_task: Option<mpsc::UnboundedSender<LangWorkerMessage>>,
        system_inner: Option<Weak<PyActorSystemInner>>,
    ) -> Self {
        Self {
            descriptor,
            python_actor: Arc::new(Mutex::new(None)),
            cached_methods: Arc::new(Mutex::new(None)),
            runtime,
            event_loop,
            tx_task,
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

    fn new_py_ctx(&self, ctx: &ActorContext) -> PyActorContext {
        PyActorContext::new(
            ctx,
            self.runtime.clone(),
            self.event_loop.clone(),
            self.new_inner_ctx(),
            self.system_inner.clone(),
        )
    }

    fn new_inner_ctx(&self) -> PyInnerContext {
        PyInnerContext::new(self.system_inner.clone())
    }

    pub fn new_from_construction_task(
        task: PyActorConstructionTask,
        runtime: TokioRuntime,
        event_loop: Arc<TaskLocals>,
        tx_task: Option<mpsc::UnboundedSender<LangWorkerMessage>>,
        system_inner: Option<Weak<PyActorSystemInner>>,
    ) -> Self {
        Self {
            descriptor: PyActorDescriptor::from_construction_task(task),
            python_actor: Arc::new(Mutex::new(None)),
            cached_methods: Arc::new(Mutex::new(None)),
            runtime,
            event_loop,
            tx_task,
            system_inner,
        }
    }

    async fn ensure_actor_loaded(&self) -> Result<()> {
        let rt = self.runtime.clone();
        let mut actor_guard = self.python_actor.lock().await;
        if actor_guard.is_some() {
            return Ok(());
        }

        let ctx = self.new_inner_ctx();
        let desc = self.descriptor.clone();
        let (actor_instance, methods) = rt
            .runtime
            .spawn_blocking(move || {
                Python::with_gil(|py| {
                    let py_module = py.import("lyricore")?;
                    py_module.call_method1("set_global_inner_context", (ctx,))?;
                    execute_construction_task(&desc, py)
                })
            })
            .await
            .map_err(|e| {
                LyricoreActorError::Actor(ActorError::RpcError(format!(
                    "Failed to execute construction task: {}",
                    e
                )))
            })??;

        *actor_guard = Some(actor_instance);
        *self.cached_methods.lock().await = Some(methods);
        Ok(())
    }
}

#[async_trait::async_trait]
impl Actor for PyActorWrapper {
    type Message = PyMessage;
    type State = ();

    async fn on_start(&mut self, ctx: &mut ActorContext) -> Result<()> {
        self.ensure_actor_loaded().await?;
        let py_ctx = self.new_py_ctx(ctx);
        let on_start_method = {
            let methods_guard = self.cached_methods.lock().await;
            Python::with_gil(|_| {
                methods_guard
                    .as_ref()
                    .and_then(|cached| cached.on_start.clone())
            })
        };

        if let Some(method) = on_start_method {
            tracing::debug!("Calling Python on_start method directly");
            let event_loop_opt = self.event_loop.clone();
            let inner_ctx = self.new_inner_ctx();
            let future = Python::with_gil(|py| -> PyResult<_> {
                let py_module = py.import("lyricore")?;
                py_module.call_method1("set_global_inner_context", (inner_ctx,))?;

                let py_ctx_obj = Py::new(py, py_ctx)?.into_any();
                // Invoke Python on_start method
                let result = method.call1(py, (py_ctx_obj,))?;
                pyo3_async_runtimes::into_future_with_locals(
                    event_loop_opt.as_ref(),
                    result.into_bound(py),
                )
            })?;
            let _ = future.await.map_err(|e| {
                tracing::warn!("Error in Python on_start method: {}", e);
                LyricoreActorError::Actor(ActorError::RpcError(format!(
                    "Error in Python on_start method: {}",
                    e
                )))
            })?;
        }
        Ok(())
    }

    async fn on_message(&mut self, msg: Self::Message, ctx: &mut ActorContext) -> Result<()> {
        self.ensure_actor_loaded().await?;
        let py_ctx = self.new_py_ctx(ctx);

        let on_message_method = {
            let methods_guard = self.cached_methods.lock().await;
            Python::with_gil(|py| {
                methods_guard
                    .as_ref()
                    .and_then(|cached| cached.on_message.clone())
            })
        };

        if let Some(method) = on_message_method {
            tracing::debug!("Calling Python on_message method");

            let event_loop = self.event_loop.clone();

            let py_msg_value = Python::with_gil(|py| msg.to_python(py))?;
            let inner_ctx = self.new_inner_ctx();

            let feature = Python::with_gil(|py| -> PyResult<_> {
                let py_module = py.import("lyricore")?;
                py_module.call_method1("set_global_inner_context", (inner_ctx,))?;
                let py_ctx_obj = Py::new(py, py_ctx)?.into_any();

                // Invoke the Python on_message method
                let result = method.call1(py, (py_msg_value, py_ctx_obj))?;
                pyo3_async_runtimes::into_future_with_locals(
                    event_loop.as_ref(),
                    result.into_bound(py),
                )
            })?;
            let _result = feature.await?;
        } else {
            // If no on_message method is defined, we call handle_message
            self.handle_message(msg, ctx).await?;
        }
        Ok(())
    }

    async fn handle_message(
        &mut self,
        msg: Self::Message,
        ctx: &mut ActorContext,
    ) -> Result<PyResponse> {
        self.ensure_actor_loaded().await?;
        let py_ctx = self.new_py_ctx(ctx);

        let handle_message_method = {
            let methods_guard = self.cached_methods.lock().await;
            Python::with_gil(|_| {
                methods_guard
                    .as_ref()
                    .and_then(|cached| cached.handle_message.clone())
            })
        };

        if let Some(method) = handle_message_method {
            tracing::debug!("Calling Python handle_message method");
            let event_loop = self.event_loop.clone();

            // Handle the message conversion to Python
            let py_msg_value = Python::with_gil(|py| msg.to_python(py))?;
            let inner_ctx = self.new_inner_ctx();
            let feature = Python::with_gil(|py| -> PyResult<_> {
                let py_module = py.import("lyricore")?;
                py_module.call_method1("set_global_inner_context", (inner_ctx,))?;

                let py_ctx_obj = Py::new(py, py_ctx)?.into_any();
                let result = method.call1(py, (py_msg_value, py_ctx_obj))?;
                pyo3_async_runtimes::into_future_with_locals(
                    event_loop.as_ref(),
                    result.into_bound(py),
                )
            })?;
            let result = feature.await?;
            let response = Python::with_gil(|py| PyResponse::from_python(result.into_bound(py)))?;
            Ok(response)
        } else {
            Err(LyricoreActorError::Actor(ActorError::RpcError(
                "No handle_message method defined in Python actor".to_string(),
            )))
        }
    }

    async fn on_stop(&mut self, ctx: &mut ActorContext) -> Result<()> {
        self.ensure_actor_loaded().await?;
        let py_ctx = self.new_py_ctx(ctx);

        let on_stop_method = {
            let methods_guard = self.cached_methods.lock().await;
            Python::with_gil(|py| {
                methods_guard
                    .as_ref()
                    .and_then(|cached| cached.on_stop.clone())
            })
        };
        if let Some(method) = on_stop_method {
            tracing::debug!("Calling Python on_stop method");
            let event_loop = self.event_loop.clone();
            let inner_ctx = self.new_inner_ctx();
            let future = Python::with_gil(|py| -> PyResult<_> {
                let py_module = py.import("lyricore")?;
                py_module.call_method1("set_global_inner_context", (inner_ctx,))?;

                let py_ctx_obj = Py::new(py, py_ctx)?.into_any();
                let result = method.call1(py, (py_ctx_obj,))?;
                pyo3_async_runtimes::into_future_with_locals(
                    event_loop.as_ref(),
                    result.into_bound(py),
                )
            })?;
            let _ = future.await?;
        }
        Ok(())
    }
}
