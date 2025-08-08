use crate::py_actor_system::get_or_create_global_runtime;
use futures::TryStreamExt;
use lyricore::TokioRuntime;
use lyricore_store::{
    DataView, ExternalDataKeeper, FastCopyMode, FastMemOps, LifetimeKeeper, ObjectId, ObjectRef,
    ObjectStore, StoreConfig, StoreError,
};
use pyo3::buffer::PyBuffer;
use pyo3::exceptions::{PyKeyError, PyMemoryError, PyRuntimeError, PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict, PyTuple};
use pyo3::IntoPyObjectExt;
use std::sync::Arc;

/// Python CopyMode
#[pyclass]
#[derive(Clone, Copy, Debug)]
pub enum PyCopyMode {
    ZeroCopy,
    FastCopy,
    SafeCopy,
}

#[pymethods]
impl PyCopyMode {
    #[new]
    fn new(mode: &str) -> PyResult<Self> {
        match mode.to_lowercase().as_str() {
            "zerocopy" | "zero_copy" | "zero-copy" => Ok(PyCopyMode::ZeroCopy),
            "fastcopy" | "fast_copy" | "fast-copy" => Ok(PyCopyMode::FastCopy),
            "safecopy" | "safe_copy" | "safe-copy" => Ok(PyCopyMode::SafeCopy),
            _ => Err(PyValueError::new_err(
                "Invalid copy mode. Use 'zerocopy', 'fastcopy', or 'safecopy'",
            )),
        }
    }

    fn __str__(&self) -> &'static str {
        match self {
            PyCopyMode::ZeroCopy => "zerocopy",
            PyCopyMode::FastCopy => "fastcopy",
            PyCopyMode::SafeCopy => "safecopy",
        }
    }
}

impl From<PyCopyMode> for FastCopyMode {
    fn from(py_mode: PyCopyMode) -> Self {
        match py_mode {
            PyCopyMode::ZeroCopy => FastCopyMode::Auto, // Auto mode for zero-copy
            PyCopyMode::FastCopy => FastCopyMode::Auto,
            PyCopyMode::SafeCopy => FastCopyMode::Standard,
        }
    }
}

#[derive(Debug)]
struct BufferKeeper {
    _buffer: PyBuffer<u8>,
}

impl BufferKeeper {
    fn new(buffer: PyBuffer<u8>) -> Self {
        Self { _buffer: buffer }
    }
}

unsafe impl Send for BufferKeeper {}
unsafe impl Sync for BufferKeeper {}

impl LifetimeKeeper for BufferKeeper {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// NumPy Extractor: To extract information from NumPy arrays
struct NumpyExtractor;

impl NumpyExtractor {
    /// Extracts dtype, shape, itemsize, and whether the array is C-contiguous.
    fn extract_info(py: Python, array: &PyObject) -> PyResult<(String, Vec<usize>, usize, bool)> {
        let array_bound = array.bind(py);

        let numpy = py.import("numpy")?;
        let ndarray_type = numpy.getattr("ndarray")?;
        if !array_bound.is_instance(&ndarray_type)? {
            return Err(PyTypeError::new_err("Object is not a NumPy array"));
        }

        let dtype_obj = array_bound.getattr("dtype")?;
        let dtype_str: String = dtype_obj.getattr("name")?.extract()?;
        let shape: Vec<usize> = array_bound.getattr("shape")?.extract()?;
        let itemsize: usize = array_bound.getattr("itemsize")?.extract()?;
        let is_contiguous: bool = array_bound
            .getattr("flags")?
            .getattr("c_contiguous")?
            .extract()?;

        Ok((dtype_str, shape, itemsize, is_contiguous))
    }

    pub fn extract_zerocopy(
        py: Python,
        array: &PyObject,
    ) -> PyResult<(*const pyo3::buffer::ReadOnlyCell<u8>, usize, BufferKeeper)> {
        let array_bound = array.bind(py);
        let numpy = py.import("numpy")?;

        // Make sure the array is contiguous(C-contiguous)
        let final_array = if array_bound
            .getattr("flags")?
            .getattr("c_contiguous")?
            .extract::<bool>()?
        {
            // This only increases reference count, no data copy
            array.clone()
        } else {
            let ascontiguousarray = numpy.getattr("ascontiguousarray")?;
            // <-- This actually copies data
            ascontiguousarray.call1((array_bound,))?.unbind()
        };

        let final_bound = final_array.bind(py);

        // Create a uint8 view (zero-copy, just reinterpret memory)
        let dtype_u8 = numpy.getattr("uint8")?;
        let byte_view = final_bound.call_method1("view", (dtype_u8,))?;

        // Reshape to 1D
        let nbytes: usize = final_bound.getattr("nbytes")?.extract()?;
        let flattened = byte_view.call_method1("reshape", (nbytes,))?;

        // Get the buffer from the flattened array
        let buffer: PyBuffer<u8> = PyBuffer::get(&flattened)?;

        if !buffer.is_c_contiguous() {
            return Err(PyValueError::new_err("Buffer must be C-contiguous"));
        }

        let slice = buffer
            .as_slice(py)
            .ok_or_else(|| PyRuntimeError::new_err("Failed to get buffer slice"))?;

        let ptr = slice.as_ptr();
        let len = slice.len();
        let keeper = BufferKeeper::new(buffer);

        Ok((ptr, len, keeper))
    }

    /// Extracts data from a NumPy array using fast copy.
    unsafe fn extract_fastcopy(py: Python, array: &PyObject) -> PyResult<Vec<u8>> {
        let array_bound = array.bind(py);

        if array_bound
            .getattr("flags")?
            .getattr("c_contiguous")?
            .extract::<bool>()?
        {
            let data_ptr = array_bound
                .getattr("ctypes")?
                .getattr("data")?
                .extract::<usize>()? as *const u8;
            let itemsize: usize = array_bound.getattr("itemsize")?.extract()?;
            let shape: Vec<usize> = array_bound.getattr("shape")?.extract()?;
            let total_size = shape.iter().product::<usize>() * itemsize;

            Ok(FastMemOps::fast_copy(
                data_ptr,
                total_size,
                FastCopyMode::Auto,
            ))
        } else {
            Self::extract_safecopy(py, array)
        }
    }

    /// Extracts data from a NumPy array using safe copy.
    fn extract_safecopy(py: Python, array: &PyObject) -> PyResult<Vec<u8>> {
        let array_bound = array.bind(py);
        let tobytes = array_bound.call_method0("tobytes")?;
        let bytes = tobytes.downcast::<PyBytes>()?;
        bytes.extract::<Vec<u8>>()
    }
}

#[pyclass]
#[derive(Clone)]
pub struct PyStoreConfig {
    inner: StoreConfig,
}

#[pymethods]
impl PyStoreConfig {
    #[new]
    #[pyo3(signature = (max_memory=1024*1024*1024, max_object_size=64*1024*1024, memory_pressure_threshold=0.8, track_access_time=true))]
    fn new(
        max_memory: usize,
        max_object_size: usize,
        memory_pressure_threshold: f32,
        track_access_time: bool,
    ) -> PyResult<Self> {
        if memory_pressure_threshold < 0.0 || memory_pressure_threshold > 1.0 {
            return Err(PyValueError::new_err(
                "memory_pressure_threshold must be between 0.0 and 1.0",
            ));
        }

        Ok(Self {
            inner: StoreConfig {
                max_memory,
                max_object_size,
                memory_pressure_threshold,
                track_access_time,
            },
        })
    }

    #[getter]
    fn max_memory(&self) -> usize {
        self.inner.max_memory
    }

    #[getter]
    fn max_object_size(&self) -> usize {
        self.inner.max_object_size
    }

    #[getter]
    fn memory_pressure_threshold(&self) -> f32 {
        self.inner.memory_pressure_threshold
    }

    #[getter]
    fn track_access_time(&self) -> bool {
        self.inner.track_access_time
    }

    fn __repr__(&self) -> String {
        format!(
            "PyStoreConfig(max_memory={}, max_object_size={}, memory_pressure_threshold={:.2}, track_access_time={})",
            self.inner.max_memory,
            self.inner.max_object_size,
            self.inner.memory_pressure_threshold,
            self.inner.track_access_time
        )
    }
}

#[pyclass]
pub struct PyObjectRef {
    inner: ObjectRef,
}

#[pymethods]
impl PyObjectRef {
    #[getter]
    fn id(&self) -> String {
        self.inner.id().to_string()
    }

    #[getter]
    fn size(&self) -> usize {
        self.inner.size()
    }

    #[getter]
    fn data_type(&self) -> String {
        format!("{:?}", self.inner.metadata().data_type)
    }

    // Get the raw byte data of the object
    fn as_bytes(&self, py: Python) -> PyResult<Py<PyBytes>> {
        let data = self.inner.data();
        Ok(PyBytes::new(py, data).into())
    }

    // Gets a memoryview of the object data
    fn as_memoryview(&self, py: Python) -> PyResult<PyObject> {
        let data = self.inner.data();
        let py_bytes = PyBytes::new(py, data);
        let memoryview = py.import("builtins")?.getattr("memoryview")?;
        let res = memoryview.call1((py_bytes,))?;
        res.into_py_any(py)
    }

    // If the object is compatible with NumPy, return a NumPy array (zero-copy)
    fn as_numpy(&self, py: Python) -> PyResult<Option<PyObject>> {
        let view = DataView::new(self.inner.clone());

        if let Some(numpy_view) = view.as_numpy_compatible() {
            let numpy = py.import("numpy")?;
            let frombuffer = numpy.getattr("frombuffer")?;

            let data = self.inner.data();
            let py_bytes = PyBytes::new(py, data);

            // Create a NumPy array from the byte data
            let array = frombuffer.call1((py_bytes, numpy_view.dtype.as_str()))?;

            // If the numpy_view has a shape, reshape the array
            if !numpy_view.shape.is_empty() {
                let shape_tuple = PyTuple::new(py, &numpy_view.shape)?;
                let reshaped = array.call_method1("reshape", (shape_tuple,))?;
                let reshaped: PyObject = reshaped.into_py_any(py)?;
                Ok(Some(reshaped))
            } else {
                Ok(Some(array.into_py_any(py)?))
            }
        } else {
            Ok(None)
        }
    }

    // Gets metadata of the object
    fn metadata(&self, py: Python) -> PyResult<PyObject> {
        let dict = PyDict::new(py);
        let meta = self.inner.metadata();

        dict.set_item("data_type", format!("{:?}", meta.data_type))?;
        dict.set_item("alignment", meta.alignment)?;
        dict.set_item("track_access_time", meta.track_access_time)?;

        if let Some(ref shape) = meta.shape {
            dict.set_item("shape", shape)?;
        } else {
            dict.set_item("shape", py.None())?;
        }

        if let Some(ref strides) = meta.strides {
            dict.set_item("strides", strides)?;
        } else {
            dict.set_item("strides", py.None())?;
        }

        Ok(dict.into())
    }

    fn __repr__(&self) -> String {
        format!(
            "PyObjectRef(id={}, size={}, data_type={})",
            self.inner.id(),
            self.inner.size(),
            format!("{:?}", self.inner.metadata().data_type)
        )
    }
}

#[pyclass]
pub struct PyObjectView {
    inner: DataView,
}

#[pymethods]
impl PyObjectView {
    #[getter]
    fn id(&self) -> String {
        self.inner.obj_ref.id().to_string()
    }

    #[getter]
    fn size(&self) -> usize {
        self.inner.obj_ref.size()
    }

    #[getter]
    fn data_type(&self) -> String {
        format!("{:?}", self.inner.obj_ref.metadata().data_type)
    }

    // Gets the raw byte data of the object
    fn as_bytes(&self, py: Python) -> PyResult<Py<PyBytes>> {
        let data = self.inner.as_bytes();
        Ok(PyBytes::new(py, data).into())
    }

    // Gets a NumPy array view of the object data if compatible
    fn as_numpy(&self, py: Python) -> PyResult<Option<PyObject>> {
        if let Some(numpy_view) = self.inner.as_numpy_compatible() {
            let numpy = py.import("numpy")?;
            let frombuffer = numpy.getattr("frombuffer")?;

            let py_bytes = PyBytes::new(py, numpy_view.data);
            let array = frombuffer.call1((py_bytes, numpy_view.dtype.as_str()))?;

            if !numpy_view.shape.is_empty() {
                let shape_tuple = PyTuple::new(py, &numpy_view.shape)?;
                let reshaped = array.call_method1("reshape", (shape_tuple,))?;
                let reshaped: PyObject = reshaped.into_py_any(py)?;
                Ok(Some(reshaped))
            } else {
                Ok(Some(array.into_py_any(py)?))
            }
        } else {
            Ok(None)
        }
    }

    // Transforms the object view into a Python object using pickle
    fn to_object(&self, py: Python) -> PyResult<PyObject> {
        let pickle = py.import("lyricore.pickle")?;
        let loads = pickle.getattr("loads")?;
        let data = self.inner.as_bytes();
        let py_bytes = PyBytes::new(py, data);
        loads.call1((py_bytes,))?.into_py_any(py)
    }

    fn __repr__(&self) -> String {
        format!(
            "PyObjectView(id={}, size={}, data_type={})",
            self.inner.obj_ref.id(),
            self.inner.obj_ref.size(),
            format!("{:?}", self.inner.obj_ref.metadata().data_type)
        )
    }
}

#[derive(Clone)]
pub(crate) struct ObjectStoreInner {
    store: Arc<ObjectStore>,
    runtime: TokioRuntime,
}

impl ObjectStoreInner {
    pub fn new(runtime: TokioRuntime, config: Option<StoreConfig>) -> Self {
        let store_config = config.unwrap_or_default();
        let store = ObjectStore::new(store_config);

        Self {
            store: Arc::new(store),
            runtime,
        }
    }
}

#[pyclass]
#[derive(Clone)]
pub struct PyObjectStore {
    inner: Arc<ObjectStoreInner>,
}

impl PyObjectStore {
    pub fn from_internal(inner: ObjectStoreInner) -> Self {
        Self {
            inner: Arc::new(inner),
        }
    }

    //
    pub fn from(runtime: TokioRuntime, config: Option<PyStoreConfig>) -> PyResult<Self> {
        let store_config = config.map(|c| c.inner);
        let internal = ObjectStoreInner::new(runtime, store_config);
        Ok(Self::from_internal(internal))
    }
}

#[pymethods]
impl PyObjectStore {
    #[new]
    #[pyo3(signature = (config=None, worker_threads=None))]
    fn new(config: Option<PyStoreConfig>, worker_threads: Option<usize>) -> PyResult<Self> {
        let runtime = get_or_create_global_runtime(worker_threads);
        let config = config.map(|c| c.inner);
        let inner = ObjectStoreInner::new(runtime, config);
        Ok(Self::from_internal(inner))
    }

    // Store the object as a serialized byte array
    fn put<'a>(&self, py: Python<'a>, obj: PyObject) -> PyResult<Bound<'a, PyAny>> {
        let store = Arc::clone(&self.inner.store);
        let rt = self.inner.runtime.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let data = rt
                .runtime
                .spawn(async move {
                    Python::with_gil(|py| -> PyResult<Vec<u8>> {
                        let pickle = py.import("lyricore.pickle")?;
                        let dumps = pickle.getattr("dumps")?;
                        let serialized = dumps.call1((obj,))?;
                        let bytes = serialized
                            .downcast::<PyBytes>()
                            .map_err(|_| PyTypeError::new_err("Expected a bytes object"))?;
                        let rust_vec: Vec<u8> = bytes.extract()?;
                        Ok(rust_vec)
                    })
                })
                .await
                .map_err(|e| PyRuntimeError::new_err(format!("Task join error: {}", e)))??;
            let object_id = store.put(data).await.map_err(map_store_error)?;
            Ok(object_id.to_string())
        })
    }

    /// Get an object reference by its ID
    /// The PyObjectRef will hold a reference to the object in the store
    /// So if this reference not dropped, the object will not be deleted
    fn get<'a>(&self, py: Python<'a>, object_id: String) -> PyResult<Bound<'a, PyAny>> {
        let store = Arc::clone(&self.inner.store);
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let id = parse_object_id(&object_id)?;
            let obj_ref = store.get(id).await.map_err(map_store_error)?;
            Python::with_gil(|py| Py::new(py, PyObjectRef { inner: obj_ref }))
        })
    }

    // Gets an object by ID and deserializes it into a Python object
    fn get_object<'a>(&self, py: Python<'a>, object_id: String) -> PyResult<Bound<'a, PyAny>> {
        let store = Arc::clone(&self.inner.store);
        let rt = self.inner.runtime.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let id = parse_object_id(&object_id)?;
            let obj_ref = store.get(id).await.map_err(map_store_error)?;

            rt.runtime
                .spawn(async move {
                    Python::with_gil(|py| -> PyResult<PyObject> {
                        let data = obj_ref.data();
                        let pickle = py.import("lyricore.pickle")?;
                        let loads = pickle.getattr("loads")?;
                        let py_bytes = PyBytes::new(py, data);
                        let res = loads.call1((py_bytes,))?;
                        res.into_py_any(py)
                    })
                })
                .await
                .map_err(|e| PyRuntimeError::new_err(format!("Task join error: {}", e)))?
        })
    }

    // Store the raw bytes directly in the object store
    fn put_bytes<'a>(
        &self,
        py: Python<'a>,
        data: &Bound<'_, PyBytes>,
    ) -> PyResult<Bound<'a, PyAny>> {
        let store = Arc::clone(&self.inner.store);
        let bytes_data = data.extract()?;

        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let object_id = store.put(bytes_data).await.map_err(map_store_error)?;
            Ok(object_id.to_string())
        })
    }

    // Gets the raw bytes of an object by its ID
    fn get_bytes<'a>(&self, py: Python<'a>, object_id: String) -> PyResult<Bound<'a, PyAny>> {
        let store = Arc::clone(&self.inner.store);

        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let id = parse_object_id(&object_id)?;
            let obj_ref = store.get(id).await.map_err(map_store_error)?;
            Python::with_gil(|py| {
                let data = obj_ref.data();
                let bytes = PyBytes::new(py, data);
                bytes.into_py_any(py)
            })
        })
    }

    /// High-performance storage for NumPy arrays.
    #[pyo3(signature = (array, copy_mode="fast_copy"))]
    fn put_numpy<'a>(
        &self,
        py: Python<'a>,
        array: PyObject,
        copy_mode: &str,
    ) -> PyResult<Bound<'a, PyAny>> {
        let store = Arc::clone(&self.inner.store);
        let mode = PyCopyMode::new(copy_mode)?;
        let rt = self.inner.runtime.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let object_id = match mode {
                PyCopyMode::ZeroCopy => {
                    let (data_ptr, total_size, keeper_array, dtype_str, shape) = rt
                        .runtime
                        .spawn(async move {
                            Python::with_gil(|py| -> PyResult<_> {
                                let (dtype_str, shape, _, _) =
                                    NumpyExtractor::extract_info(py, &array)?;
                                unsafe {
                                    let (data_ptr, total_size, keeper_array) =
                                        NumpyExtractor::extract_zerocopy(py, &array)?;
                                    Ok((
                                        data_ptr as usize,
                                        total_size,
                                        keeper_array,
                                        dtype_str,
                                        shape,
                                    ))
                                }
                            })
                        })
                        .await
                        .unwrap()?;

                    unsafe {
                        let id = store
                            .put_numpy_external_ref(
                                data_ptr,
                                total_size,
                                dtype_str,
                                shape,
                                'C',
                                ExternalDataKeeper::new(keeper_array),
                            )
                            .await
                            .map_err(map_store_error)?;
                        Ok(id)
                    }
                }

                PyCopyMode::FastCopy => {
                    let (data, dtype_str, shape) = rt
                        .runtime
                        .spawn(async move {
                            Python::with_gil(|py| -> PyResult<_> {
                                let (dtype_str, shape, _, _) =
                                    NumpyExtractor::extract_info(py, &array)?;

                                unsafe {
                                    let data = NumpyExtractor::extract_fastcopy(py, &array)?;
                                    Ok((data, dtype_str, shape))
                                }
                            })
                        })
                        .await
                        .unwrap()?;
                    store
                        .put_numpy(data, dtype_str, shape, 'C')
                        .await
                        .map_err(map_store_error)
                }

                PyCopyMode::SafeCopy => {
                    let (data, dtype_str, shape) = rt
                        .runtime
                        .spawn(async move {
                            Python::with_gil(|py| -> PyResult<_> {
                                let (dtype_str, shape, _, _) =
                                    NumpyExtractor::extract_info(py, &array)?;
                                let data = NumpyExtractor::extract_safecopy(py, &array)?;
                                Ok((data, dtype_str, shape))
                            })
                        })
                        .await
                        .unwrap()?;
                    store
                        .put_numpy(data, dtype_str, shape, 'C')
                        .await
                        .map_err(map_store_error)
                }
            };

            object_id.map(|r| r.to_string())
        })
    }

    /// High-performance storage for multiple NumPy arrays.
    #[pyo3(signature = (arrays, copy_mode="fastcopy"))]
    fn put_numpy_batch_optimized<'a>(
        &self,
        py: Python<'a>,
        arrays: Vec<PyObject>,
        copy_mode: &str,
    ) -> PyResult<Bound<'a, PyAny>> {
        let store = Arc::clone(&self.inner.store);
        let mode = PyCopyMode::new(copy_mode)?;
        let rt = self.inner.runtime.clone();

        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let object_ids = match mode {
                PyCopyMode::ZeroCopy => {
                    let (data_list, keeper_arrays) = rt
                        .runtime
                        .spawn(async move {
                            Python::with_gil(|py| -> PyResult<_> {
                                let mut data_list = Vec::with_capacity(arrays.len());
                                let mut keeper_arrays = Vec::with_capacity(arrays.len());

                                // 提取所有数组数据
                                for array in arrays {
                                    let (dtype_str, shape, _, _) =
                                        NumpyExtractor::extract_info(py, &array)?;
                                    unsafe {
                                        let (data_ptr, total_size, keeper_array) =
                                            NumpyExtractor::extract_zerocopy(py, &array)?;
                                        let data_ptr = data_ptr as usize;
                                        data_list.push((data_ptr, total_size, dtype_str, shape));
                                        keeper_arrays.push(keeper_array);
                                    }
                                }
                                Ok((data_list, keeper_arrays))
                            })
                        })
                        .await
                        .unwrap()?;

                    let mut ids = Vec::with_capacity(data_list.len());
                    for ((data_ptr, total_size, dtype_str, shape), keeper_array) in
                        data_list.into_iter().zip(keeper_arrays.into_iter())
                    {
                        unsafe {
                            let id = store
                                .put_numpy_external_ref(
                                    data_ptr,
                                    total_size,
                                    dtype_str,
                                    shape,
                                    'C',
                                    ExternalDataKeeper::new(keeper_array),
                                )
                                .await
                                .map_err(map_store_error)?;
                            ids.push(id);
                        }
                    }
                    Ok(ids)
                }

                PyCopyMode::FastCopy | PyCopyMode::SafeCopy => {
                    let res = rt
                        .runtime
                        .spawn(async move {
                            Python::with_gil(|py| -> PyResult<Vec<_>> {
                                arrays
                                    .into_iter()
                                    .map(|array| match mode {
                                        PyCopyMode::FastCopy => unsafe {
                                            let (dtype_str, shape, _, _) =
                                                NumpyExtractor::extract_info(py, &array)?;
                                            unsafe {
                                                let data =
                                                    NumpyExtractor::extract_fastcopy(py, &array)?;
                                                Ok((data, dtype_str, shape))
                                            }
                                        },
                                        PyCopyMode::SafeCopy => {
                                            let (dtype_str, shape, _, _) =
                                                NumpyExtractor::extract_info(py, &array)?;

                                            unsafe {
                                                let data =
                                                    NumpyExtractor::extract_safecopy(py, &array)?;
                                                Ok((data, dtype_str, shape))
                                            }
                                            // NumpyExtractor::extract_safecopy(py, &array)
                                        }
                                        _ => unreachable!(),
                                    })
                                    .collect()
                            })
                        })
                        .await
                        .map_err(|e| {
                            PyRuntimeError::new_err(format!("Task join error: {}", e))
                        })??;
                    res.into_iter()
                        .map(|(data, dtype_str, shape)| {
                            store.put_numpy(data, dtype_str, shape, 'C')
                        })
                        .collect::<futures::stream::FuturesUnordered<_>>()
                        .try_collect::<Vec<_>>()
                        .await
                        .map_err(map_store_error)
                }
            };

            let ids: Vec<String> = object_ids?.into_iter().map(|id| id.to_string()).collect();
            Ok(ids)
        })
    }
    /// Adaptive storage for NumPy arrays based on size threshold.
    #[pyo3(signature = (array, size_threshold=1048576, force_copy=None))]
    fn put_numpy_adaptive<'a>(
        &self,
        py: Python<'a>,
        array: PyObject,
        size_threshold: usize,
        force_copy: Option<bool>,
    ) -> PyResult<Bound<'a, PyAny>> {
        let store = Arc::clone(&self.inner.store);
        let rt = self.inner.runtime.clone();
        let clone_array = Python::with_gil(|py| array.clone_ref(py));
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let (selected_mode, estimated_size) = rt
                .runtime
                .spawn(async move {
                    Python::with_gil(|py| -> PyResult<(PyCopyMode, usize)> {
                        let (_, shape, itemsize, is_contiguous) =
                            NumpyExtractor::extract_info(py, &clone_array)?;
                        let estimated_size = shape.iter().product::<usize>() * itemsize;

                        let mode = if let Some(force) = force_copy {
                            if force {
                                if is_contiguous && estimated_size > size_threshold {
                                    PyCopyMode::FastCopy
                                } else {
                                    PyCopyMode::SafeCopy
                                }
                            } else {
                                PyCopyMode::ZeroCopy
                            }
                        } else {
                            if estimated_size > size_threshold && is_contiguous {
                                PyCopyMode::ZeroCopy
                            } else if is_contiguous {
                                PyCopyMode::FastCopy
                            } else {
                                PyCopyMode::SafeCopy
                            }
                        };

                        Ok((mode, estimated_size))
                    })
                })
                .await
                .map_err(|e| PyRuntimeError::new_err(format!("Task join error: {}", e)))??;

            tracing::debug!(
                "Selected copy mode: {:?}, Estimated size: {}",
                selected_mode,
                estimated_size
            );
            let object_id = match selected_mode {
                PyCopyMode::ZeroCopy => {
                    let (data_ptr, total_size, keeper_array, dtype_str, shape) = rt
                        .runtime
                        .spawn(async move {
                            Python::with_gil(|py| -> PyResult<_> {
                                let (dtype_str, shape, _, _) =
                                    NumpyExtractor::extract_info(py, &array)?;
                                let (data_ptr, total_size, keeper_array) =
                                    NumpyExtractor::extract_zerocopy(py, &array)?;
                                Ok((
                                    data_ptr as usize,
                                    total_size,
                                    keeper_array,
                                    dtype_str,
                                    shape,
                                ))
                            })
                        })
                        .await
                        .map_err(|e| {
                            PyRuntimeError::new_err(format!("Task join error: {}", e))
                        })??;
                    unsafe {
                        // let data_ptr = data_ptr as *const u8;
                        let id = store
                            .put_numpy_external_ref(
                                data_ptr,
                                total_size,
                                dtype_str,
                                shape,
                                'C',
                                ExternalDataKeeper::new(keeper_array),
                            )
                            .await
                            .map_err(map_store_error)?;
                        Ok(id)
                    }
                }
                _ => {
                    let (data, dtype_str, shape) = rt
                        .runtime
                        .spawn(async move {
                            Python::with_gil(|py| -> PyResult<_> {
                                let (dtype_str, shape, _, _) =
                                    NumpyExtractor::extract_info(py, &array)?;
                                let data = match selected_mode {
                                    PyCopyMode::FastCopy => unsafe {
                                        NumpyExtractor::extract_fastcopy(py, &array)?
                                    },
                                    PyCopyMode::SafeCopy => {
                                        NumpyExtractor::extract_safecopy(py, &array)?
                                    }
                                    _ => unreachable!(),
                                };
                                Ok((data, dtype_str, shape))
                            })
                        })
                        .await
                        .map_err(|e| {
                            PyRuntimeError::new_err(format!("Task join error: {}", e))
                        })??;
                    store
                        .put_numpy(data, dtype_str, shape, 'C')
                        .await
                        .map_err(map_store_error)
                }
            };

            let object_id = object_id?;
            Python::with_gil(|py| -> PyResult<PyObject> {
                let result_dict = PyDict::new(py);
                result_dict.set_item("object_id", object_id.to_string())?;
                result_dict.set_item("copy_mode", selected_mode.__str__())?;
                result_dict.set_item("estimated_size", estimated_size)?;
                Ok(result_dict.into())
            })
        })
    }

    /// Gets a NumPy array by its ID(zero-copy view).
    fn get_numpy<'a>(&self, py: Python<'a>, object_id: String) -> PyResult<Bound<'a, PyAny>> {
        let store = Arc::clone(&self.inner.store);

        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let id = parse_object_id(&object_id)?;
            let obj_ref = store.get(id).await.map_err(map_store_error)?;

            Python::with_gil(|py| -> PyResult<PyObject> {
                let py_obj_ref = PyObjectRef { inner: obj_ref };

                if let Some(numpy_array) = py_obj_ref.as_numpy(py)? {
                    Ok(numpy_array)
                } else {
                    Err(PyTypeError::new_err("Object is not NumPy compatible"))
                }
            })
        })
    }

    // Asynchronously stores multiple objects in the object store.
    fn put_batch<'a>(&self, py: Python<'a>, objects: Vec<PyObject>) -> PyResult<Bound<'a, PyAny>> {
        let store = Arc::clone(&self.inner.store);

        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let mut ids = Vec::with_capacity(objects.len());

            // Parallelize the serialization and storage of objects
            let futures = objects.into_iter().map(|obj| {
                let store = Arc::clone(&store);
                async move {
                    let data = Python::with_gil(|py| -> PyResult<Vec<u8>> {
                        let pickle = py.import("lyricore.pickle")?;
                        let dumps = pickle.getattr("dumps")?;
                        let serialized = dumps.call1((obj,))?;
                        let bytes = serialized
                            .downcast::<PyBytes>()
                            .map_err(|_| PyTypeError::new_err("Expected a bytes object"))?;
                        let rust_vec: Vec<u8> = bytes.extract()?;
                        Ok(rust_vec)
                    })?;

                    store.put(data).await.map_err(map_store_error)
                }
            });

            let results = futures::future::try_join_all(futures).await?;
            for id in results {
                ids.push(id.to_string());
            }

            Ok(ids)
        })
    }

    // Gets a batch of object references by their IDs
    fn get_batch<'a>(&self, py: Python<'a>, object_ids: Vec<String>) -> PyResult<Bound<'a, PyAny>> {
        let store = Arc::clone(&self.inner.store);

        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let futures = object_ids.into_iter().map(|id_str| {
                let store = Arc::clone(&store);
                async move {
                    let id = parse_object_id(&id_str)?;
                    store.get(id).await.map_err(map_store_error)
                }
            });

            let results = futures::future::try_join_all(futures).await?;

            Python::with_gil(|py| {
                let py_refs: Result<Vec<_>, _> = results
                    .into_iter()
                    .map(|obj_ref| Py::new(py, PyObjectRef { inner: obj_ref }))
                    .collect();
                py_refs
            })
        })
    }

    // Gets a batch of objects by their IDs and deserializes them into Python objects
    fn get_objects<'a>(
        &self,
        py: Python<'a>,
        object_ids: Vec<String>,
    ) -> PyResult<Bound<'a, PyAny>> {
        let store = Arc::clone(&self.inner.store);

        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let futures = object_ids.into_iter().map(|id_str| {
                let store = Arc::clone(&store);
                async move {
                    let id = parse_object_id(&id_str)?;
                    store.get(id).await.map_err(map_store_error)
                }
            });

            let results = futures::future::try_join_all(futures).await?;

            Python::with_gil(|py| -> PyResult<Vec<PyObject>> {
                let mut objects = Vec::new();
                for obj_ref in results {
                    let data = obj_ref.data();
                    let pickle = py.import("lyricore.pickle")?;
                    let loads = pickle.getattr("loads")?;
                    let py_bytes = PyBytes::new(py, data);
                    let obj = loads.call1((py_bytes,))?;
                    objects.push(obj.into_py_any(py)?);
                }
                Ok(objects)
            })
        })
    }

    /// Deletes an object by its ID
    fn delete<'a>(&self, py: Python<'a>, object_id: String) -> PyResult<Bound<'a, PyAny>> {
        let store = Arc::clone(&self.inner.store);

        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let id = parse_object_id(&object_id)?;
            match store.delete(id).await {
                Ok(()) => Ok(true),
                Err(StoreError::ObjectNotFound(_)) => Ok(false),
                Err(e) => Err(map_store_error(e)),
            }
        })
    }

    // Checks if an object exists by its ID
    fn contains<'a>(&self, py: Python<'a>, object_id: String) -> PyResult<Bound<'a, PyAny>> {
        let store = Arc::clone(&self.inner.store);

        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let id = parse_object_id(&object_id)?;
            Ok(store.contains(id).await)
        })
    }

    // Gets a view of an object by its ID
    fn get_view<'a>(&self, py: Python<'a>, object_id: String) -> PyResult<Bound<'a, PyAny>> {
        let store = Arc::clone(&self.inner.store);

        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let id = parse_object_id(&object_id)?;
            let view = store.get_view(id).await.map_err(map_store_error)?;

            Python::with_gil(|py| Py::new(py, PyObjectView { inner: view }))
        })
    }

    // Gets statistics about the object store
    fn stats(&self, py: Python) -> PyResult<PyObject> {
        let stats = self.inner.store.stats();
        let dict = PyDict::new(py);

        dict.set_item("total_objects", stats.total_objects)?;
        dict.set_item("total_memory", stats.total_memory)?;
        dict.set_item("memory_usage_mb", stats.memory_usage_mb())?;
        dict.set_item("hit_rate", stats.hit_rate())?;
        dict.set_item("cache_hits", stats.cache_hits)?;
        dict.set_item("cache_misses", stats.cache_misses)?;

        Ok(dict.into())
    }

    // Cleanup the object store by store API(LRU)
    fn cleanup<'a>(&self, py: Python<'a>) -> PyResult<Bound<'a, PyAny>> {
        let store = Arc::clone(&self.inner.store);

        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            store.cleanup().await.map_err(map_store_error)
        })
    }

    // Cleanup the object store by store API(LRU)
    fn clear<'a>(&self, py: Python<'a>) -> PyResult<Bound<'a, PyAny>> {
        let store = Arc::clone(&self.inner.store);

        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            store.clear().await;
            Ok(())
        })
    }

    // Show the statistics of the object store
    fn print_stats(&self) {
        self.inner.store.print_stats();
    }

    /// Stores an object with adaptive strategy based on size threshold.
    #[pyo3(signature = (obj, size_threshold=1048576))] // Default 1MB
    fn put_smart<'a>(
        &self,
        py: Python<'a>,
        obj: PyObject,
        size_threshold: usize,
    ) -> PyResult<Bound<'a, PyAny>> {
        let store = Arc::clone(&self.inner.store);

        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            // Get the estimated size of the Python object
            let (data, estimated_size) = Python::with_gil(|py| -> PyResult<(Vec<u8>, usize)> {
                let estimated_size = estimate_python_object_size(py, &obj)?;

                let pickle = py.import("lyricore.pickle")?;
                let dumps = pickle.getattr("dumps")?;
                let serialized = dumps.call1((obj,))?;
                let bytes = serialized
                    .downcast::<PyBytes>()
                    .map_err(|_| PyTypeError::new_err("Expected a bytes object"))?;
                let data: Vec<u8> = bytes.extract()?;

                Ok((data, estimated_size))
            })?;

            // Select the storage strategy based on size
            let object_id = if estimated_size > size_threshold {
                // Big objects: apply compression or other optimizations
                store.put(data).await.map_err(map_store_error)?
            } else {
                // Small objects: store directly
                store.put(data).await.map_err(map_store_error)?
            };

            Ok(object_id.to_string())
        })
    }

    // Memory-mapped file support (if mmap feature is enabled)
    #[cfg(feature = "mmap")]
    fn put_mmap<'a>(&self, py: Python<'a>, file_path: String) -> PyResult<Bound<'a, PyAny>> {
        let store = Arc::clone(&self.inner.store);

        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            use std::path::Path;
            let path = Path::new(&file_path);
            let object_id = store.put_mmap(path).await.map_err(map_store_error)?;
            Ok(object_id.to_string())
        })
    }

    fn __repr__(&self) -> String {
        let stats = self.inner.store.stats();
        format!(
            "PyObjectStore(objects={}, memory={:.2}MB, hit_rate={:.2}%)",
            stats.total_objects,
            stats.memory_usage_mb(),
            stats.hit_rate() * 100.0
        )
    }

    /// Get storage information for an object by its ID
    fn get_storage_info<'a>(
        &self,
        py: Python<'a>,
        object_id: String,
    ) -> PyResult<Bound<'a, PyAny>> {
        let store = Arc::clone(&self.inner.store);
        let rt = self.inner.runtime.clone();

        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let id = parse_object_id(&object_id)?;
            let info = store.get_storage_info(id).await.map_err(map_store_error)?;
            rt.runtime
                .spawn(async move {
                    Python::with_gil(|py| -> PyResult<PyObject> {
                        let info_dict = PyDict::new(py);
                        info_dict.set_item("id", info.id.to_string())?;
                        info_dict.set_item("size", info.size)?;
                        info_dict.set_item("storage_type", info.storage_type)?;
                        info_dict.set_item("is_zero_copy", info.is_zero_copy)?;
                        info_dict.set_item("data_type", format!("{:?}", info.data_type))?;
                        info_dict.set_item("alignment", info.alignment)?;
                        if let Some(ref shape) = info.shape {
                            info_dict.set_item("shape", shape.clone())?;
                        }
                        Ok(info_dict.into())
                    })
                })
                .await
                .map_err(|e| PyRuntimeError::new_err(format!("Task join error: {}", e)))?
        })
    }
    /// Analyze memory usage of the object store
    fn analyze_memory_usage(&self, py: Python) -> PyResult<PyObject> {
        let analysis = self.inner.store.analyze_memory_usage();
        let dict = PyDict::new(py);

        dict.set_item("total_objects", analysis.total_objects)?;
        dict.set_item("total_memory_bytes", analysis.total_memory_bytes)?;
        dict.set_item("memory_usage_mb", analysis.memory_usage_mb)?;
        dict.set_item(
            "average_object_size_bytes",
            analysis.average_object_size_bytes,
        )?;
        dict.set_item("hit_rate", analysis.hit_rate)?;
        dict.set_item(
            "cache_efficiency_percent",
            analysis.cache_efficiency_percent,
        )?;
        dict.set_item("max_memory_bytes", analysis.max_memory_bytes)?;
        dict.set_item("is_under_pressure", analysis.is_under_pressure)?;

        Ok(dict.into())
    }
    /// Zero-copy statistics
    fn get_zero_copy_stats<'a>(&self, py: Python<'a>) -> PyResult<Bound<'a, PyAny>> {
        let store = Arc::clone(&self.inner.store);
        let rt = self.inner.runtime.clone();

        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let stats = store.get_zero_copy_stats().await;
            rt.runtime
                .spawn(async move {
                    Python::with_gil(|py| -> PyResult<PyObject> {
                        let stats_dict = PyDict::new(py);
                        stats_dict.set_item("zero_copy_objects", stats.zero_copy_objects)?;
                        stats_dict
                            .set_item("zero_copy_memory_bytes", stats.zero_copy_memory_bytes)?;
                        stats_dict.set_item("shared_objects", stats.shared_objects)?;
                        stats_dict.set_item("shared_memory_bytes", stats.shared_memory_bytes)?;
                        stats_dict.set_item("standard_objects", stats.standard_objects)?;
                        stats_dict
                            .set_item("standard_memory_bytes", stats.standard_memory_bytes)?;
                        stats_dict.set_item("total_objects", stats.total_objects)?;
                        stats_dict.set_item("zero_copy_ratio", stats.zero_copy_ratio)?;
                        Ok(stats_dict.into())
                    })
                })
                .await
                .map_err(|e| PyRuntimeError::new_err(format!("Task join error: {}", e)))?
        })
    }
    /// Memory pressure information
    fn get_memory_pressure_info(&self, py: Python) -> PyResult<PyObject> {
        let info = self.inner.store.get_memory_pressure_info();
        let dict = PyDict::new(py);

        dict.set_item("current_memory_bytes", info.current_memory_bytes)?;
        dict.set_item("max_memory_bytes", info.max_memory_bytes)?;
        dict.set_item("memory_usage_ratio", info.memory_usage_ratio)?;
        dict.set_item("threshold", info.threshold)?;
        dict.set_item("is_under_pressure", info.is_under_pressure)?;
        dict.set_item("pressure_level", info.pressure_level())?;
        dict.set_item("needs_immediate_cleanup", info.needs_immediate_cleanup())?;
        Ok(dict.into())
    }
}

fn parse_object_id(id_str: &str) -> PyResult<ObjectId> {
    id_str
        .parse::<u64>()
        .map(ObjectId)
        .map_err(|_| PyValueError::new_err("Invalid object ID format"))
}

fn map_store_error(error: StoreError) -> PyErr {
    match error {
        StoreError::ObjectNotFound(id) => PyKeyError::new_err(format!("Object {} not found", id)),
        StoreError::ObjectTooLarge { size, max_size } => PyValueError::new_err(format!(
            "Object too large: {} bytes (max: {} bytes)",
            size, max_size
        )),
        StoreError::OutOfMemory { current, max } => PyMemoryError::new_err(format!(
            "Out of memory: using {} bytes (max: {} bytes)",
            current, max
        )),
        StoreError::Internal(msg) => PyRuntimeError::new_err(format!("Internal error: {}", msg)),
    }
}

fn estimate_python_object_size(py: Python, obj: &PyObject) -> PyResult<usize> {
    let sys = py.import("lyricore.utils")?;
    let getsizeof = sys.getattr("get_sizeof")?;
    let size: usize = getsizeof.call1((obj,))?.extract()?;
    Ok(size)
}
