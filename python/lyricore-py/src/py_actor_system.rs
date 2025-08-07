use crate::object_store::{PyObjectStore, PyStoreConfig};
use crate::py_actor::PyActorWrapper;
use crate::py_actor_ref::PyActorRef;
use crate::utils::py_value::PyValue;
use crate::utils::{PyActorConstructionTask, PyActorDescriptor};
use lyricore::serialization::SerializationStrategy;
use lyricore::{ActorAddress, ActorContext, ActorPath, ActorSystem, SchedulerConfig, TokioRuntime};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyTuple};
use pyo3::{pyclass, pymethods, Bound, FromPyObject, PyAny, PyErr, PyObject, PyResult, Python};
use pyo3_async_runtimes::TaskLocals;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::{Arc, OnceLock, Weak};
use tokio::sync::{mpsc, oneshot, Mutex as TokioMutex};
use uuid::Uuid;
static GLOBAL_RUNTIME: OnceLock<TokioRuntime> = OnceLock::new();

pub(crate) fn get_or_create_global_runtime(worker_threads: Option<usize>) -> TokioRuntime {
    GLOBAL_RUNTIME
        .get_or_init(|| {
            let num_threads = worker_threads.unwrap_or_else(num_cpus::get);
            TokioRuntime::new(num_threads).expect("Failed to create Tokio runtime")
        })
        .clone()
}

#[derive(Debug)]
pub(crate) enum LangWorkerMessage {
    SubmitEvent {
        method: PyObject,
        ctx: ActorContext,
        response_tx: oneshot::Sender<lyricore::error::Result<()>>,
    },
}

pub(crate) struct PyActorSystemInner {
    runtime: TokioRuntime,
    inner: Arc<TokioMutex<ActorSystem>>,
    system_name: String,
    listen_address: String,
    tx_task: mpsc::UnboundedSender<LangWorkerMessage>,
    pub(crate) store: PyObjectStore,
    curr: Option<Weak<PyActorSystemInner>>,
}

#[pyclass]
pub struct PyActorSystem {
    rx_task: Option<mpsc::UnboundedReceiver<LangWorkerMessage>>,
    inner: Arc<PyActorSystemInner>,
}

impl PyActorSystemInner {
    // 添加新方法：从构造任务创建 Actor
    pub(crate) fn spawn_from_construction_task<'a>(
        &self,
        py: Python<'a>,
        task_dict: &Bound<PyDict>,
        path: String,
    ) -> PyResult<Bound<'a, PyAny>> {
        let inner = Arc::clone(&self.inner);
        let system_name = self.system_name.clone();
        let listen_address = self.listen_address.clone();
        let rt = self.runtime.clone();
        let new_rt = self.runtime.clone();
        let tx_task = self.tx_task.clone();
        let curr = self.curr.clone();

        let locals = Python::with_gil(|py| pyo3_async_runtimes::tokio::get_current_locals(py))?;

        // Extract the construction task from the provided dictionary
        let construction_task = self.extract_construction_task(py, task_dict)?;

        let locals_clone = Arc::new(pyo3_async_runtimes::tokio::get_current_locals(py)?);

        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            // Create a PyActorWrapper from the construction task
            let wrapper = PyActorWrapper::new_from_construction_task(
                construction_task,
                new_rt.clone(),
                locals_clone,
                Some(tx_task),
                curr,
            );

            // Path starts with '/' or 'lyricore://'
            let actor_path = if path.starts_with('/') {
                ActorPath::new(system_name, ActorAddress::from_str(&listen_address)?, path)
            } else {
                ActorPath::new(
                    system_name,
                    ActorAddress::from_str(&listen_address)?,
                    format!("/user/{}", path),
                )
            };

            // Spawn the actor at the specified path
            let rust_actor_ref = rt
                .runtime
                .spawn(async move {
                    let system = inner.lock().await;
                    system.spawn_at_path(actor_path, wrapper)
                })
                .await
                .unwrap()?;

            Ok(PyActorRef::new(rust_actor_ref, new_rt, Arc::new(locals)))
        })
    }

    fn extract_construction_task(
        &self,
        _py: Python,
        task_dict: &Bound<PyDict>,
    ) -> PyResult<PyActorConstructionTask> {
        let constructor_func = task_dict
            .get_item("constructor_func")?
            .ok_or_else(|| {
                PyErr::new::<pyo3::exceptions::PyKeyError, _>("Missing constructor_func")
            })?
            .extract::<String>()?;

        let constructor_args = task_dict
            .get_item("constructor_args")?
            .ok_or_else(|| {
                PyErr::new::<pyo3::exceptions::PyKeyError, _>("Missing constructor_args")
            })?
            .extract::<Vec<PyValue>>()?;

        let constructor_kwargs = task_dict
            .get_item("constructor_kwargs")?
            .ok_or_else(|| {
                PyErr::new::<pyo3::exceptions::PyKeyError, _>("Missing constructor_kwargs")
            })?
            .extract::<HashMap<String, PyValue>>()?;

        let function_hash = task_dict
            .get_item("function_hash")?
            .ok_or_else(|| PyErr::new::<pyo3::exceptions::PyKeyError, _>("Missing function_hash"))?
            .extract::<String>()?;

        let module_name = task_dict
            .get_item("module_name")?
            .ok_or_else(|| PyErr::new::<pyo3::exceptions::PyKeyError, _>("Missing module_name"))?
            .extract::<String>()?;

        let class_name = task_dict
            .get_item("class_name")?
            .ok_or_else(|| PyErr::new::<pyo3::exceptions::PyKeyError, _>("Missing class_name"))?
            .extract::<String>()?;

        let capture_globals = task_dict
            .get_item("capture_globals")?
            .and_then(|v| v.extract::<Option<String>>().ok())
            .flatten();

        Ok(PyActorConstructionTask {
            constructor_func,
            constructor_args,
            constructor_kwargs,
            function_hash,
            module_name,
            class_name,
            capture_globals,
        })
    }

    pub(crate) fn spawn<'a>(
        &self,
        py: Python<'a>,
        actor_class: PyObject,
        path: String,
        args: Option<&Bound<PyTuple>>,
        kwargs: Option<&Bound<PyDict>>,
    ) -> PyResult<Bound<'a, PyAny>> {
        let inner = Arc::clone(&self.inner);
        let system_name = self.system_name.clone();
        let listen_address = self.listen_address.clone();
        let rt = self.runtime.clone();
        let new_rt = self.runtime.clone();
        let tx_task = self.tx_task.clone();
        let curr = self.curr.clone();

        let locals = Python::with_gil(|py| pyo3_async_runtimes::tokio::get_current_locals(py))?;

        // Extract module name, class name, qualname, init_args, and init_kwargs
        let (module_name, class_name, qualname, init_args, init_kwargs) = {
            let module_name = actor_class
                .getattr(py, "__module__")?
                .extract::<String>(py)?;
            let class_name = actor_class.getattr(py, "__name__")?.extract::<String>(py)?;
            let qualname = actor_class
                .getattr(py, "__qualname__")?
                .extract::<String>(py)?;

            let init_args = if let Some(args) = args {
                args.iter()
                    .map(|arg| PyValue::extract_bound(&arg))
                    .collect::<PyResult<Vec<_>>>()?
            } else {
                Vec::new()
            };

            let init_kwargs = if let Some(kwargs) = kwargs {
                kwargs
                    .iter()
                    .map(|(k, v)| {
                        let key = k.extract::<String>()?;
                        let value = PyValue::extract_bound(&v)?;
                        Ok((key, value))
                    })
                    .collect::<PyResult<HashMap<_, _>>>()?
            } else {
                HashMap::new()
            };

            (module_name, class_name, qualname, init_args, init_kwargs)
        };

        let locals_clone = Arc::new(pyo3_async_runtimes::tokio::get_current_locals(py)?);

        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            // Create PyActorDescriptor
            let descriptor = PyActorDescriptor {
                module_name,
                class_name,
                qualname: qualname.clone(),
                actor_id: Uuid::new_v4().to_string(),
                checkpoint_data: None,
                serialized_class_b64: String::new(),
                execution_context_b64: None,
                is_local_class: qualname.contains(".<locals>."),
                class_hash: String::new(),
                init_args,
                init_kwargs,
                // 新字段的默认值
                constructor_func: String::new(),
                constructor_args: Vec::new(),
                constructor_kwargs: HashMap::new(),
                function_hash: String::new(),
                capture_globals: None,
            };

            // Create a PyActorWrapper
            let wrapper = PyActorWrapper::new(
                descriptor,
                new_rt.clone(),
                locals_clone,
                Some(tx_task),
                curr,
            );

            let actor_path = if path.starts_with('/') {
                ActorPath::new(system_name, ActorAddress::from_str(&listen_address)?, path)
            } else {
                ActorPath::new(
                    system_name,
                    ActorAddress::from_str(&listen_address)?,
                    format!("/user/{}", path),
                )
            };

            let rust_actor_ref = rt
                .runtime
                .spawn(async move {
                    let system = inner.lock().await;
                    system.spawn_at_path(actor_path, wrapper)
                })
                .await
                .unwrap()?;

            Ok(PyActorRef::new(rust_actor_ref, new_rt, Arc::new(locals)))
        })
    }

    pub(crate) fn actor_of<'a>(&self, py: Python<'a>, path: String) -> PyResult<Bound<'a, PyAny>> {
        let inner = Arc::clone(&self.inner);
        let rt = self.runtime.clone();
        let system_name = self.system_name.clone();
        let listen_address = self.listen_address.clone();

        let locals = Python::with_gil(|py| pyo3_async_runtimes::tokio::get_current_locals(py))?;
        tracing::debug!("Using locals: {:?}", locals);

        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let actor_path = if path.starts_with("lyricore://") {
                ActorPath::parse(&path)
                    .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))?
            } else if path.starts_with('/') {
                ActorPath::new(
                    system_name,
                    ActorAddress::from_str(&listen_address).map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string())
                    })?,
                    path,
                )
            } else {
                ActorPath::new(
                    system_name,
                    ActorAddress::from_str(&listen_address).map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string())
                    })?,
                    format!("/user/{}", path),
                )
            };

            let actor_ref = rt
                .runtime
                .spawn(async move {
                    let system = inner.lock().await;
                    system.actor_of_path(&actor_path).await
                })
                .await
                .unwrap()?;
            Ok(PyActorRef::new(actor_ref, rt, Arc::new(locals)))
        })
    }
}

#[pymethods]
impl PyActorSystem {
    #[new]
    #[pyo3(signature = (system_name, listen_address, worker_threads=None, store_config=None))]
    fn new(
        system_name: String,
        listen_address: String,
        worker_threads: Option<usize>,
        store_config: Option<PyStoreConfig>,
    ) -> PyResult<Self> {
        lyricore::init_tracing_subscriber("lyriccore", "INFO");
        let runtime = get_or_create_global_runtime(worker_threads);

        let config = SchedulerConfig {
            worker_threads: worker_threads.unwrap_or_else(num_cpus::get),
            ..Default::default()
        };

        let strategy = SerializationStrategy::fast_json();

        let (inner, tx_task, rx_task) = runtime.runtime.block_on(async {
            let (tx_task, rx_task) = mpsc::unbounded_channel();
            let actor_sys = ActorSystem::new_with_name(
                system_name.clone(),
                listen_address.clone(),
                config,
                Some(strategy),
            );
            (actor_sys, tx_task, rx_task)
        });
        let store = PyObjectStore::from(runtime.clone(), store_config)?;
        let sys = Arc::new(TokioMutex::new(inner?));
        let inner = Arc::new_cyclic(|weak| {
            PyActorSystemInner {
                runtime,
                inner: sys,
                system_name,
                listen_address,
                tx_task,
                store,
                curr: Some(weak.clone()), // Arc::new_cyclic
            }
        });
        Ok(Self {
            rx_task: Some(rx_task),
            inner,
        })
    }

    fn start<'a>(&mut self, py: Python<'a>) -> PyResult<Bound<'a, PyAny>> {
        let inner = Arc::clone(&self.inner.inner);

        let rt = self.inner.runtime.clone();
        let rx_task = self.rx_task.take().expect("rx_task should be initialized");
        let tx_task = self.inner.tx_task.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let inner_clone = Arc::clone(&inner);
            let locals =
                Python::with_gil(|py| pyo3_async_runtimes::tokio::get_current_locals(py)).unwrap();
            tracing::debug!("[start], Using locals: {:?}", locals);
            let locals_clone = Arc::new(locals);
            let _ = rt
                .runtime
                .spawn(async move {
                    let mut system = inner.lock().await;
                    system.start_server().await
                })
                .await
                .unwrap()?;
            let _ = rt.runtime.spawn(handle_lang_worker_messages(
                rx_task,
                tx_task,
                inner_clone,
                locals_clone,
            ));
            Ok(())
        })
    }

    fn shutdown<'a>(&self, py: Python<'a>) -> PyResult<Bound<'a, PyAny>> {
        let inner = Arc::clone(&self.inner.inner);
        let rt = self.inner.runtime.clone();

        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let _ = rt
                .runtime
                .spawn(async move {
                    let mut system = inner.lock().await;
                    system.shutdown().await
                })
                .await
                .unwrap()?;
            Ok(())
        })
    }
    // 新增方法
    fn spawn_from_construction_task<'a>(
        &self,
        py: Python<'a>,
        task_dict: &Bound<PyDict>,
        path: String,
    ) -> PyResult<Bound<'a, PyAny>> {
        self.inner.spawn_from_construction_task(py, task_dict, path)
    }
    fn spawn<'a>(
        &self,
        py: Python<'a>,
        actor_class: PyObject,
        path: String,
        args: Option<&Bound<PyTuple>>,
        kwargs: Option<&Bound<PyDict>>,
    ) -> PyResult<Bound<'a, PyAny>> {
        self.inner.spawn(py, actor_class, path, args, kwargs)
    }

    fn actor_of<'a>(&self, py: Python<'a>, path: String) -> PyResult<Bound<'a, PyAny>> {
        self.inner.actor_of(py, path)
    }

    fn connect_to_node<'a>(
        &self,
        py: Python<'a>,
        node_id: String,
        address: String,
    ) -> PyResult<Bound<'a, PyAny>> {
        let inner = Arc::clone(&self.inner.inner);
        let rt = self.inner.runtime.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let _ = rt
                .runtime
                .spawn(async move {
                    let system = inner.lock().await;
                    system.connect_to_node(node_id, address).await
                })
                .await
                .unwrap()?;
            Ok(())
        })
    }

    fn get_store(&self) -> PyResult<PyObjectStore> {
        Ok(self.inner.store.clone())
    }
}

async fn handle_lang_worker_messages(
    mut rx: mpsc::UnboundedReceiver<LangWorkerMessage>,
    tx: mpsc::UnboundedSender<LangWorkerMessage>,
    system: Arc<TokioMutex<ActorSystem>>,
    locals: Arc<TaskLocals>,
) {
    tracing::debug!("[handle_lang_worker_messages], Using locals: {:?}", locals);
    loop {
        tokio::select! {
            biased;

            msg = rx.recv() => {
                tracing::debug!("Received message: {:?}", msg);
                match msg {
                    Some(msg) => {
                        let locals_clone = Arc::clone(&locals);
                        if let Err(e) = handle_lang_worker_message_internal(msg, locals_clone).await {
                            tracing::error!("Error handling LangWorkerMessage: {}", e);
                        }
                    }
                    None => {
                        tracing::warn!("LangWorkerMessage channel closed, exiting handler.");
                        break;
                    }
                }
            }
        }
    }
}

async fn handle_lang_worker_message_internal(
    msg: LangWorkerMessage,
    locals: Arc<TaskLocals>,
) -> lyricore::Result<()> {
    tracing::debug!(
        "[handle_lang_worker_message_internal] Using locals: {:?}",
        locals
    );
    match msg {
        LangWorkerMessage::SubmitEvent {
            method,
            ctx,
            response_tx,
        } => {
            todo!()
        }
    }
}
