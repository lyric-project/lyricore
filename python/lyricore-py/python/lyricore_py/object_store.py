import asyncio
import contextvars
import threading
from typing import Optional

from ._lyricore_py import PyObjectStore


def _is_async_context():
    """Check if we're in an async context."""
    try:
        loop = asyncio.get_running_loop()
        return asyncio.current_task(loop=loop) is not None
    except RuntimeError:
        return False


class ObjectStoreVar:
    """Global object store with async/sync context awareness."""

    _thread_local = threading.local()
    _async_local: contextvars.ContextVar[Optional[PyObjectStore]] = (
        contextvars.ContextVar("current_object_store", default=None)
    )

    @classmethod
    def set_global_object_store(cls, store: PyObjectStore) -> None:
        """Set the global object store."""
        if not isinstance(store, PyObjectStore):
            raise TypeError("store must be an instance of PyObjectStore")

        is_async = _is_async_context()
        if is_async:
            cls._async_local.set(store)
        else:
            cls._thread_local.store = store

    @classmethod
    def get_global_object_store(cls) -> PyObjectStore:
        """Get the global object store."""
        is_async = _is_async_context()
        if is_async:
            store = cls._async_local.get()
            if store is None:
                raise RuntimeError("Object store not set in async context")
            return store
        else:
            store = getattr(cls._thread_local, "store", None)
            if store is None:
                raise RuntimeError("Object store not set in sync context")
            return store


def get_global_object_store() -> PyObjectStore:
    """Get the global object store."""
    return ObjectStoreVar.get_global_object_store()


def set_global_object_store(store: PyObjectStore) -> None:
    """Set the global object store."""
    ObjectStoreVar.set_global_object_store(store)
