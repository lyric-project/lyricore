use crate::py_actor_ref::PyActorRef;
use crate::py_actor_system::get_or_create_global_runtime;
use crate::utils::py_value::PyValue;
use crate::utils::PyMessage;
use lyricore::{
    AllEventsClassifier, EventBus, EventBusConfig, EventBusStats, EventClassifier, PublishResult,
    TokioRuntime, TopicClassifier,
};
use pyo3::{pyclass, pymethods, FromPyObject, PyErr, PyObject, PyResult, Python};
use std::sync::{Arc, Weak};

#[pyclass]
#[derive(Clone)]
pub struct PyEventBusConfig {
    inner: EventBusConfig,
}

#[pyclass]
#[derive(Clone)]
pub struct PyPublishResult {
    inner: PublishResult,
}

#[pyclass]
#[derive(Clone)]
pub struct PyEventBusStats {
    inner: EventBusStats,
}

#[pymethods]
impl PyEventBusStats {
    #[getter]
    fn total_events_published(&self) -> u64 {
        self.inner.total_events_published
    }
    #[getter]
    fn total_subscriptions(&self) -> usize {
        self.inner.total_subscriptions
    }

    #[getter]
    fn failed_deliveries(&self) -> u64 {
        self.inner.failed_deliveries
    }

    fn __repr__(&self) -> PyResult<String> {
        Ok(format!(
            "EventBusStats(total_events_published={}, total_subscriptions={}, failed_deliveries={})",
            self.total_events_published(),
            self.total_subscriptions(),
            self.failed_deliveries(),
        ))
    }
}

#[pyclass]
#[derive(Clone)]
pub struct PyTopicClassifier {
    inner: TopicClassifier<PyMessage>,
}

#[pymethods]
impl PyTopicClassifier {
    #[new]
    #[pyo3(signature = (topics))]
    pub fn new(topics: Vec<String>) -> PyResult<Self> {
        let inner = TopicClassifier::new(topics);
        Ok(Self { inner })
    }
}

#[pyclass]
#[derive(Clone)]
pub struct PyEventBus {
    inner: Arc<EventBus>,
    runtime: TokioRuntime,
}

#[pymethods]
impl PyEventBus {
    #[new]
    #[pyo3(signature = (config=None))]
    pub fn new(config: Option<PyEventBusConfig>) -> PyResult<Self> {
        let config = config.map(|c| c.inner).unwrap_or_default();
        let runtime = get_or_create_global_runtime(None);
        let inner = Arc::new(EventBus::new(config));
        Ok(Self { inner, runtime })
    }

    #[pyo3(signature = (actor_ref, topic_classifier=None))]
    pub async fn subscribe(
        &self,
        actor_ref: PyActorRef,
        topic_classifier: Option<PyTopicClassifier>,
    ) -> PyResult<String> {
        let inner = self.inner.clone();
        let actor_ref = actor_ref.inner.clone();
        let rt = self.runtime.clone();
        rt.runtime
            .spawn(async move {
                let id = match topic_classifier {
                    Some(tc) => inner.subscribe(actor_ref, tc.inner).await.map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                            "Failed to subscribe to event bus with topic classifier: {}",
                            e
                        ))
                    })?,
                    None => inner
                        .subscribe(actor_ref, AllEventsClassifier::<PyMessage>::new())
                        .await
                        .map_err(|e| {
                            PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                                "Failed to subscribe to event bus with all events classifier: {}",
                                e
                            ))
                        })?,
                };
                Ok(id.0)
            })
            .await
            .map_err(|e| {
                PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                    "Failed to join subscribe task: {}",
                    e
                ))
            })?
    }
    pub async fn unsubscribe(&self, subscription_id: String) -> PyResult<bool> {
        let inner = self.inner.clone();
        let rt = self.runtime.clone();
        rt.runtime
            .spawn(async move {
                let id = lyricore::SubscriptionId(subscription_id);
                inner.unsubscribe(&id).await.map_err(|e| {
                    PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                        "Failed to unsubscribe from event bus: {}",
                        e
                    ))
                })
            })
            .await
            .map_err(|e| {
                PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                    "Failed to join unsubscribe task: {}",
                    e
                ))
            })?
    }

    pub async fn unsubscribe_actor(&self, actor_ref: PyActorRef) -> PyResult<usize> {
        let inner = self.inner.clone();
        let actor_ref = actor_ref.inner.clone();
        let rt = self.runtime.clone();
        rt.runtime
            .spawn(async move {
                inner.unsubscribe_actor(&actor_ref).await.map_err(|e| {
                    PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                        "Failed to unsubscribe actor from event bus: {}",
                        e
                    ))
                })
            })
            .await
            .map_err(|e| {
                PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                    "Failed to join unsubscribe_actor task: {}",
                    e
                ))
            })?
    }

    #[pyo3(signature = (message, topic=None))]
    pub async fn publish(
        &self,
        message: PyObject,
        topic: Option<String>,
    ) -> PyResult<PyPublishResult> {
        let inner = self.inner.clone();
        let rt = self.runtime.clone();
        rt.runtime
            .spawn(async move {
                let py_value = Python::with_gil(|py| PyValue::extract_bound(&message.bind(py)))?;
                let py_msg = PyMessage::new(py_value);
                let res = inner.publish_with_topic(py_msg, topic).await.map_err(|e| {
                    PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(e.to_string())
                })?;
                Ok(PyPublishResult { inner: res })
            })
            .await
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(e.to_string()))?
    }

    pub async fn get_stats(&self) -> PyResult<PyEventBusStats> {
        let inner = self.inner.clone();
        let rt = self.runtime.clone();
        rt.runtime
            .spawn(async move {
                let stats = inner.get_stats().await;
                Ok(PyEventBusStats { inner: stats })
            })
            .await
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(e.to_string()))?
    }

    pub async fn shutdown(&self) -> PyResult<()> {
        let inner = self.inner.clone();
        let rt = self.runtime.clone();
        rt.runtime
            .spawn(async move {
                inner
                    .shutdown()
                    .await
                    .map_err(|e| PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(e.to_string()))
            })
            .await
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(e.to_string()))?
    }
}
