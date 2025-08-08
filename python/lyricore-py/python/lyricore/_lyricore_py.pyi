"""
Lyricore Python Type Definitions

This file defines type information for all interfaces exported from Rust to Python.
Includes type definitions for Actor system, object storage, Actor references and other core components.
"""

import asyncio
from typing import (
    Any,
    AsyncGenerator,
    Awaitable,
    Callable,
    Dict,
    Generic,
    List,
    Optional,
    Sequence,
    Tuple,
    TypeVar,
    Union,
)

import numpy as np
from typing_extensions import Self

# 版本信息
__version__: str
build_info: str
build_profile: str

# ============================================================================
# Object Storage Related Type Definitions
# ============================================================================

class PyCopyMode:
    """
    Python Copy Mode Enum

    Defines different modes for handling data copying in object storage:
    - ZeroCopy: Zero-copy mode, uses raw memory references directly
    - FastCopy: Fast copy mode, optimized memory copying
    - SafeCopy: Safe copy mode, standard memory copying
    """

    def __new__(cls, mode: str) -> Self:
        """
        Create copy mode instance

        Args:
            mode: Copy mode string, supports 'zerocopy', 'fastcopy', 'safecopy'

        Returns:
            PyCopyMode: Copy mode instance
        """
        ...

    def __str__(self) -> str:
        """Return string representation of copy mode"""
        ...

class PyStoreConfig:
    """
    Python Object Store Configuration

    Configures various parameters for object storage, including memory limits, object size limits, etc.
    """

    def __new__(
        cls,
        max_memory: int = 1024 * 1024 * 1024,
        max_object_size: int = 64 * 1024 * 1024,
        memory_pressure_threshold: float = 0.8,
        track_access_time: bool = True,
    ) -> Self:
        """
        Create storage configuration instance

        Args:
            max_memory: Maximum memory usage (bytes), default 1GB
            max_object_size: Maximum single object size (bytes), default 64MB
            memory_pressure_threshold: Memory pressure threshold (0.0-1.0), default 0.8
            track_access_time: Whether to track access time, default True
        """
        ...

    @property
    def max_memory(self) -> int:
        """Get maximum memory limit"""
        ...

    @property
    def max_object_size(self) -> int:
        """Get maximum single object size"""
        ...

    @property
    def memory_pressure_threshold(self) -> float:
        """Get memory pressure threshold"""
        ...

    @property
    def track_access_time(self) -> bool:
        """Whether to track access time"""
        ...

    def __repr__(self) -> str:
        """Return string representation of configuration"""
        ...

class PyObjectRef:
    """
    Python Object Reference

    Represents a reference to an object stored in object storage, providing access to underlying data.
    """

    @property
    def id(self) -> str:
        """Get object ID"""
        ...

    @property
    def size(self) -> int:
        """Get object size (bytes)"""
        ...

    @property
    def data_type(self) -> str:
        """Get object data type"""
        ...

    def as_bytes(self) -> bytes:
        """
        Get raw byte data of the object

        Returns:
            bytes: Object byte data
        """
        ...

    def as_memoryview(self) -> memoryview:
        """
        Get memory view of the object

        Returns:
            memoryview: Object memory view
        """
        ...

    def as_numpy(self) -> Optional[np.ndarray]:
        """
        Get NumPy array representation of the object (if compatible)

        Returns:
            Optional[np.ndarray]: NumPy array, or None if not compatible
        """
        ...

    def metadata(self) -> Dict[str, Any]:
        """
        Get object metadata

        Returns:
            Dict[str, Any]: Dictionary containing object metadata
        """
        ...

    def __repr__(self) -> str:
        """Return string representation of object reference"""
        ...

class PyObjectView:
    """
    Python Object View

    Provides read-only view to stored objects, supporting zero-copy access.
    """

    @property
    def id(self) -> str:
        """Get object ID"""
        ...

    @property
    def size(self) -> int:
        """Get object size (bytes)"""
        ...

    @property
    def data_type(self) -> str:
        """Get object data type"""
        ...

    def as_bytes(self) -> bytes:
        """
        Get raw byte data of the object

        Returns:
            bytes: Object byte data
        """
        ...

    def as_numpy(self) -> Optional[np.ndarray]:
        """
        Get NumPy array representation of the object (if compatible)

        Returns:
            Optional[np.ndarray]: NumPy array, or None if not compatible
        """
        ...

    def to_object(self) -> Any:
        """
        Convert view to Python object (using pickle deserialization)

        Returns:
            Any: Deserialized Python object
        """
        ...

    def __repr__(self) -> str:
        """Return string representation of object view"""
        ...

class PyObjectStore:
    """
    Python Object Store

    High-performance object storage system supporting multiple storage modes and optimization strategies.
    """

    def __new__(
        cls,
        config: Optional[PyStoreConfig] = None,
        worker_threads: Optional[int] = None,
    ) -> Self:
        """
        Create object store instance

        Args:
            config: Storage configuration, uses default if None
            worker_threads: Number of worker threads, uses default if None
        """
        ...

    async def put(self, obj: Any) -> str:
        """
        Store object (serialized as byte array)

        Args:
            obj: Python object to store

        Returns:
            str: Object ID
        """
        ...

    async def get(self, object_id: str) -> PyObjectRef:
        """
        Get object reference

        Args:
            object_id: Object ID

        Returns:
            PyObjectRef: Object reference
        """
        ...

    async def get_object(self, object_id: str) -> Any:
        """
        Get object and deserialize

        Args:
            object_id: Object ID

        Returns:
            Any: Deserialized Python object
        """
        ...

    async def put_bytes(self, data: bytes) -> str:
        """
        直接存储字节数据

        Args:
            data: 要存储的字节数据

        Returns:
            str: 对象ID
        """
        ...

    async def get_bytes(self, object_id: str) -> bytes:
        """
        获取对象的原始字节数据

        Args:
            object_id: 对象ID

        Returns:
            bytes: 对象的字节数据
        """
        ...

    async def put_numpy(self, array: np.ndarray, copy_mode: str = "fast_copy") -> str:
        """
        高性能存储NumPy数组

        Args:
            array: NumPy数组
            copy_mode: 复制模式 ('zerocopy', 'fastcopy', 'safecopy')

        Returns:
            str: 对象ID
        """
        ...

    async def put_numpy_batch_optimized(
        self, arrays: List[np.ndarray], copy_mode: str = "fast_copy"
    ) -> List[str]:
        """
        批量高性能存储NumPy数组

        Args:
            arrays: NumPy数组列表
            copy_mode: 复制模式 ('zerocopy', 'fastcopy', 'safecopy')

        Returns:
            List[str]: 对象ID列表
        """
        ...

    async def put_numpy_adaptive(
        self,
        array: np.ndarray,
        size_threshold: int = 1048576,
        force_copy: Optional[bool] = None,
    ) -> Dict[str, Any]:
        """
        自适应存储NumPy数组（基于大小阈值）

        Args:
            array: NumPy数组
            size_threshold: 大小阈值（字节）
            force_copy: 是否强制复制

        Returns:
            Dict[str, Any]: 包含对象ID、复制模式和估算大小的字典
        """
        ...

    async def get_numpy(self, object_id: str) -> np.ndarray:
        """
        获取NumPy数组（零拷贝视图）

        Args:
            object_id: 对象ID

        Returns:
            np.ndarray: NumPy数组
        """
        ...

    async def put_batch(self, objects: List[Any]) -> List[str]:
        """
        批量存储对象

        Args:
            objects: 对象列表

        Returns:
            List[str]: 对象ID列表
        """
        ...

    async def get_batch(self, object_ids: List[str]) -> List[PyObjectRef]:
        """
        批量获取对象引用

        Args:
            object_ids: 对象ID列表

        Returns:
            List[PyObjectRef]: 对象引用列表
        """
        ...

    async def get_objects(self, object_ids: List[str]) -> List[Any]:
        """
        批量获取对象并反序列化

        Args:
            object_ids: 对象ID列表

        Returns:
            List[Any]: 反序列化后的对象列表
        """
        ...

    async def delete(self, object_id: str) -> bool:
        """
        删除对象

        Args:
            object_id: 对象ID

        Returns:
            bool: 是否成功删除
        """
        ...

    async def contains(self, object_id: str) -> bool:
        """
        检查对象是否存在

        Args:
            object_id: 对象ID

        Returns:
            bool: 对象是否存在
        """
        ...

    async def get_view(self, object_id: str) -> PyObjectView:
        """
        获取对象视图

        Args:
            object_id: 对象ID

        Returns:
            PyObjectView: 对象视图
        """
        ...

    def stats(self) -> Dict[str, Any]:
        """
        获取对象存储统计信息

        Returns:
            Dict[str, Any]: 包含统计信息的字典
        """
        ...

    async def cleanup(self) -> None:
        """清理对象存储（LRU策略）"""
        ...

    async def clear(self) -> None:
        """清空对象存储"""
        ...

    def print_stats(self) -> None:
        """打印对象存储统计信息"""
        ...

    async def put_smart(self, obj: Any, size_threshold: int = 1048576) -> str:
        """
        智能存储对象（基于大小阈值）

        Args:
            obj: 要存储的对象
            size_threshold: 大小阈值（字节）

        Returns:
            str: 对象ID
        """
        ...

    async def get_storage_info(self, object_id: str) -> Dict[str, Any]:
        """
        获取对象存储信息

        Args:
            object_id: 对象ID

        Returns:
            Dict[str, Any]: 包含存储信息的字典
        """
        ...

    def analyze_memory_usage(self) -> Dict[str, Any]:
        """
        分析内存使用情况

        Returns:
            Dict[str, Any]: 包含内存分析信息的字典
        """
        ...

    async def get_zero_copy_stats(self) -> Dict[str, Any]:
        """
        获取零拷贝统计信息

        Returns:
            Dict[str, Any]: 包含零拷贝统计信息的字典
        """
        ...

    def get_memory_pressure_info(self) -> Dict[str, Any]:
        """
        获取内存压力信息

        Returns:
            Dict[str, Any]: 包含内存压力信息的字典
        """
        ...

    def __repr__(self) -> str:
        """返回对象存储的字符串表示"""
        ...

# ============================================================================
# Actor System Related Type Definitions
# ============================================================================

class PyActorSystem:
    """
    Python Actor System

    Manages Actor lifecycle, message passing and distributed communication.
    """

    def __new__(
        cls,
        system_name: str,
        listen_address: str,
        worker_threads: Optional[int] = None,
        store_config: Optional[PyStoreConfig] = None,
    ) -> Self:
        """
        Create Actor system instance

        Args:
            system_name: System name
            listen_address: Listen address
            worker_threads: Number of worker threads
            store_config: Object store configuration
        """
        ...

    async def start(self) -> None:
        """Start Actor system"""
        ...

    async def shutdown(self) -> None:
        """Shutdown Actor system"""
        ...

    async def spawn_from_construction_task(
        self, task_dict: Dict[str, Any], path: str
    ) -> "PyActorRef":
        """
        Create Actor from construction task

        Args:
            task_dict: Construction task dictionary
            path: Actor path

        Returns:
            PyActorRef: Actor reference
        """
        ...

    async def spawn(
        self,
        actor_class: Any,
        path: str,
        args: Optional[Tuple] = None,
        kwargs: Optional[Dict[str, Any]] = None,
    ) -> "PyActorRef":
        """
        Create Actor

        Args:
            actor_class: Actor class
            path: Actor path
            args: Constructor arguments
            kwargs: Constructor keyword arguments

        Returns:
            PyActorRef: Actor reference
        """
        ...

    async def actor_of(self, path: str) -> "PyActorRef":
        """
        Get Actor reference

        Args:
            path: Actor path

        Returns:
            PyActorRef: Actor reference
        """
        ...

    async def connect_to_node(self, node_id: str, address: str) -> None:
        """
        Connect to remote node

        Args:
            node_id: Node ID
            address: Node address
        """
        ...

    def get_store(self) -> PyObjectStore:
        """
        Get object store

        Returns:
            PyObjectStore: Object store instance
        """
        ...

class PyActorRef:
    """
    Python Actor Reference

    Represents a reference to an Actor, providing message sending and query functionality.
    """

    async def tell(self, message: Any) -> None:
        """
        Send message (no response)

        Args:
            message: Message content
        """
        ...

    async def ask(self, message: Any, timeout_ms: Optional[int] = None) -> Any:
        """
        Send message and wait for response

        Args:
            message: Message content
            timeout_ms: Timeout (milliseconds)

        Returns:
            Any: Response content
        """
        ...

    def stop(self) -> None:
        """Stop Actor"""
        ...

    @property
    def path(self) -> str:
        """Get Actor path"""
        ...

    async def async_ping(self) -> bool:
        """Async ping test"""
        ...

    async def async_ping_with_str(self, s: str) -> str:
        """
        Async ping test with string

        Args:
            s: Test string

        Returns:
            str: Response string
        """
        ...

class PyActorContext:
    """
    Python Actor Context

    Provides runtime context information for Actors, including self-reference, system access, etc.
    """

    @property
    def actor_id(self) -> str:
        """Get Actor ID"""
        ...

    async def tell_self(self, message: Any) -> None:
        """
        Send message to self

        Args:
            message: Message content
        """
        ...

    @property
    def self_ref(self) -> PyActorRef:
        """Get self reference"""
        ...

    def spawn(
        self,
        actor_class: Any,
        path: str,
        args: Optional[Tuple] = None,
        kwargs: Optional[Dict[str, Any]] = None,
    ) -> Awaitable[PyActorRef]:
        """
        Create child Actor

        Args:
            actor_class: Actor class
            path: Actor path
            args: Constructor arguments
            kwargs: Constructor keyword arguments

        Returns:
            Awaitable[PyActorRef]: Asynchronously returned Actor reference
        """
        ...

    def actor_of(self, path: str) -> Awaitable[PyActorRef]:
        """
        Get Actor reference

        Args:
            path: Actor path

        Returns:
            Awaitable[PyActorRef]: Asynchronously returned Actor reference
        """
        ...

    def spawn_from_construction_task(
        self, task_dict: Dict[str, Any], path: str
    ) -> Awaitable[PyActorRef]:
        """
        Create Actor from construction task

        Args:
            task_dict: Construction task dictionary
            path: Actor path

        Returns:
            Awaitable[PyActorRef]: Asynchronously returned Actor reference
        """
        ...

    def get_store(self) -> PyObjectStore:
        """
        Get object store

        Returns:
            PyObjectStore: Object store instance
        """
        ...

# ============================================================================
# Type Aliases and Convenience Types
# ============================================================================

# 类型别名，提供更友好的名称
ActorSystem = PyActorSystem
ActorRef = PyActorRef
ActorContext = PyActorContext
ObjectStore = PyObjectStore
ObjectRef = PyObjectRef
ObjectView = PyObjectView
StoreConfig = PyStoreConfig
CopyMode = PyCopyMode

# ============================================================================
# Async Type Definitions
# ============================================================================

T = TypeVar("T")

class AsyncResult(Generic[T]):
    """
    异步结果包装器

    用于包装异步操作的结果，提供类型安全。
    """

    def __await__(self) -> T:
        """
        等待异步结果

        Returns:
            T: 结果值
        """
        ...

# ============================================================================
# Exception Types
# ============================================================================

class ActorSystemError(Exception):
    """Actor system exception"""

    ...

class ObjectStoreError(Exception):
    """Object store exception"""

    ...

class ActorNotFoundError(Exception):
    """Actor not found exception"""

    ...

class TimeoutError(Exception):
    """Timeout exception"""

    ...

class SerializationError(Exception):
    """Serialization exception"""

    ...

class MemoryError(Exception):
    """Memory exception"""

    ...

# ============================================================================
# Utility Function Type Definitions
# ============================================================================

def get_global_object_store() -> Optional[PyObjectStore]:
    """
    Get global object store

    Returns:
        Optional[PyObjectStore]: Global object store instance
    """
    ...

def set_global_object_store(store: PyObjectStore) -> None:
    """
    Set global object store

    Args:
        store: Object store instance
    """
    ...

# ============================================================================
# Decorator Type Definitions
# ============================================================================

def actor(cls: type) -> type:
    """
    Actor decorator

    Used to mark Python classes as Actors, automatically adding necessary Actor methods.

    Args:
        cls: Class to decorate

    Returns:
        type: Decorated class
    """
    ...

def on(event_type: str) -> Callable:
    """
    Event handler decorator

    Used to mark methods as handlers for specific event types.

    Args:
        event_type: Event type

    Returns:
        Callable: Decorator function
    """
    ...

# ============================================================================
# Type Hint Utilities
# ============================================================================

AnyMessage = TypeVar("AnyMessage")
AnyResponse = TypeVar("AnyResponse")

MessageHandler = Callable[[AnyMessage, "ActorContext"], Awaitable[AnyResponse]]
EventHandler = Callable[[AnyMessage, "ActorContext"], Awaitable[None]]

# ============================================================================
# Serialization Related
# ============================================================================

class Serializable:
    """
    Serializable interface

    Classes implementing this interface can be serialized and passed between Actors.
    """

    def serialize(self) -> bytes:
        """
        Serialize object

        Returns:
            bytes: Serialized byte data
        """
        ...

    @classmethod
    def deserialize(cls, data: bytes) -> "Serializable":
        """
        Deserialize object

        Args:
            data: Serialized data

        Returns:
            Serializable: Deserialized object
        """
        ...

# ============================================================================
# Lifecycle Hooks
# ============================================================================

class ActorLifecycle:
    """
    Actor lifecycle hooks

    Defines lifecycle methods for Actors, subclasses can override these methods.
    """

    async def on_start(self, ctx: "ActorContext") -> None:
        """
        Called when Actor starts

        Args:
            ctx: Actor context
        """
        ...

    async def on_stop(self, ctx: "ActorContext") -> None:
        """
        Called when Actor stops

        Args:
            ctx: Actor context
        """
        ...

    async def on_message(self, message: Any, ctx: "ActorContext") -> None:
        """
        Called when message is received

        Args:
            message: Message content
            ctx: Actor context
        """
        ...

    async def handle_message(self, message: Any, ctx: "ActorContext") -> Any:
        """
        Handle message and return response

        Args:
            message: Message content
            ctx: Actor context

        Returns:
            Any: Response content
        """
        ...

    async def get_state(self) -> Any:
        """
        Get Actor state

        Returns:
            Any: Actor state
        """
        ...

    async def set_state(self, state: Any) -> None:
        """
        Set Actor state

        Args:
            state: New state
        """
        ...
