"""
Distributed Queue implementation for Lyricore Actor System
Similar to Ray's Queue utility but built on the Lyricore Actor framework
"""

import asyncio
import logging
import uuid
from collections import deque
from dataclasses import dataclass, field
from typing import Any, Dict, List, Optional, Set, Tuple

from .py_actor import ActorContext, actor

logger = logging.getLogger(__name__)


# ============================================================================
# Queue Configuration
# ============================================================================


@dataclass
class QueueConfig:
    """Configuration for distributed queue."""

    max_size: int = 0  # 0 means unlimited
    enable_persistence: bool = False
    batch_timeout: float = 0.1  # timeout for batching operations
    max_batch_size: int = 100


# ============================================================================
# Queue Messages
# ============================================================================


@dataclass
class QueueMessage:
    """Base class for queue messages."""

    message_id: str = field(default_factory=lambda: str(uuid.uuid4()))


@dataclass
class PutMessage(QueueMessage):
    """Message to put an item into the queue."""

    item: Any = None
    block: bool = True
    task_id: Optional[str] = None


@dataclass
class GetMessage(QueueMessage):
    """Message to get an item from the queue."""

    block: bool = True
    timeout: Optional[float] = None
    task_id: Optional[str] = None


@dataclass
class PutBatchMessage(QueueMessage):
    """Message to put multiple items into the queue."""

    items: List[Any] = field(default_factory=list)
    block: bool = True
    task_id: Optional[str] = None


@dataclass
class GetBatchMessage(QueueMessage):
    """Message to get multiple items from the queue."""

    num_items: int = 1
    block: bool = True
    timeout: Optional[float] = None
    task_id: Optional[str] = None


@dataclass
class SizeMessage(QueueMessage):
    """Message to get queue size."""

    task_id: Optional[str] = None


@dataclass
class EmptyMessage(QueueMessage):
    """Message to check if queue is empty."""

    task_id: Optional[str] = None


@dataclass
class FullMessage(QueueMessage):
    """Message to check if queue is full."""

    task_id: Optional[str] = None


@dataclass
class ClearMessage(QueueMessage):
    """Message to clear the queue."""

    task_id: Optional[str] = None


@dataclass
class BlockingPutMessage(QueueMessage):
    """Message for blocking put operation."""

    items: List[Any] = field(default_factory=list)
    callback_actor_path: str = ""
    timeout: Optional[float] = None
    task_id: Optional[str] = None


@dataclass
class BlockingGetMessage(QueueMessage):
    """Message for blocking get operation."""

    num_items: int = 1
    callback_actor_path: str = ""
    timeout: Optional[float] = None
    task_id: Optional[str] = None


# ============================================================================
# Queue Response Messages
# ============================================================================


@dataclass
class QueueResponse:
    """Base response from queue operations."""

    success: bool
    message_id: str
    error: Optional[str] = None
    task_id: Optional[str] = None


@dataclass
class PutResponse(QueueResponse):
    """Response for put operations."""

    pass


@dataclass
class GetResponse(QueueResponse):
    """Response for get operations."""

    item: Any = None


@dataclass
class BatchGetResponse(QueueResponse):
    """Response for batch get operations."""

    items: List[Any] = field(default_factory=list)


@dataclass
class SizeResponse(QueueResponse):
    """Response for size queries."""

    size: int = 0


@dataclass
class StatusResponse(QueueResponse):
    """Response for status queries (empty/full)."""

    status: bool = False


# ============================================================================
# Business Method Return Types
# ============================================================================


@dataclass
class PutResult:
    """Result type for put operations."""

    success: bool
    error: Optional[str] = None
    task_id: Optional[str] = None


@dataclass
class GetResult:
    """Result type for get operations."""

    success: bool
    item: Any = None
    error: Optional[str] = None
    task_id: Optional[str] = None


@dataclass
class GetBatchResult:
    """Result type for batch get operations."""

    success: bool
    items: List[Any] = field(default_factory=list)
    error: Optional[str] = None
    task_id: Optional[str] = None


@dataclass
class PutBatchResult:
    """Result type for batch put operations."""

    success: bool
    error: Optional[str] = None
    task_id: Optional[str] = None


@dataclass
class SizeResult:
    """Result type for size operations."""

    size: int
    error: Optional[str] = None
    task_id: Optional[str] = None


@dataclass
class EmptyResult:
    """Result type for empty operations."""

    empty: bool
    error: Optional[str] = None
    task_id: Optional[str] = None


@dataclass
class FullResult:
    """Result type for full operations."""

    full: bool
    error: Optional[str] = None
    task_id: Optional[str] = None


@dataclass
class ClearResult:
    """Result type for clear operations."""

    success: bool
    error: Optional[str] = None
    task_id: Optional[str] = None


# ============================================================================
# Queue Actor Implementation
# ============================================================================


@actor
class QueueActor:
    """
    Distributed queue actor that manages queue state and operations.
    This actor handles all queue operations in a thread-safe manner.
    """

    def __init__(self, config: QueueConfig = None):
        self.config = config or QueueConfig()
        self._queue: deque = deque()
        self._size_limit = self.config.max_size
        self._closed = False
        self._ctx = None
        # For blocking operations
        self._put_waiters: List[
            Tuple[str, List[Any], Optional[float]]
        ] = []  # (callback_path, items, deadline)
        self._get_waiters: List[
            Tuple[str, int, Optional[float]]
        ] = []  # (callback_path, num_items, deadline)

        logger.info(f"QueueActor initialized with config: {self.config}")

    async def on_start(self, ctx: ActorContext):
        """Called when the actor starts."""
        logger.info(f"QueueActor started at {ctx.actor_id}")
        self._ctx = ctx
        # Start timeout checker task
        asyncio.create_task(self._timeout_checker())

    async def on_stop(self, ctx: ActorContext):
        """Called when the actor stops."""
        self._closed = True
        logger.info(f"QueueActor stopped at {ctx.actor_id}")

    # Business methods for direct method calls via proxy
    async def put_item(
        self,
        item: Any,
        block: bool = True,
        ctx: ActorContext = None,
        task_id: str = None,
    ) -> PutResult:
        """Put an item into the queue - business method."""
        if self._closed:
            return PutResult(success=False, error="Queue is closed", task_id=task_id)

        # Check if queue is full
        if self._is_full():
            if not block:
                return PutResult(success=False, error="Queue is full", task_id=task_id)
            else:
                # For blocking operations, return False to indicate blocking is needed
                return PutResult(
                    success=False, error="Blocking operation needed", task_id=task_id
                )

        # Add item to queue
        self._queue.append(item)
        # Try to satisfy waiting getters
        await self._try_satisfy_waiters()
        return PutResult(success=True, task_id=task_id)

    async def get_item(
        self, block: bool = True, ctx: ActorContext = None, task_id: str = None
    ) -> GetResult:
        """Get an item from the queue - business method."""
        if self._closed and len(self._queue) == 0:
            return GetResult(
                success=False, error="Queue is closed and empty", task_id=task_id
            )

        # Check if queue has items
        if len(self._queue) == 0:
            if not block:
                return GetResult(success=False, error="Queue is empty", task_id=task_id)
            else:
                # For blocking operations, return False to indicate blocking is needed
                return GetResult(
                    success=False, error="Blocking operation needed", task_id=task_id
                )

        # Get item from queue
        item = self._queue.popleft()
        # Try to satisfy waiting putters
        await self._try_satisfy_waiters()
        return GetResult(success=True, item=item, task_id=task_id)

    async def put_batch_items(
        self,
        items: List[Any],
        block: bool = True,
        ctx: ActorContext = None,
        task_id: str = None,
    ) -> PutBatchResult:
        """Put multiple items into the queue - business method."""
        if self._closed:
            return PutBatchResult(
                success=False, error="Queue is closed", task_id=task_id
            )

        # Check available space
        available_space = self._available_space()
        if available_space < len(items):
            if not block:
                return PutBatchResult(
                    success=False, error="Not enough space", task_id=task_id
                )
            else:
                # For blocking operations, return False to indicate blocking is needed
                return PutBatchResult(
                    success=False, error="Blocking operation needed", task_id=task_id
                )

        # Add items to queue
        for item in items:
            self._queue.append(item)
        # Try to satisfy waiting getters
        await self._try_satisfy_waiters()
        return PutBatchResult(success=True, task_id=task_id)

    async def get_batch_items(
        self,
        num_items: int,
        block: bool = True,
        ctx: ActorContext = None,
        task_id: str = None,
    ) -> GetBatchResult:
        """Get multiple items from the queue - business method."""
        if self._closed and len(self._queue) == 0:
            return GetBatchResult(
                success=False,
                items=[],
                error="Queue is closed and empty",
                task_id=task_id,
            )

        available_items = len(self._queue)
        requested_items = num_items

        if available_items < requested_items:
            if not block:
                # Return available items
                items = []
                for _ in range(min(available_items, requested_items)):
                    if self._queue:
                        items.append(self._queue.popleft())
                return GetBatchResult(success=True, items=items, task_id=task_id)
            else:
                # For blocking operations, return False to indicate blocking is needed
                return GetBatchResult(
                    success=False,
                    items=[],
                    error="Blocking operation needed",
                    task_id=task_id,
                )

        # Get items from queue
        items = []
        for _ in range(requested_items):
            if self._queue:
                items.append(self._queue.popleft())
        # Try to satisfy waiting putters
        await self._try_satisfy_waiters()
        return GetBatchResult(success=True, items=items, task_id=task_id)

    async def get_size(self, ctx: ActorContext = None, task_id: str = None) -> int:
        """Get current queue size - business method."""
        return len(self._queue)

    async def is_empty(self, ctx: ActorContext = None, task_id: str = None) -> bool:
        """Check if queue is empty - business method."""
        return len(self._queue) == 0

    async def is_full(self, ctx: ActorContext = None, task_id: str = None) -> bool:
        """Check if queue is full - business method."""
        return self._is_full()

    async def clear_queue(
        self, ctx: ActorContext = None, task_id: str = None
    ) -> ClearResult:
        """Clear the queue - business method."""
        self._queue.clear()
        return ClearResult(success=True, task_id=task_id)

    async def blocking_put(
        self, message: BlockingPutMessage, ctx: ActorContext = None
    ) -> None:
        """Handle blocking put operation."""
        # Calculate deadline if timeout specified
        deadline = None
        if message.timeout is not None:
            deadline = asyncio.get_event_loop().time() + message.timeout

        # Try to put immediately - check if we have enough space
        required_space = len(message.items)
        available_space = self._available_space()

        if available_space >= required_space:
            self._queue.extend(message.items)
            # Notify success via callback
            try:
                callback_ref = await self._ctx.actor_of(message.callback_actor_path)
                await callback_ref.notify(message.task_id, True)
            except Exception as e:
                logger.error(
                    f"Failed to notify callback {message.callback_actor_path}: {e}"
                )
            # After putting, try to satisfy waiters
            await self._try_satisfy_waiters()
            return

        # Not enough space, add to waiters with deadline
        self._put_waiters.append((message.callback_actor_path, message.items, deadline))

    async def blocking_get(
        self, message: BlockingGetMessage, ctx: ActorContext = None
    ) -> None:
        """Handle blocking get operation."""
        # Calculate deadline if timeout specified
        deadline = None
        if message.timeout is not None:
            deadline = asyncio.get_event_loop().time() + message.timeout

        # Try to get immediately
        if len(self._queue) >= message.num_items:
            items = []
            for _ in range(message.num_items):
                if self._queue:
                    items.append(self._queue.popleft())

            # Notify success via callback
            try:
                callback_ref = await self._ctx.actor_of(message.callback_actor_path)
                if message.num_items == 1:
                    await callback_ref.notify(message.task_id, True, item=items[0])
                else:
                    await callback_ref.notify(message.task_id, True, items=items)
            except Exception as e:
                logger.error(
                    f"Failed to notify callback {message.callback_actor_path}: {e}"
                )
            # After getting, try to satisfy waiters
            await self._try_satisfy_waiters()
            return

        # Not enough items, add to waiters with deadline
        self._get_waiters.append(
            (message.callback_actor_path, message.num_items, deadline)
        )

    async def _try_satisfy_waiters(self):
        """Try to satisfy waiting putters and getters."""
        current_time = asyncio.get_event_loop().time()

        # Check for expired timeouts first
        # Remove expired put waiters and notify them of timeout
        active_put_waiters = []
        for callback_path, items, deadline in self._put_waiters:
            if deadline is not None and deadline <= current_time:
                # Timeout expired
                try:
                    callback_ref = await self._ctx.actor_of(callback_path)
                    await callback_ref.notify(
                        None, False, error=QueueTimeoutError("Timeout")
                    )
                except Exception as e:
                    logger.error(f"Failed to notify callback {callback_path}: {e}")
            else:
                active_put_waiters.append((callback_path, items, deadline))
        self._put_waiters = active_put_waiters

        # Remove expired get waiters and notify them of timeout
        active_get_waiters = []
        for callback_path, num_items, deadline in self._get_waiters:
            if deadline is not None and deadline <= current_time:
                # Timeout expired
                try:
                    callback_ref = await self._ctx.actor_of(callback_path)
                    await callback_ref.notify(
                        None, False, error=QueueTimeoutError("Timeout")
                    )
                except Exception as e:
                    logger.error(f"Failed to notify callback {callback_path}: {e}")
            else:
                active_get_waiters.append((callback_path, num_items, deadline))
        self._get_waiters = active_get_waiters

        # Keep track of whether we made progress
        made_progress = True

        # Alternate between satisfying getters and putters to prevent starvation
        while made_progress:
            made_progress = False

            # Satisfy getters first (if queue has items)
            if self._queue and self._get_waiters:
                callback_path, num_items, _ = self._get_waiters[0]
                if len(self._queue) >= num_items:
                    # Remove from waiters
                    self._get_waiters.pop(0)

                    # Get items
                    items = []
                    for _ in range(num_items):
                        if self._queue:
                            items.append(self._queue.popleft())

                    # Notify getter
                    try:
                        callback_ref = await self._ctx.actor_of(callback_path)
                        if num_items == 1:
                            await callback_ref.notify(None, True, item=items[0])
                        else:
                            await callback_ref.notify(None, True, items=items)
                    except Exception as e:
                        logger.error(f"Failed to notify callback {callback_path}: {e}")

                    made_progress = True

            # Then satisfy putters (if queue has space)
            if not self._is_full() and self._put_waiters:
                callback_path, items, _ = self._put_waiters[0]
                required_space = len(items)
                available_space = self._available_space()

                if available_space >= required_space:
                    # Remove from waiters
                    self._put_waiters.pop(0)

                    # Put items
                    self._queue.extend(items)

                    # Notify putter
                    try:
                        callback_ref = await self._ctx.actor_of(callback_path)
                        await callback_ref.notify(None, True)
                    except Exception as e:
                        logger.error(f"Failed to notify callback {callback_path}: {e}")

                    made_progress = True

    async def _timeout_checker(self):
        """Periodically check for expired timeouts."""
        while not self._closed:
            try:
                # Check for expired timeouts
                await self._try_satisfy_waiters()
                # Sleep for a short interval
                await asyncio.sleep(0.1)  # Check every 100ms
            except Exception as e:
                logger.error(f"Error in timeout checker: {e}")
                break

    # Traditional message handlers for backward compatibility
    async def put_message(
        self, message: PutMessage, ctx: ActorContext = None
    ) -> PutResult:
        """Handle put message - traditional message handler."""
        success = await self.put_item(message.item, message.block, ctx, message.task_id)
        return PutResult(success=success, task_id=message.task_id)

    async def get_message(
        self, message: GetMessage, ctx: ActorContext = None
    ) -> GetResult:
        """Handle get message - traditional message handler."""
        success, item = await self.get_item(message.block, ctx, message.task_id)
        return GetResult(success=success, item=item, task_id=message.task_id)

    async def put_batch_message(
        self, message: PutBatchMessage, ctx: ActorContext = None
    ) -> PutBatchResult:
        """Handle put batch message - traditional message handler."""
        success = await self.put_batch_items(
            message.items, message.block, ctx, message.task_id
        )
        return PutBatchResult(success=success, task_id=message.task_id)

    async def get_batch_message(
        self, message: GetBatchMessage, ctx: ActorContext = None
    ) -> GetBatchResult:
        """Handle get batch message - traditional message handler."""
        success, items = await self.get_batch_items(
            message.num_items, message.block, ctx, message.task_id
        )
        return GetBatchResult(success=success, items=items, task_id=message.task_id)

    async def size_message(
        self, message: SizeMessage, ctx: ActorContext = None
    ) -> SizeResult:
        """Handle size message - traditional message handler."""
        size = len(self._queue)
        return SizeResult(size=size, task_id=message.task_id)

    async def empty_message(
        self, message: EmptyMessage, ctx: ActorContext = None
    ) -> EmptyResult:
        """Handle empty message - traditional message handler."""
        is_empty = len(self._queue) == 0
        return EmptyResult(empty=is_empty, task_id=message.task_id)

    async def full_message(
        self, message: FullMessage, ctx: ActorContext = None
    ) -> FullResult:
        """Handle full message - traditional message handler."""
        is_full = self._is_full()
        return FullResult(full=is_full, task_id=message.task_id)

    async def clear_message(
        self, message: ClearMessage, ctx: ActorContext = None
    ) -> ClearResult:
        """Handle clear message - traditional message handler."""
        self._queue.clear()
        return ClearResult(success=True, task_id=message.task_id)

    # Helper methods
    def _is_full(self) -> bool:
        """Check if queue is full."""
        if self._size_limit <= 0:
            return False
        return len(self._queue) >= self._size_limit

    def _available_space(self) -> int:
        """Get available space in queue."""
        if self._size_limit <= 0:
            return float("inf")
        return max(0, self._size_limit - len(self._queue))


# ============================================================================
# Queue Client Interface
# ============================================================================


class Queue:
    """
    Distributed Queue client interface.
    This provides a high-level interface similar to Ray's Queue.
    """

    _TASK_FUTURE_MAP: Dict[str, asyncio.Future] = {}

    def __init__(
        self,
        actor_system,
        queue_name: Optional[str] = None,
        max_size: int = 0,
        path_prefix: str = "/user/queues",
    ):
        """
        Initialize a distributed queue.

        Args:
            actor_system: The Lyricore actor system
            queue_name: Name of the queue (auto-generated if None)
            max_size: Maximum queue size (0 for unlimited)
            path_prefix: Path prefix for queue actors
        """
        self._system = actor_system
        self._queue_name = queue_name or f"queue_{uuid.uuid4().hex[:8]}"
        self._queue_path = f"{path_prefix}/{self._queue_name}"
        self._config = QueueConfig(max_size=max_size)
        self._queue_ref = None
        self._callback_actor_ref = None
        self._callback_actor_path = None
        self._initialized = False
        self._client_id = f"{self._queue_name}_{uuid.uuid4().hex[:8]}"
        # Task ID counter for generating unique task IDs
        self._task_counter = 0

    def __getstate__(self):
        if not self._initialized:
            raise RuntimeError("Queue must be initialized before serialization")
        state = self.__dict__.copy()
        state.pop("_system")
        return state

    def __setstate__(self, state):
        self.__dict__.update(state)

    def _generate_task_id(self) -> str:
        """Generate a unique task ID for this queue operation."""
        self._task_counter += 1
        return f"{self._queue_name}_task_{self._task_counter}_{uuid.uuid4().hex[:8]}"

    async def _ensure_initialized(self):
        """Ensure the queue actor is initialized."""
        if not self._initialized:
            try:
                # Try to get existing queue actor
                self._queue_ref = await self._system.spawn(
                    QueueActor, self._queue_path, self._config
                )
                await self._initialize_callback_actor()
            except Exception as e:
                raise QueueError(f"Failed to get existing queue actor: {e}")

            self._initialized = True

    async def _initialize_callback_actor(self) -> None:
        """Initialize the callback actor for handling async responses."""

        @actor
        class InnerQueueCallbackActor:
            async def notify(
                self,
                task_id: str,
                success: bool,
                item: Any = None,
                items: List[Any] = None,
                error: Exception = None,
            ):
                """Handle notification from queue actor."""
                if task_id in Queue._TASK_FUTURE_MAP:
                    future = Queue._TASK_FUTURE_MAP[task_id]
                    if not future.done():
                        if success:
                            if item is not None:
                                future.set_result(item)
                            elif items is not None:
                                future.set_result(items)
                            else:
                                future.set_result(True)
                        else:
                            future.set_exception(error or QueueError("Unknown error"))
                    # Clean up the future from map
                    del Queue._TASK_FUTURE_MAP[task_id]

        if self._callback_actor_ref is None:
            callback_actor_name = f"{self._client_id}_callback"
            callback_actor_path = f"/user/{callback_actor_name}"
            try:
                # self._callback_actor_ref = await self._system.actor_of(callback_actor_path)
                self._callback_actor_ref = await self._system.spawn(
                    InnerQueueCallbackActor, callback_actor_path
                )
                self._callback_actor_path = self._callback_actor_ref.path
            except Exception as e:
                raise QueueError(
                    f"Failed to get existing callback actor at {callback_actor_path}, error: {e}"
                )

    async def put(
        self, item: Any, block: bool = True, timeout: Optional[float] = None
    ) -> None:
        """
        Put an item into the queue.

        Args:
            item: Item to put into the queue
            block: Whether to block if queue is full
            timeout: Timeout for blocking operations

        Raises:
            QueueFullError: If queue is full and block=False
            QueueTimeoutError: If timeout occurs
        """
        await self._ensure_initialized()

        # Generate task ID for internal tracking
        task_id = self._generate_task_id()

        # For non-blocking operations, try directly first
        if not block:
            result = await self._queue_ref.put_item.ask(item, False, task_id=task_id)
            if not result.success:
                raise QueueFullError("Queue is full")
            return

        # For blocking operations, use future-based approach
        loop = asyncio.get_event_loop()
        future = loop.create_future()
        Queue._TASK_FUTURE_MAP[task_id] = future

        try:
            # Try immediate put first
            result = await self._queue_ref.put_item.ask(item, False, task_id=task_id)
            if result.success:
                return

            # If that fails, use blocking put
            await self._queue_ref.blocking_put.tell(
                BlockingPutMessage(
                    items=[item],
                    callback_actor_path=self._callback_actor_path,
                    timeout=timeout,
                    task_id=task_id,
                )
            )

            # Wait for the future to complete
            if timeout is not None:
                await asyncio.wait_for(future, timeout)
            else:
                await future

            return task_id
        except asyncio.TimeoutError:
            if task_id in Queue._TASK_FUTURE_MAP:
                del Queue._TASK_FUTURE_MAP[task_id]
            raise QueueTimeoutError("Put operation timed out")
        except Exception:
            if task_id in Queue._TASK_FUTURE_MAP:
                del Queue._TASK_FUTURE_MAP[task_id]
            raise

    async def put_batch(
        self, items: List[Any], block: bool = True, timeout: Optional[float] = None
    ) -> None:
        """
        Put multiple items into the queue.

        Args:
            items: Items to put into the queue
            block: Whether to block if queue doesn't have enough space
            timeout: Timeout for blocking operations

        Raises:
            QueueFullError: If queue doesn't have enough space and block=False
            QueueTimeoutError: If timeout occurs
        """
        await self._ensure_initialized()

        # Generate task ID for internal tracking
        task_id = self._generate_task_id()

        # For non-blocking operations, try directly first
        if not block:
            result = await self._queue_ref.put_batch_items.ask(
                items, False, task_id=task_id
            )
            if not result.success:
                raise QueueFullError("Not enough space in queue")
            return

        # For blocking operations, use future-based approach
        loop = asyncio.get_event_loop()
        future = loop.create_future()
        Queue._TASK_FUTURE_MAP[task_id] = future

        try:
            # Try immediate put first
            result = await self._queue_ref.put_batch_items.ask(
                items, False, task_id=task_id
            )
            if result.success:
                return

            # If that fails, use blocking put
            await self._queue_ref.blocking_put.tell(
                BlockingPutMessage(
                    items=items,
                    callback_actor_path=self._callback_actor_path,
                    timeout=timeout,
                    task_id=task_id,
                )
            )

            # Wait for the future to complete
            if timeout is not None:
                await asyncio.wait_for(future, timeout)
            else:
                await future

            return task_id
        except asyncio.TimeoutError:
            if task_id in Queue._TASK_FUTURE_MAP:
                del Queue._TASK_FUTURE_MAP[task_id]
            raise QueueTimeoutError("Put operation timed out")
        except Exception:
            if task_id in Queue._TASK_FUTURE_MAP:
                del Queue._TASK_FUTURE_MAP[task_id]
            raise

    async def get(self, block: bool = True, timeout: Optional[float] = None) -> Any:
        """
        Get an item from the queue.

        Args:
            block: Whether to block if queue is empty
            timeout: Timeout for blocking operations

        Returns:
            The item from the queue

        Raises:
            QueueEmptyError: If queue is empty and block=False
            QueueTimeoutError: If timeout occurs
        """
        await self._ensure_initialized()

        # Generate task ID for internal tracking
        task_id = self._generate_task_id()

        # For non-blocking operations, try directly first
        if not block:
            result = await self._queue_ref.get_item.ask(False, task_id=task_id)
            if not result.success:
                raise QueueEmptyError("Queue is empty")
            return result.item

        # For blocking operations, use future-based approach
        loop = asyncio.get_event_loop()
        future = loop.create_future()
        Queue._TASK_FUTURE_MAP[task_id] = future

        try:
            # Try immediate get first
            result = await self._queue_ref.get_item.ask(False, task_id=task_id)
            if result.success:
                return result.item

            # If that fails, use blocking get
            await self._queue_ref.blocking_get.tell(
                BlockingGetMessage(
                    num_items=1,
                    callback_actor_path=self._callback_actor_path,
                    timeout=timeout,
                    task_id=task_id,
                )
            )

            # Wait for the future to complete
            if timeout is not None:
                item = await asyncio.wait_for(future, timeout)
            else:
                item = await future

            return item
        except asyncio.TimeoutError:
            if task_id in Queue._TASK_FUTURE_MAP:
                del Queue._TASK_FUTURE_MAP[task_id]
            raise QueueTimeoutError("Get operation timed out")
        except Exception:
            if task_id in Queue._TASK_FUTURE_MAP:
                del Queue._TASK_FUTURE_MAP[task_id]
            raise

    async def get_batch(
        self, num_items: int, block: bool = True, timeout: Optional[float] = None
    ) -> List[Any]:
        """
        Get multiple items from the queue.

        Args:
            num_items: Number of items to get
            block: Whether to block if not enough items available
            timeout: Timeout for blocking operations

        Returns:
            List of items from the queue

        Raises:
            QueueEmptyError: If queue is empty and block=False
            QueueTimeoutError: If timeout occurs
        """
        await self._ensure_initialized()

        # Generate task ID for internal tracking
        task_id = self._generate_task_id()

        # For non-blocking operations, try directly first
        if not block:
            result = await self._queue_ref.get_batch_items.ask(
                num_items, False, task_id=task_id
            )
            return result.items

        # For blocking operations, use future-based approach
        loop = asyncio.get_event_loop()
        future = loop.create_future()
        Queue._TASK_FUTURE_MAP[task_id] = future

        try:
            # Try immediate get first
            result = await self._queue_ref.get_batch_items.ask(
                num_items, False, task_id=task_id
            )
            if result.success and len(result.items) == num_items:
                return result.items

            # If that fails, use blocking get
            await self._queue_ref.blocking_get.tell(
                BlockingGetMessage(
                    num_items=num_items,
                    callback_actor_path=self._callback_actor_path,
                    timeout=timeout,
                    task_id=task_id,
                )
            )

            # Wait for the future to complete
            if timeout is not None:
                items = await asyncio.wait_for(future, timeout)
            else:
                items = await future

            return items
        except asyncio.TimeoutError:
            if task_id in Queue._TASK_FUTURE_MAP:
                del Queue._TASK_FUTURE_MAP[task_id]
            raise QueueTimeoutError("Get operation timed out")
        except Exception:
            if task_id in Queue._TASK_FUTURE_MAP:
                del Queue._TASK_FUTURE_MAP[task_id]
            raise

    async def size(self) -> int:
        """Get current queue size."""
        await self._ensure_initialized()

        # Generate task ID for internal tracking
        task_id = self._generate_task_id()

        size = await self._queue_ref.get_size.ask(task_id=task_id)
        return size

    async def empty(self) -> bool:
        """Check if queue is empty."""
        await self._ensure_initialized()

        # Generate task ID for internal tracking
        task_id = self._generate_task_id()

        is_empty = await self._queue_ref.is_empty.ask(task_id=task_id)
        return is_empty

    async def full(self) -> bool:
        """Check if queue is full."""
        await self._ensure_initialized()

        # Generate task ID for internal tracking
        task_id = self._generate_task_id()

        is_full = await self._queue_ref.is_full.ask(task_id=task_id)
        return is_full

    async def clear(self) -> None:
        """Clear all items from the queue."""
        await self._ensure_initialized()

        # Generate task ID for internal tracking
        task_id = self._generate_task_id()

        success = await self._queue_ref.clear_queue.ask(task_id=task_id)
        if not success:
            raise QueueError("Failed to clear queue")

    @property
    def name(self) -> str:
        """Get queue name."""
        return self._queue_name

    @property
    def path(self) -> str:
        """Get queue actor path."""
        return self._queue_path


# ============================================================================
# Queue Exceptions
# ============================================================================


class QueueError(Exception):
    """Base exception for queue operations."""

    pass


class QueueFullError(QueueError):
    """Exception raised when queue is full."""

    pass


class QueueEmptyError(QueueError):
    """Exception raised when queue is empty."""

    pass


class QueueTimeoutError(QueueError):
    """Exception raised when queue operation times out."""

    pass


class QueueClosedError(QueueError):
    """Exception raised when queue is closed."""

    pass


# ============================================================================
# Factory Functions
# ============================================================================


async def create_queue(
    actor_system, name: Optional[str] = None, max_size: int = 0
) -> Queue:
    """
    Create a new distributed queue.

    Args:
        actor_system: The Lyricore actor system
        name: Queue name (auto-generated if None)
        max_size: Maximum queue size (0 for unlimited)

    Returns:
        Queue instance
    """
    queue = Queue(actor_system, name, max_size)
    await queue._ensure_initialized()
    return queue


def get_queue(actor_system, name: str) -> Queue:
    """
    Get reference to an existing queue.

    Args:
        actor_system: The Lyricore actor system
        name: Queue name

    Returns:
        Queue instance
    """
    return Queue(actor_system, name)
