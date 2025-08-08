mod object_store;
mod py_actor;
mod py_actor_context;
mod py_actor_ref;
mod py_actor_system;
mod utils;

use crate::py_actor_context::{PyActorContext, PyInnerContext};
use crate::py_actor_ref::PyActorRef;
use crate::py_actor_system::PyActorSystem;
use pyo3::prelude::*;
use std::sync::OnceLock;

pub fn get_lyric_core_version() -> &'static str {
    static LYRIC_CORE_VERSION: OnceLock<String> = OnceLock::new();

    LYRIC_CORE_VERSION.get_or_init(|| {
        let version = env!("CARGO_PKG_VERSION");
        // cargo uses "1.0-alpha1" etc. while python uses "1.0.0a1", this is not full compatibility,
        // but it's good enough for now
        // see https://docs.rs/semver/1.0.9/semver/struct.Version.html#method.parse for rust spec
        // see https://peps.python.org/pep-0440/ for python spec
        // it seems the dot after "alpha/beta" e.g. "-alpha.1" is not necessary, hence why this works
        version.replace("-alpha", "a").replace("-beta", "b")
    })
}
pub fn build_info() -> String {
    format!(
        "profile={} pgo={}",
        env!("PROFILE"),
        // We use a `cfg!` here not `env!`/`option_env!` as those would
        // embed `RUSTFLAGS` into the generated binary which causes problems
        // with reproducable builds.
        cfg!(specified_profile_use),
    )
}

fn _lyricore_init(m: &Bound<PyModule>) -> PyResult<()> {
    m.add_class::<PyActorSystem>()?;
    m.add_class::<PyActorRef>()?;
    m.add_class::<PyActorContext>()?;
    m.add_class::<PyInnerContext>()?;
    Ok(())
}

fn _lyricore_store_init(m: &Bound<PyModule>) -> PyResult<()> {
    use crate::object_store::{PyObjectRef, PyObjectStore, PyObjectView, PyStoreConfig};
    m.add_class::<PyObjectStore>()?;
    m.add_class::<PyObjectRef>()?;
    m.add_class::<PyObjectView>()?;
    m.add_class::<PyStoreConfig>()?;
    Ok(())
}

#[pymodule]
mod _lyricore {
    #[allow(clippy::wildcard_imports)]
    use super::*;

    #[pymodule_init]
    fn module_init(m: &Bound<'_, PyModule>) -> PyResult<()> {
        m.add("__version__", get_lyric_core_version())?;
        m.add("build_profile", env!("PROFILE"))?;
        m.add("build_info", build_info())?;
        _lyricore_init(m)?;
        _lyricore_store_init(m)?;
        Ok(())
    }
}
