import base64
import functools
import hashlib
import logging
import traceback
from dataclasses import dataclass
from typing import Any, Dict, List, Optional, Type, TypeVar

try:
    import numpy as np

    HAS_NUMPY = True
except ImportError:
    HAS_NUMPY = False

    # Create a dummy numpy module for type hints
    class DummyNumpy:
        ndarray = type(None)

    np = DummyNumpy()
from . import pickle
from ._lyricore import PyActorContext, PyObjectStore, PyStoreConfig
from .error import ActorNoRouteError
from .object_store import (
    ActorMessageContextVar,
    get_global_object_store,
    set_global_inner_context,
)
from .py_actor import ActorContext
from .router import _has_message_handlers, _setup_message_routing
from .utils import get_sizeof

T = TypeVar("T")

logger = logging.getLogger(__name__)


@dataclass
class ActorConstructionTask:
    """Enhanced Actor Construction Task with ObjectStore support"""

    constructor_func: bytes
    constructor_args: list
    constructor_kwargs: dict
    function_hash: str
    module_name: str
    class_name: str

    # ObjectStore Configuration
    objectstore_config_dict: dict
    requires_objectstore: bool = True
    capture_globals: Optional[bytes] = None


@dataclass
class ObjectStoreConfig:
    """ObjectStore integration configuration"""

    # The threshold for automatic serialization of large objects
    auto_serialize_threshold: int = 1024 * 1024  # 1MB

    # Supports dynamic types for serialization
    auto_serialize_types: tuple = None

    # When enabled, it allows the framework to optimize serialization
    enable_batch_optimization: bool = True

    # The patterns for automatic serialization of parameters
    auto_serialize_patterns: List[str] = None

    def __post_init__(self):
        if self.auto_serialize_patterns is None:
            self.auto_serialize_patterns = ["data", "array", "buffer", "payload"]

        if self.auto_serialize_types is None:
            types_list = [bytes, bytearray]
            if HAS_NUMPY:
                types_list.append(np.ndarray)
            self.auto_serialize_types = tuple(types_list)


def _create_actor_init_dict(
    cls: Type[T], objectstore_config: ObjectStoreConfig, *args, **kwargs
):
    # TODO: Handle Object Store Ref arguments
    def create_enhanced_actor_instance():
        # It will serialize the actor class and its arguments
        instance = cls(*args, **kwargs)
        return instance

    try:
        serialized_func = pickle.dumps(create_enhanced_actor_instance)
        func_hash = hashlib.sha256(serialized_func).hexdigest()[:16]

        task = ActorConstructionTask(
            constructor_func=serialized_func,
            constructor_args=[],  # All arguments are passed to the closure serialized_func
            constructor_kwargs={},
            function_hash=func_hash,
            module_name=cls.__module__,
            class_name=cls.__qualname__,
            objectstore_config_dict=objectstore_config.__dict__,
            requires_objectstore=True,
        )
        return {
            "constructor_func": base64.b64encode(task.constructor_func).decode("utf-8"),
            "constructor_args": task.constructor_args,
            "constructor_kwargs": task.constructor_kwargs,
            "function_hash": task.function_hash,
            "module_name": task.module_name,
            "class_name": task.class_name,
            "objectstore_config_dict": task.objectstore_config_dict,
            "requires_objectstore": task.requires_objectstore,
            "capture_globals": base64.b64encode(task.capture_globals).decode("utf-8")
            if task.capture_globals
            else None,
        }
    except Exception as e:
        raise RuntimeError(
            f"Failed to serialize local enhanced class construction: {e}"
        )


def _wrap_actor_class(
    cls: Type[T],
    num_cpus: int = 1,
    num_gpus: int = 0,
) -> Type[T]:
    if hasattr(cls, "_is_actor_wrapped"):
        return cls
    original_init = cls.__init__
    original_name = cls.__name__
    original_module_name = cls.__module__
    original_qualname = cls.__qualname__
    # Check if the class has message
    needs_routing = _has_message_handlers(cls)

    def enhanced_init(self, *args, **kwargs):
        # Invoke the original __init__
        original_init(self, *args, **kwargs)

    class WrappedActorClass(cls):
        """Create a wrapped actor class with enhanced init"""

        def __init__(self, *args, **kwargs):
            # super().__init__(*args, **kwargs)
            self._original_args = args
            self._original_kwargs = kwargs
            self.__actor_store__ = get_global_object_store()
            # If the class has message handlers, we need to set up routing
            if needs_routing:
                self.__message_router__ = (
                    None  # Will be initialized when first message is received
                )
            enhanced_init(self, *args, **kwargs)
            self.__actor_config__ = ObjectStoreConfig()
            self.__actor_serializer_ = MessageSerializer(
                self.__actor_store__, self.__actor_config__
            )
            self.__actor_do_wrap_core_methods__()

        # Wrap all core methods
        def __actor_do_wrap_core_methods__(self):
            core_methods = ["on_start", "on_message", "handle_message", "on_stop"]
            if needs_routing and not (
                hasattr(self, "on_message") or hasattr(self, "handle_message")
            ):

                async def default_handle_message(_self, message, ctx):
                    cls_name = type(_self).__name__
                    raise NotImplementedError(
                        f"No message handler defined for {cls_name} "
                        "actor, please implement 'on_message' or 'handle_message' method or "
                        "set 'needs_routing' to False. Received message: "
                        f"{message!r}"
                    )

                setattr(self, "handle_message", default_handle_message.__get__(self))
            for method_name in core_methods:
                if hasattr(self, method_name):
                    original_method = getattr(self, method_name)
                    # If routing is needed and the method is a message handler,
                    if needs_routing and method_name in [
                        "on_message",
                        "handle_message",
                    ]:
                        wrapped_method = _wrap_core_method_with_routing(
                            self,
                            original_method,
                            method_name,
                            self.__actor_store__,
                            self.__actor_serializer_,
                            self.__actor_config__,
                        )
                    else:
                        wrapped_method = _wrap_core_method(
                            original_method,
                            method_name,
                            self.__actor_store__,
                            self.__actor_serializer_,
                            self.__actor_config__,
                        )
                    setattr(self, method_name, wrapped_method)

    WrappedActorClass.__name__ = f"Actor({original_name})"
    WrappedActorClass.__qualname__ = f"Actor({original_qualname})"
    WrappedActorClass.__module__ = original_module_name
    WrappedActorClass._actor_config = {
        "num_cpus": num_cpus,
        "num_gpus": num_gpus,
    }
    WrappedActorClass._is_actor_wrapped = True
    WrappedActorClass._original_class = cls
    WrappedActorClass._has_routing = needs_routing
    return WrappedActorClass


def _wrap_core_method(
    original_method,
    method_name: str,
    store: PyObjectStore,
    serializer: "MessageSerializer",
    config: ObjectStoreConfig,
):
    """Wrap core Actor methods to handle serialization and deserialization"""

    @functools.wraps(original_method)
    async def core_wrapper(*args, **kwargs):
        # For methods like on_message and handle_message, we need to handle the message parameter specially
        actor_ctx: Optional[ActorContext] = None
        for arg in args:
            if isinstance(arg, PyActorContext):
                actor_ctx = ActorContext(arg, config)
                set_global_inner_context(arg.get_inner_ctx())

        if method_name in ["on_message", "handle_message"] and args:
            # The first argument is the message, which needs to be deserialized
            message_bytes = args[0]
            if isinstance(message_bytes, bytes):
                # Try to deserialize the message
                try:
                    message = pickle.loads(message_bytes)
                except Exception as e:
                    logger.info(f"Warning: Failed to deserialize message: {e}")
                    message = message_bytes
            else:
                message = message_bytes

            deserialized_message = await _deserialize_message_content(store, message)
            args = (deserialized_message,) + args[1:]

        def _arg_func(arg):
            """Handle the arguments to convert PyActorContext to ActorContext"""
            if isinstance(arg, PyActorContext):
                return ActorContext(arg, config)
            return arg

        # Deserializing other arguments
        deserialized_args, deserialized_kwargs = await serializer.deserialize_args(
            args, kwargs, arg_func=_arg_func
        )

        # To invoke the original method with deserialized arguments
        with ActorMessageContextVar(actor_ctx):
            result = await original_method(*deserialized_args, **deserialized_kwargs)

        if result is not None and method_name == "handle_message":
            # Try to serialize the result if needed
            final_result = await _serialize_result_if_needed(result, store, serializer)
            return final_result

        return result

    return core_wrapper


def _wrap_core_method_with_routing(
    self,
    original_method,
    method_name: str,
    store: PyObjectStore,
    serializer: "MessageSerializer",
    config: ObjectStoreConfig,
):
    """Wrap core Actor methods with routing support"""

    @functools.wraps(original_method)
    async def core_wrapper(*args, **kwargs):
        # Deserialize the message if it's a message handler
        # Set context store
        actor_ctx: Optional[ActorContext] = None

        for arg in args:
            if isinstance(arg, PyActorContext):
                actor_ctx = ActorContext(arg, config)
                set_global_inner_context(arg.get_inner_ctx())
        if method_name in ["on_message", "handle_message"] and args:
            message_bytes = args[0]
            if isinstance(message_bytes, bytes):
                try:
                    message = pickle.loads(message_bytes)
                except Exception as e:
                    logger.warning(f"Warning: Failed to deserialize message: {e}")
                    message = message_bytes
            else:
                message = message_bytes

            deserialized_message = await _deserialize_message_content(store, message)
            args = (deserialized_message,) + args[1:]

        def _arg_func(arg):
            if isinstance(arg, PyActorContext):
                return actor_ctx
            return arg

        # Deserialize args and kwargs
        deserialized_args, deserialized_kwargs = await serializer.deserialize_args(
            args, kwargs, arg_func=_arg_func
        )

        # Try to use the message router if it exists
        if hasattr(self, "__message_router__"):
            # Initialize the message router if it doesn't exist
            if self.__message_router__ is None:
                self.__message_router__ = _setup_message_routing(self)

            # Get the message and context from deserialized arguments
            message = deserialized_args[0] if deserialized_args else None
            ctx = deserialized_args[1] if len(deserialized_args) > 1 else None

            try:
                # Try routing the message, raise an exception if routing fails
                with ActorMessageContextVar(actor_ctx):
                    result = await self.__message_router__.route(message, ctx)

                if result is not None:
                    # Deserialize the result if needed when routing is successful and method is handle_message
                    if method_name == "handle_message":
                        final_result = await _serialize_result_if_needed(
                            result, store, serializer
                        )
                        return final_result
                else:
                    # The result is None, which means the message was sent to handle
                    # and the routing was successful, just return None
                    return None
            except ActorNoRouteError:
                # Just log the error and continue to invoke the original method
                logger.debug("No routing handler found for message, ")
            except Exception as e:
                msg_err = traceback.format_exc()
                logger.error(f"Error during message routing: {msg_err}")
                raise e from e

        # Invoke the original method with deserialized arguments if no routing was done

        with ActorMessageContextVar(actor_ctx):
            result = await original_method(*deserialized_args, **deserialized_kwargs)

        # Serialize the result if needed
        if result is not None and method_name == "handle_message":
            final_result = await _serialize_result_if_needed(result, store, serializer)
            return final_result

        return result

    return core_wrapper


async def _deserialize_message_content(store: PyObjectStore, message: Any) -> Any:
    """Deserialize message content, handling ObjectStore references and large data"""
    if isinstance(message, dict):
        # Handle single ObjectStore reference
        if "_objectstore_ref" in message:
            ref_info = message["_objectstore_ref"]
            object_id = ref_info["object_id"]
            obj_type = ref_info["type"]

            if obj_type == "numpy.ndarray":
                return await store.get_numpy(object_id)
            elif obj_type == "bytes":
                return await store.get_bytes(object_id)
            else:
                return await store.get_object(object_id)

        # Handle Numpy bytes format
        elif "_numpy_bytes" in message:
            numpy_info = message["_numpy_bytes"]
            if HAS_NUMPY:
                data = numpy_info["data"]
                shape = numpy_info["shape"]
                dtype = numpy_info["dtype"]
                array = np.frombuffer(data, dtype=dtype).reshape(shape)
                return array
            else:
                return numpy_info["data"]

        # Include dictionary with ObjectStore references
        elif message.get("_has_objectstore_refs"):
            deserialized_dict = {}
            for key, value in message.items():
                if key in ["_has_objectstore_refs", "_large_data_message"]:
                    deserialized_dict[key] = value
                    continue
                deserialized_dict[key] = await _deserialize_message_content(
                    store, value
                )
            return deserialized_dict

    return message


class ObjectStoreRef:
    """ObjectStore reference wrapper, for handling large objects in ObjectStore."""

    def __init__(
        self, object_id: str, store: PyObjectStore, metadata: Optional[Dict] = None
    ):
        self.object_id = object_id
        self.store = store
        self.metadata = metadata or {}
        self._cached_object = None
        self._is_loaded = False

    async def get(self) -> Any:
        """Get the actual object from ObjectStore."""
        if not self._is_loaded:
            if self.metadata.get("is_numpy", False):
                self._cached_object = await self.store.get_numpy(self.object_id)
            elif self.metadata.get("type") == "bytes":
                self._cached_object = await self.store.get_bytes(self.object_id)
            else:
                self._cached_object = await self.store.get_object(self.object_id)
            self._is_loaded = True
        return self._cached_object

    def __repr__(self):
        return f"ObjectStoreRef(id={self.object_id}, type={self.metadata.get('type', 'unknown')})"


class MessageSerializer:
    def __init__(self, store, config: ObjectStoreConfig):
        self.store = store  # PyObjectStore Instance
        self.config = config

    async def serialize_args(self, args: tuple, kwargs: dict) -> tuple[tuple, dict]:
        """Serialize function arguments"""
        serialized_args = []
        serialized_kwargs = {}

        # Handle positional arguments
        for arg in args:
            serialized_arg = await self._serialize_value(arg)
            serialized_args.append(serialized_arg)

        # Handle keyword arguments
        for key, value in kwargs.items():
            serialized_value = await self._serialize_value(value, param_name=key)
            serialized_kwargs[key] = serialized_value

        return tuple(serialized_args), serialized_kwargs

    async def deserialize_args(
        self, args: tuple, kwargs: dict, arg_func=None
    ) -> tuple[tuple, dict]:
        """Deserialize function arguments"""
        deserialized_args = []
        deserialized_kwargs = {}

        # Handle positional arguments
        for arg in args:
            deserialized_arg = await self._deserialize_value(arg)
            if arg_func is not None and callable(arg_func):
                deserialized_arg = arg_func(deserialized_arg)
            deserialized_args.append(deserialized_arg)

        # Handle keyword arguments
        for key, value in kwargs.items():
            deserialized_value = await self._deserialize_value(value)
            deserialized_kwargs[key] = deserialized_value

        return tuple(deserialized_args), deserialized_kwargs

    async def _serialize_value(self, value: Any, param_name: str = None) -> Any:
        """Default serialization logic for a single value"""
        # Check if the value should be serialized
        if not self._should_serialize(value, param_name):
            return value

        try:
            # Handle NumPy arrays
            if HAS_NUMPY and isinstance(value, np.ndarray):
                object_id = await self.store.put_numpy(value)
                return ObjectStoreRef(
                    object_id,
                    self.store,
                    {
                        "is_numpy": True,
                        "type": "numpy.ndarray",
                        "shape": value.shape,
                        "dtype": str(value.dtype),
                    },
                )

            # Handle bytes and bytearrays
            elif isinstance(value, (bytes, bytearray)):
                if isinstance(value, bytearray):
                    value = bytes(value)
                object_id = await self.store.put_bytes(value)
                return ObjectStoreRef(
                    object_id,
                    self.store,
                    {"is_numpy": False, "type": "bytes", "size": len(value)},
                )

            # Handle other objects
            else:
                # TODO: it might be better to use a more efficient serialization method
                object_id = await self.store.put(value)
                return ObjectStoreRef(
                    object_id,
                    self.store,
                    {
                        "is_numpy": False,
                        "type": type(value).__name__,
                        "size": get_sizeof(value),
                    },
                )

        except Exception as e:
            # Failed to serialize, log the error and return the original value
            logger.info(f"Warning: Failed to serialize {type(value)}: {e}")
            return value

    async def _deserialize_value(self, value: Any) -> Any:
        """Default deserialization logic for a single value"""
        if isinstance(value, ObjectStoreRef):
            return await value.get()
        return value

    def _should_serialize(self, value: Any, param_name: str = None) -> bool:
        """Judge whether the value should be serialized based on size and type"""
        # Check if the value is in the auto-serialize types
        if HAS_NUMPY and isinstance(value, np.ndarray):
            size = value.nbytes
            if size >= self.config.auto_serialize_threshold:
                return True
        elif isinstance(value, (bytes, bytearray)):
            size = len(value)
            if size >= self.config.auto_serialize_threshold:
                return True
        else:
            size = get_sizeof(value)
            if size >= self.config.auto_serialize_threshold:
                return True

        # If param_name is provided, check against auto-serialize patterns
        if param_name and any(
            pattern in param_name.lower()
            for pattern in self.config.auto_serialize_patterns
        ):
            if get_sizeof(value) >= self.config.auto_serialize_threshold:
                return True

        return False


class ObjectStoreActorRef:
    """Support for ObjectStore integration in ActorRef"""

    def __init__(
        self, original_ref, store: PyObjectStore, config: ObjectStoreConfig = None
    ):
        self.ref = original_ref
        self.store = store
        self.config = config or ObjectStoreConfig()
        self.serializer = MessageSerializer(store, self.config)

    def __getstate__(self):
        state = self.__dict__.copy()
        # Remove the executor as it's not serializable
        state.pop("store", None)
        state.pop("serializer", None)
        return state

    def __setstate__(self, state):
        from . import get_global_object_store

        self.__dict__.update(state)
        # Recreate the executor
        self.store = get_global_object_store()
        self.serializer = MessageSerializer(self.store, self.config)

    async def tell(self, message: Any) -> None:
        """Send a message to the actor (enhanced, supports automatic serialization)"""
        serialized_message = await self._serialize_message(message)
        serialized_message_bytes = pickle.dumps(serialized_message)
        await self.ref.tell(serialized_message_bytes)

    async def ask(self, message: Any, timeout: Optional[float] = None) -> Any:
        """Send a message to the actor and wait for a response (enhanced, supports automatic serialization)"""
        serialized_message = await self._serialize_message(message)
        # Serialize the message to bytes
        serialized_message_bytes = pickle.dumps(serialized_message)
        result_bytes = await self.ref.ask(serialized_message_bytes, timeout)
        # Deserialize the response bytes
        if result_bytes is None:
            logger.debug("Received None response from actor.")
            return None
        logger.debug(f"Received response bytes: {result_bytes[:100]}... (truncated)")
        result = pickle.loads(result_bytes)
        return await self._deserialize_response(result)

    async def _serialize_message(self, message: Any) -> Any:
        """Serialize a message, handling large objects and ObjectStore references"""
        if self.serializer._should_serialize(message):
            # Big objects: store in ObjectStore and return reference information
            try:
                if HAS_NUMPY and isinstance(message, np.ndarray):
                    object_id = await self.store.put_numpy(message)
                    return {
                        "_objectstore_ref": {
                            "object_id": object_id,
                            "type": "numpy.ndarray",
                            "shape": message.shape,
                            "dtype": str(message.dtype),
                            "size_mb": message.nbytes / (1024 * 1024),
                        }
                    }
                elif isinstance(message, (bytes, bytearray)):
                    if isinstance(message, bytearray):
                        message = bytes(message)
                    object_id = await self.store.put_bytes(message)
                    return {
                        "_objectstore_ref": {
                            "object_id": object_id,
                            "type": "bytes",
                            "size": len(message),
                        }
                    }
                else:
                    object_id = await self.store.put(message)
                    return {
                        "_objectstore_ref": {
                            "object_id": object_id,
                            "type": type(message).__name__,
                            "size": get_sizeof(message),
                        }
                    }
            except Exception as e:
                logger.warning(f"Warning: Failed to serialize large object: {e}")
                # Depending on the type of message, we can try to convert it to bytes
                if HAS_NUMPY and isinstance(message, np.ndarray):
                    return {
                        "_numpy_bytes": {
                            "data": message.tobytes(),
                            "shape": message.shape,
                            "dtype": str(message.dtype),
                        }
                    }
                return message

        # Check if the message is a dictionary
        elif isinstance(message, dict):
            serialized_dict = {}
            has_large_objects = False

            for key, value in message.items():
                if self.serializer._should_serialize(value):
                    has_large_objects = True
                    try:
                        if HAS_NUMPY and isinstance(value, np.ndarray):
                            object_id = await self.store.put_numpy(value)
                            serialized_dict[key] = {
                                "_objectstore_ref": {
                                    "object_id": object_id,
                                    "type": "numpy.ndarray",
                                    "shape": value.shape,
                                    "dtype": str(value.dtype),
                                }
                            }
                        elif isinstance(value, (bytes, bytearray)):
                            if isinstance(value, bytearray):
                                value = bytes(value)
                            object_id = await self.store.put_bytes(value)
                            serialized_dict[key] = {
                                "_objectstore_ref": {
                                    "object_id": object_id,
                                    "type": "bytes",
                                    "size": len(value),
                                }
                            }
                        else:
                            object_id = await self.store.put(value)
                            serialized_dict[key] = {
                                "_objectstore_ref": {
                                    "object_id": object_id,
                                    "type": type(value).__name__,
                                }
                            }
                    except Exception as e:
                        logger.warning(f"Warning: Failed to serialize {key}: {e}")
                        # Deserialize large objects to bytes if possible
                        if HAS_NUMPY and isinstance(value, np.ndarray):
                            serialized_dict[key] = {
                                "_numpy_bytes": {
                                    "data": value.tobytes(),
                                    "shape": value.shape,
                                    "dtype": str(value.dtype),
                                }
                            }
                        else:
                            serialized_dict[key] = value
                else:
                    serialized_dict[key] = value

            if has_large_objects:
                serialized_dict["_has_objectstore_refs"] = True
                return serialized_dict

        return message

    async def _deserialize_response(self, response: Any) -> Any:
        """Deserialize the response, handling ObjectStore references and large data"""
        if isinstance(response, dict):
            # Handle single ObjectStore reference
            if "_objectstore_ref" in response:
                ref_info = response["_objectstore_ref"]
                object_id = ref_info["object_id"]
                obj_type = ref_info["type"]

                if obj_type == "numpy.ndarray":
                    return await self.store.get_numpy(object_id)
                elif obj_type == "bytes":
                    return await self.store.get_bytes(object_id)
                else:
                    return await self.store.get_object(object_id)

            # Handle Numpy bytes format
            elif "_numpy_bytes" in response:
                numpy_info = response["_numpy_bytes"]
                if HAS_NUMPY:
                    data = numpy_info["data"]
                    shape = numpy_info["shape"]
                    dtype = numpy_info["dtype"]
                    array = np.frombuffer(data, dtype=dtype).reshape(shape)
                    return array
                else:
                    return numpy_info["data"]

            # Handle dictionary with ObjectStore references
            elif response.get("_has_objectstore_refs"):
                deserialized_dict = {}
                for key, value in response.items():
                    if key == "_has_objectstore_refs":
                        continue
                    deserialized_dict[key] = await self._deserialize_response(value)
                return deserialized_dict

        return response

    def stop(self):
        return self.ref.stop()

    @property
    def path(self):
        return self.ref.path

    def __getattr__(self, name):
        return getattr(self.ref, name)


async def _serialize_result_if_needed(result, store, serializer):
    """Default serialization logic for the result of an actor method"""
    final_result = result

    if result is not None and serializer._should_serialize(result):
        # For large objects, return reference information instead of direct serialization
        if HAS_NUMPY and isinstance(result, np.ndarray):
            object_id = await store.put_numpy(result)
            final_result = {
                "_objectstore_ref": {
                    "object_id": object_id,
                    "type": "numpy.ndarray",
                    "shape": result.shape,
                    "dtype": str(result.dtype),
                }
            }
        elif isinstance(result, (bytes, bytearray)):
            if isinstance(result, bytearray):
                result = bytes(result)
            object_id = await store.put_bytes(result)
            final_result = {
                "_objectstore_ref": {
                    "object_id": object_id,
                    "type": "bytes",
                    "size": len(result),
                }
            }
        else:
            object_id = await store.put(result)
            final_result = {
                "_objectstore_ref": {
                    "object_id": object_id,
                    "type": type(result).__name__,
                }
            }

    # Deserialize the final result if it is not None
    if final_result is not None:
        try:
            final_result = pickle.dumps(final_result)
        except Exception as e:
            logger.warning(f"Warning: Failed to serialize result: {e}")
            final_result = None

    return final_result
