import asyncio
import contextvars
import threading
from collections import deque
from typing import Any, Optional

from ._lyricore import PyInnerContext, PyObjectStore


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
    _async_local: contextvars.ContextVar[Optional[PyInnerContext]] = (
        contextvars.ContextVar("current_object", default=None)
    )

    @classmethod
    def set_global_object(cls, store: PyInnerContext) -> None:
        """Set the global object store."""
        if not isinstance(store, PyInnerContext):
            raise TypeError("store must be an instance of PyInnerContext")

        is_async = _is_async_context()
        if is_async:
            cls._async_local.set(store)
        else:
            cls._thread_local.store = store

    @classmethod
    def get_global_object(cls) -> PyInnerContext:
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


class ActorMessageContextVar:
    """Global actor message context with async/sync context awareness."""

    _thread_local = threading.local()
    _async_local: contextvars.ContextVar = contextvars.ContextVar(
        "current_ctx_stack", default=deque()
    )

    def __init__(self, context: Any):
        self.context = context

    def __enter__(self):
        ActorMessageContextVar.enter_context(self.context)
        return self

    def __exit__(self, exc_type, exc_val, exc_tb):
        ActorMessageContextVar.exit_context()

    @classmethod
    def enter_context(cls, context: Any) -> None:
        """Set the global actor message context."""
        is_async = _is_async_context()
        if is_async:
            stack = cls._async_local.get()
            stack.append(context)
            cls._async_local.set(stack)
        else:
            if not hasattr(cls._thread_local, "current_ctx_stack"):
                cls._thread_local.current_dag_stack = deque()
            cls._thread_local.current_dag_stack.append(context)

    @classmethod
    def exit_context(cls) -> None:
        """Clear the global actor message context."""
        is_async = _is_async_context()
        if is_async:
            stack = cls._async_local.get()
            if stack:
                stack.pop()
                cls._async_local.set(stack)
        else:
            if (
                hasattr(cls._thread_local, "current_ctx_stack")
                and cls._thread_local.current_dag_stack
            ):
                cls._thread_local.current_dag_stack.pop()

    @classmethod
    def get_current_context(cls) -> Optional[Any]:
        """Get the current actor message context."""
        is_async = _is_async_context()
        if is_async:
            stack = cls._async_local.get()
            return stack[-1] if stack else None
        else:
            if (
                hasattr(cls._thread_local, "current_ctx_stack")
                and cls._thread_local.current_dag_stack
            ):
                return cls._thread_local.current_dag_stack[-1]
            return None


def get_global_object_store() -> PyObjectStore:
    """Get the global object store."""
    return get_global_inner_context().get_store()


def get_global_inner_context() -> PyInnerContext:
    """Set the global inner context."""
    return ObjectStoreVar.get_global_object()


def set_global_inner_context(inner_context: PyInnerContext) -> None:
    ObjectStoreVar.set_global_object(inner_context)


def get_current_message_context() -> Optional[Any]:
    """Get the current actor message context."""
    return ActorMessageContextVar.get_current_context()
