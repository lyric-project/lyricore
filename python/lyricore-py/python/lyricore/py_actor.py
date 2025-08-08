"""
Lyricore Python Actor Framework

A high-performance distributed actor system built on Rust, with Python bindings.
"""

import logging
from dataclasses import dataclass
from typing import TYPE_CHECKING, Any, Dict, Optional, Type

from .error import (
    ActorError,
    ActorNotFoundError,
    ActorStoppedError,
    MessageTimeoutError,
)

# Import the native Rust module
try:
    from ._lyricore import PyActorContext as _PyActorContext
    from ._lyricore import PyActorRef as _PyActorRef
    from ._lyricore import PyActorSystem as _PyActorSystem
    from ._lyricore import PyStoreConfig as _PyStoreConfig
except ImportError as e:
    raise ImportError(
        f"Failed to import native lyricore module: {e}. "
        "Make sure you have compiled the Rust extension."
    ) from e


if TYPE_CHECKING:
    from .actor_wrapper import (
        ObjectStoreActorRef,
        ObjectStoreConfig,
    )

logger = logging.getLogger(__name__)

__all__ = [
    "ActorSystem",
    "ActorRef",
    "ActorContext",
    "actor",
    "ActorError",
    "ActorNotFoundError",
    "ActorStoppedError",
    "MessageTimeoutError",
]


# ============================================================================
# Exception Classes
# ============================================================================


# ============================================================================
# Configuration Classes
# ============================================================================


@dataclass
class ActorSystemConfig:
    """Configuration for ActorSystem."""

    worker_threads: Optional[int] = None
    serialization_format: str = "json"  # "json" | "messagepack"
    max_mailbox_size: int = 10000
    batch_size: int = 128

    def __post_init__(self):
        if self.serialization_format not in ["json", "messagepack"]:
            raise ValueError("serialization_format must be 'json' or 'messagepack'")


# ============================================================================
# Message System
# ============================================================================


# ============================================================================
# Actor Context
# ============================================================================


class ActorContext:
    """Actor context providing access to system services."""

    def __init__(
        self,
        rust_ctx: _PyActorContext,
        objectstore_config: Optional["ObjectStoreConfig"] = None,
    ):
        from .actor_wrapper import ObjectStoreConfig

        self._rust_ctx = rust_ctx
        self.objectstore_config = objectstore_config or ObjectStoreConfig()

    @property
    def actor_id(self) -> str:
        """Get the actor's ID."""
        return self._rust_ctx.actor_id

    async def tell_self(self, message: Any) -> None:
        """Send a message to self."""
        ...

    async def spawn(
        self, actor_class: Type, path: str, *args, **kwargs
    ) -> "ObjectStoreActorRef":
        """Spawn a child actor at the specified path."""
        from .actor_wrapper import (
            _create_actor_init_dict,
            _wrap_actor_class,
        )

        try:
            actor_class = _wrap_actor_class(actor_class)
            construction_task = _create_actor_init_dict(
                actor_class, self.objectstore_config, *args, **kwargs
            )
            logger.debug(f"Creating enhanced actor {actor_class.__name__} at {path}")
            logger.debug(
                f"  Construction task hash: {construction_task['function_hash']}"
            )
            return await self._spawn_construction_task(
                construction_task, path, actor_class._original_class
            )
        except Exception as e:
            logger.error(
                f"Failed to spawn enhanced actor {actor_class.__name__} at {path}: {e}"
            )
            raise RuntimeError(f"Failed to spawn enhanced actor: {e}")

    async def _spawn_construction_task(
        self,
        construction_task: Dict[str, Any],
        path: str,
        actor_class: Optional[Type] = None,
    ) -> "ObjectStoreActorRef":
        """Spawn an actor from a construction task."""
        from .actor_wrapper import ObjectStoreActorRef
        from .proxy_ref import EnhancedObjectStoreActorRef

        try:
            rust_ref = await self._rust_ctx.spawn_from_construction_task(
                construction_task, path
            )
            base_ref = ActorRef(rust_ref)
            original_ref = ObjectStoreActorRef(
                base_ref, self._rust_ctx.get_store(), self.objectstore_config
            )
            return EnhancedObjectStoreActorRef(original_ref, actor_class)
        except Exception as e:
            raise ActorError(
                f"Failed to spawn actor from construction task: {e}"
            ) from e

    async def actor_of(self, path: str) -> "ObjectStoreActorRef":
        """Get reference to an actor by path."""
        from .actor_wrapper import ObjectStoreActorRef
        from .proxy_ref import EnhancedObjectStoreActorRef

        try:
            rust_ref = await self._rust_ctx.actor_of(path)
            base_ref = ActorRef(rust_ref)
            original_ref = ObjectStoreActorRef(
                base_ref, self._rust_ctx.get_store(), self.objectstore_config
            )
            return EnhancedObjectStoreActorRef(original_ref, None)
        except Exception as e:
            if "not found" in str(e).lower():
                raise ActorNotFoundError(f"Actor not found at path: {path}") from e
            else:
                raise ActorError(f"Failed to get actor reference: {e}") from e

    @property
    def self_ref(self) -> "ObjectStoreActorRef":
        """Get a reference to self."""
        from .actor_wrapper import ObjectStoreActorRef
        from .proxy_ref import EnhancedObjectStoreActorRef

        try:
            rust_ref = self._rust_ctx.self_ref
            base_ref = ActorRef(rust_ref)
            original_ref = ObjectStoreActorRef(
                base_ref, self._rust_ctx.get_store(), self.objectstore_config
            )
            return EnhancedObjectStoreActorRef(original_ref, None)
        except Exception as e:
            raise ActorError(f"Failed to get self reference: {e}") from e


# ============================================================================
# Actor Reference
# ============================================================================
class ActorRef:
    """Reference to an actor, supporting both local and remote actors."""

    def __init__(self, rust_ref: _PyActorRef):
        self._rust_ref = rust_ref
        self._init_serializable = True
        self.__actor_ref_path__ = None

    def __getstate__(self):
        state = self.__dict__.copy()
        # Remove the executor as it's not serializable
        state["__actor_ref_path__"] = self.path
        state.pop("_rust_ref", None)
        state.pop("_init_serializable", None)
        return state

    def __setstate__(self, state):
        self.__dict__.update(state)
        self._init_serializable = False

    async def _check_and_init(self):
        if not self._init_serializable:
            from . import get_global_inner_context

            if not self.__actor_ref_path__:
                raise ActorError("ActorRef path is not set for deserialized object")

            inner_context = get_global_inner_context()
            if not inner_context:
                raise ActorError(
                    "Global inner context is not set. Cannot restore ActorRef."
                )

            try:
                rust_ref = await inner_context.actor_of(self.__actor_ref_path__)
                self._rust_ref = rust_ref
                self._init_serializable = True
            except Exception as e:
                raise ActorError(
                    f"Failed to restore ActorRef for path {self.__actor_ref_path__}: {e}"
                ) from e

    async def tell(self, message: Any) -> None:
        """Send a fire-and-forget message to the actor."""
        try:
            await self._check_and_init()
            await self._rust_ref.tell(message)
        except RuntimeError as e:
            if "timeout" in str(e).lower():
                raise MessageTimeoutError(str(e)) from e
            elif "not found" in str(e).lower():
                raise ActorNotFoundError(str(e)) from e
            elif "stopped" in str(e).lower():
                raise ActorStoppedError(str(e)) from e
            else:
                raise ActorError(str(e)) from e

    async def ask(self, message: Any, timeout: Optional[float] = None) -> Any:
        """Send a message and wait for a response."""
        try:
            await self._check_and_init()
            timeout_ms = int(timeout * 1000) if timeout is not None else None
            return await self._rust_ref.ask(message, timeout_ms)
        except TimeoutError as e:
            raise MessageTimeoutError(str(e)) from e
        except RuntimeError as e:
            if "timeout" in str(e).lower():
                raise MessageTimeoutError(str(e)) from e
            elif "not found" in str(e).lower():
                raise ActorNotFoundError(str(e)) from e
            elif "stopped" in str(e).lower():
                raise ActorStoppedError(str(e)) from e
            else:
                raise ActorError(str(e)) from e

    def stop(self) -> None:
        """Stop the actor."""
        if not self._init_serializable:
            return
        self._rust_ref.stop()

    @property
    def path(self) -> str:
        """Get the actor's path."""
        if not self._init_serializable:
            return self.__actor_ref_path__
        return self._rust_ref.path


# ============================================================================
# Actor System
# ============================================================================


class ActorSystem:
    """The main actor system managing actor lifecycle and communication."""

    def __init__(
        self,
        system_name: str,
        listen_address: str = "127.0.0.1:50051",
        config: Optional[ActorSystemConfig] = None,
        store_config: Optional[_PyStoreConfig] = None,
        objectstore_config: Optional["ObjectStoreConfig"] = None,
    ):
        """Initialize the actor system.

        Args:
            system_name: Name of the actor system
            listen_address: Address to listen on (host:port)
            config: Optional configuration
        """
        from .actor_wrapper import ObjectStoreConfig

        self.system_name = system_name
        self.listen_address = listen_address
        self.config = config or ActorSystemConfig()
        self.objectstore_config = objectstore_config or ObjectStoreConfig()
        self._actor_class_registry: Dict[str, Type] = {}
        try:
            self._rust_system = _PyActorSystem(
                system_name, listen_address, self.config.worker_threads, store_config
            )
        except Exception as e:
            raise ActorError(f"Failed to create actor system: {e}") from e

        self._started = False
        self._shutdown = False

    async def start(self) -> None:
        """Start the actor system server."""
        if self._started:
            return

        try:
            await self._rust_system.start()
            self._started = True
        except Exception as e:
            raise ActorError(f"Failed to start actor system: {e}") from e

    async def after_start(self) -> None:
        """Hook to run after the actor system has started."""
        pass

    async def shutdown(self) -> None:
        """Shutdown the actor system."""
        if self._shutdown:
            return

        try:
            await self._rust_system.shutdown()
            self._shutdown = True
            self._started = False
        except Exception as e:
            raise ActorError(f"Failed to shutdown actor system: {e}") from e

    async def spawn(
        self, actor_class: Type, path: str, *args, **kwargs
    ) -> "ObjectStoreActorRef":
        """Spawn an actor at the specified path."""
        from .actor_wrapper import (
            ObjectStoreActorRef,
            _create_actor_init_dict,
            _wrap_actor_class,
        )
        from .proxy_ref import EnhancedObjectStoreActorRef

        if not self._started:
            raise RuntimeError("Actor system not started")

        try:
            # Serialize the construction task for the enhanced actor
            actor_class = _wrap_actor_class(actor_class)
            construction_task = _create_actor_init_dict(
                actor_class, self.objectstore_config, *args, **kwargs
            )

            logger.debug(f"Creating enhanced actor {actor_class.__name__} at {path}")
            logger.debug(
                f"  Construction task hash: {construction_task['function_hash']}"
            )
            self._actor_class_registry[path] = actor_class
            # create actor using rust side core API
            rust_ref = await self._rust_system.spawn_from_construction_task(
                construction_task, path
            )
            base_ref = ActorRef(rust_ref)
            original_ref = ObjectStoreActorRef(
                base_ref, self._rust_system.get_store(), self.objectstore_config
            )
            return EnhancedObjectStoreActorRef(original_ref, actor_class)
        except Exception as e:
            logger.error(
                f"Failed to spawn enhanced actor {actor_class.__name__} at {path}: {e}"
            )
            raise RuntimeError(f"Failed to spawn enhanced actor: {e}")

    async def actor_of(self, path: str) -> "ObjectStoreActorRef":
        """Get a reference to an existing actor.

        Args:
            path: Actor path (e.g., "/user/my_actor" or "lyricore://system@host:port/user/actor")

        Returns:
            ActorRef: Reference to the actor
        """
        from .actor_wrapper import ObjectStoreActorRef
        from .proxy_ref import EnhancedObjectStoreActorRef

        if not self._started:
            raise ActorError("Actor system not started. Call start() first.")

        try:
            rust_ref = await self._rust_system.actor_of(path)
            base_ref = ActorRef(rust_ref)
            original_ref = ObjectStoreActorRef(
                base_ref, self._rust_system.get_store(), self.objectstore_config
            )

            # try to get the actor class from the registry
            actor_class = self._actor_class_registry.get(path)

            return EnhancedObjectStoreActorRef(original_ref, actor_class)
        except Exception as e:
            if "not found" in str(e).lower():
                raise ActorNotFoundError(f"Actor not found at path: {path}") from e
            else:
                raise ActorError(f"Failed to get actor reference: {e}") from e

    async def connect_to_node(self, node_id: str, address: str) -> None:
        """Connect to a remote actor system node.

        Args:
            node_id: ID of the remote node
            address: Address of the remote node (host:port)
        """
        if not self._started:
            raise ActorError("Actor system not started. Call start() first.")

        try:
            await self._rust_system.connect_to_node(node_id, address)
        except Exception as e:
            raise ActorError(f"Failed to connect to node {node_id}: {e}") from e

    async def __aenter__(self):
        """Async context manager entry."""
        await self.start()
        return self

    async def __aexit__(self, exc_type, exc_val, exc_tb):
        """Async context manager exit."""
        await self.shutdown()


def actor(
    cls: Optional[Type] = None,
    *,
    num_cpus: int = 1,
    num_gpus: int = 0,
):
    """Actor decorator for defining actor classes.

    @actor(num_cpus=2, num_gpus=1)
    class MyActor:
        def __init__(self, config):
            self.config = config
        async def on_request(self, req):
            return self.process(req)

    @actor
    class MyActor:
        def __init__(self, config):
            self.config = config
        async def on_request(self, req):
            return self.process(req)
    """

    def decorator(target_cls: Type):
        """The real decorator function."""
        from .actor_wrapper import _wrap_actor_class

        return _wrap_actor_class(target_cls, num_cpus, num_gpus)

    # If cls is provided, it means the decorator is used without parameters
    if cls is not None:
        return decorator(cls)

    # If cls is None, it means the decorator is used with parameters
    return decorator
