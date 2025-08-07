use pyo3::conversion::{FromPyObject, IntoPyObject};
use pyo3::types::{PyAny, PyDict, PyList};
use pyo3::types::{PyAnyMethods, PyDictMethods, PyListMethods};
use pyo3::BoundObject;
use pyo3::{Bound, PyResult, Python};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::convert::Infallible;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum PyValue {
    None,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    Bytes(Vec<u8>),
    List(Vec<PyValue>),
    Dict(HashMap<String, PyValue>),
}

impl<'py> IntoPyObject<'py> for PyValue {
    type Target = PyAny;
    type Output = Bound<'py, Self::Target>;
    type Error = Infallible;

    fn into_pyobject(self, py: Python<'py>) -> Result<Self::Output, Self::Error> {
        Ok(match self {
            PyValue::None => py.None().into_bound(py).into_any(),
            PyValue::Bool(v) => v.into_pyobject(py).unwrap().into_any().into_bound(),
            PyValue::Int(v) => v.into_pyobject(py).unwrap().into_any().into_bound(),
            PyValue::Float(v) => v.into_pyobject(py).unwrap().into_any().into_bound(),
            PyValue::String(v) => v.into_pyobject(py).unwrap().into_any().into_bound(),
            PyValue::Bytes(v) => v.into_pyobject(py).unwrap().into_any().into_bound(),
            PyValue::List(v) => {
                let list = PyList::empty(py);
                for item in v {
                    list.append(item).unwrap();
                }
                list.into_any().into_bound()
            }
            PyValue::Dict(v) => {
                let dict = PyDict::new(py);
                for (key, value) in v {
                    dict.set_item(key, value).unwrap();
                }
                dict.into_any().into_bound()
            }
        })
    }
}

impl<'py> IntoPyObject<'py> for &PyValue {
    type Target = PyAny;
    type Output = Bound<'py, Self::Target>;
    type Error = Infallible;

    fn into_pyobject(self, py: Python<'py>) -> Result<Self::Output, Self::Error> {
        Ok(match self {
            PyValue::None => py.None().into_bound(py).into_any(),
            PyValue::Bool(v) => v.into_pyobject(py).unwrap().into_any().into_bound(),
            PyValue::Int(v) => v.into_pyobject(py).unwrap().into_any().into_bound(),
            PyValue::Float(v) => v.into_pyobject(py).unwrap().into_any().into_bound(),
            PyValue::String(v) => v.into_pyobject(py).unwrap().into_any().into_bound(),
            PyValue::Bytes(v) => v.into_pyobject(py).unwrap().into_any().into_bound(),
            PyValue::List(v) => {
                let list = PyList::empty(py);
                for item in v {
                    list.append(item).unwrap();
                }
                list.into_any().into_bound()
            }
            PyValue::Dict(v) => {
                let dict = PyDict::new(py);
                for (key, value) in v {
                    dict.set_item(key, value).unwrap();
                }
                dict.into_any().into_bound()
            }
        })
    }
}

impl<'py> FromPyObject<'py> for PyValue {
    fn extract_bound(ob: &Bound<'py, PyAny>) -> PyResult<Self> {
        if ob.is_none() {
            Ok(PyValue::None)
        } else if let Ok(v) = ob.extract::<bool>() {
            Ok(PyValue::Bool(v))
        } else if let Ok(v) = ob.extract::<i64>() {
            Ok(PyValue::Int(v))
        } else if let Ok(v) = ob.extract::<f64>() {
            Ok(PyValue::Float(v))
        } else if let Ok(v) = ob.extract::<String>() {
            Ok(PyValue::String(v))
        } else if let Ok(v) = ob.extract::<Vec<u8>>() {
            Ok(PyValue::Bytes(v))
        } else if let Ok(list) = ob.downcast::<PyList>() {
            let mut vec = Vec::new();
            for item in list.iter() {
                vec.push(item.extract()?);
            }
            Ok(PyValue::List(vec))
        } else if let Ok(dict) = ob.downcast::<PyDict>() {
            let mut map = HashMap::new();
            for (key, value) in dict {
                let key_str: String = key.extract()?;
                map.insert(key_str, value.extract()?);
            }
            Ok(PyValue::Dict(map))
        } else {
            // For any other type, we'll convert it to a string representation
            tracing::warn!("Unsupported PyValue type: {:?}", ob.get_type());
            Ok(PyValue::String(ob.str()?.to_string()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pyo3::Python;

    #[test]
    fn test_py_value_conversion() {
        Python::with_gil(|py| {
            let py_value = PyValue::Dict(HashMap::from([
                ("key1".to_string(), PyValue::Int(42)),
                ("key2".to_string(), PyValue::String("value".to_string())),
            ]));

            let py_obj: Bound<PyAny> = py_value.clone().into_pyobject(py).unwrap();
            let extracted: PyValue = FromPyObject::extract_bound(&py_obj).unwrap();

            assert_eq!(py_value, extracted);
        });
    }
}
