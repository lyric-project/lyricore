"""
Comprehensive tests for the Lyricore distributed Queue implementation
Following the pattern of the Lyricore test examples
"""

import asyncio
import uuid

import pytest
import pytest_asyncio

from lyricore import ActorContext, ActorSystem, actor, on
from lyricore._queue import (
    QueueActor,
    QueueConfig,
    QueueEmptyError,
    QueueFullError,
    QueueTimeoutError,
    create_queue,
    get_queue,
)

# from lyricore.tests.conftest import actor_system

# ============================================================================
# Test Fixtures (based on conftest.py pattern)
# ============================================================================


@pytest_asyncio.fixture
async def actor_system():
    """Create and cleanup actor system for tests."""
    system_name = f"test_system_{uuid.uuid4().hex[:8]}"
    port = 50000 + hash(system_name) % 10000

    system = ActorSystem(system_name, f"127.0.0.1:{port}")
    await system.start()

    yield system

    await system.shutdown()


@pytest_asyncio.fixture
async def queue_actor_ref(actor_system):
    """Create a queue actor for testing."""
    config = QueueConfig(max_size=5)
    path = f"/user/test_queue_{uuid.uuid4().hex[:8]}"

    queue_ref = await actor_system.spawn(QueueActor, path, config)

    yield queue_ref

    await queue_ref.stop()


@pytest_asyncio.fixture
async def test_queue(actor_system):
    """Create a test queue client."""
    queue_name = f"test_queue_{uuid.uuid4().hex[:8]}"
    queue = await create_queue(actor_system, name=queue_name, max_size=5)

    yield queue


@pytest.mark.asyncio
async def test_create_actor_with_ref(actor_system):
    queue = await create_queue(actor_system, name="ref_test_queue", max_size=10)
    await queue.put("test_item")
    item = await queue.get()


# ============================================================================
# Unit Tests for Queue Actor (using method proxy calls)
# ============================================================================


class TestQueueActorMethods:
    """Test the QueueActor implementation using method proxy calls."""

    @pytest.mark.asyncio
    async def test_put_and_get_single_item(self, queue_actor_ref):
        """Test basic put and get operations using method calls."""
        # Put an item using method call
        put_result = await queue_actor_ref.put_item.ask("test_item", True)

        assert put_result.success is True

        # Get the item using method call
        get_result = await queue_actor_ref.get_item.ask(True)

        assert get_result.success is True
        assert get_result.item == "test_item"

    @pytest.mark.asyncio
    async def test_put_batch_and_get_batch(self, queue_actor_ref):
        """Test batch operations using method calls."""
        items = ["item1", "item2", "item3"]

        # Put batch using method call
        put_result = await queue_actor_ref.put_batch_items.ask(items, True)
        assert put_result.success is True

        # Get batch using method call
        get_result = await queue_actor_ref.get_batch_items.ask(3, True)
        assert get_result.success is True
        assert get_result.items == items

    @pytest.mark.asyncio
    async def test_queue_size(self, queue_actor_ref):
        """Test size tracking using method calls."""
        # Initially empty
        size = await queue_actor_ref.get_size.ask()
        assert size == 0

        # Put some items
        await queue_actor_ref.put_item.ask("item1", True)
        await queue_actor_ref.put_item.ask("item2", True)

        size = await queue_actor_ref.get_size.ask()
        assert size == 2

        # Get one item
        await queue_actor_ref.get_item.ask(True)

        size = await queue_actor_ref.get_size.ask()
        assert size == 1

    @pytest.mark.asyncio
    async def test_empty_and_full_status(self, queue_actor_ref):
        """Test empty and full status checks using method calls."""
        # Initially empty
        is_empty = await queue_actor_ref.is_empty.ask()
        assert is_empty is True

        is_full = await queue_actor_ref.is_full.ask()
        assert is_full is False

        # Fill the queue (max_size=5)
        for i in range(5):
            await queue_actor_ref.put_item.ask(f"item{i}", True)

        is_empty = await queue_actor_ref.is_empty.ask()
        assert is_empty is False

        is_full = await queue_actor_ref.is_full.ask()
        assert is_full is True

    @pytest.mark.asyncio
    async def test_clear_queue(self, queue_actor_ref):
        """Test clearing the queue using method calls."""
        # Put some items
        for i in range(3):
            await queue_actor_ref.put_item.ask(f"item{i}", True)

        # Verify items are there
        size = await queue_actor_ref.get_size.ask()
        assert size == 3

        # Clear the queue
        clear_result = await queue_actor_ref.clear_queue.ask()
        assert clear_result.success is True

        # Verify queue is empty
        size = await queue_actor_ref.get_size.ask()
        assert size == 0

        is_empty = await queue_actor_ref.is_empty.ask()
        assert is_empty is True

    @pytest.mark.asyncio
    async def test_non_blocking_operations(self, queue_actor_ref):
        """Test non-blocking put and get operations using method calls."""
        # Fill the queue
        for i in range(5):
            await queue_actor_ref.put_item.ask(f"item{i}", True)

        # Try non-blocking put on full queue
        put_result = await queue_actor_ref.put_item.ask("overflow", False)
        assert put_result.success is False
        assert "full" in put_result.error.lower()

        # Clear the queue
        await queue_actor_ref.clear_queue.ask()

        # Try non-blocking get on empty queue
        get_result = await queue_actor_ref.get_item.ask(False)
        assert get_result.success is False
        assert "empty" in get_result.error.lower()

    @pytest.mark.asyncio
    async def test_blocking_put_with_client(self, test_queue):
        """Test blocking put operation with client interface."""
        # Fill the queue to capacity (max_size=5)
        for i in range(5):
            await test_queue.put(f"item{i}")

        is_full = await test_queue.full()
        assert is_full is True

        # Start a task to block on put
        async def put_later():
            await asyncio.sleep(0.1)  # Small delay
            await test_queue.put("blocked_item", block=True)

        put_task = asyncio.create_task(put_later())

        # Remove one item to make space
        item = await test_queue.get()
        assert item == "item0"

        # Wait for the put to complete
        await put_task

        # Verify the item was added
        size = await test_queue.size()
        assert size == 5
        is_full = await test_queue.full()
        assert is_full is True

    @pytest.mark.asyncio
    async def test_blocking_get_with_client(self, test_queue):
        """Test blocking get operation with client interface."""
        # Queue is initially empty
        is_empty = await test_queue.empty()
        assert is_empty is True

        # Start a task to block on get
        async def get_later():
            await asyncio.sleep(0.1)  # Small delay
            item = await test_queue.get(block=True)
            return item

        get_task = asyncio.create_task(get_later())

        # Add an item after a small delay
        await asyncio.sleep(0.05)
        await test_queue.put("delayed_item")

        # Wait for the get to complete
        item = await get_task
        assert item == "delayed_item"

        # Queue should be empty again
        is_empty = await test_queue.empty()
        assert is_empty is True

    @pytest.mark.asyncio
    async def test_blocking_timeout(self, test_queue):
        """Test blocking operations with timeout."""
        # Fill the queue
        for i in range(5):
            await test_queue.put(f"item{i}")

        # Try to put with a short timeout - should timeout
        start_time = asyncio.get_event_loop().time()
        with pytest.raises(QueueTimeoutError):
            await test_queue.put("timeout_item", block=True, timeout=0.1)

        # Should have waited at least 0.1 seconds
        elapsed = asyncio.get_event_loop().time() - start_time
        assert elapsed >= 0.09  # Allow some margin for timing variations

        # Clear queue and test get timeout
        await test_queue.clear()

        start_time = asyncio.get_event_loop().time()
        with pytest.raises(QueueTimeoutError):
            await test_queue.get(block=True, timeout=0.1)

        elapsed = asyncio.get_event_loop().time() - start_time
        assert elapsed >= 0.09

    @pytest.mark.asyncio
    async def test_concurrent_blocking_operations(self, test_queue):
        """Test multiple concurrent blocking operations."""
        # Queue with small capacity
        assert test_queue._config.max_size == 5

        # Start multiple producers
        async def producer(id, num_items):
            for i in range(num_items):
                await test_queue.put(f"producer_{id}_item_{i}", block=True)

        # Start multiple consumers
        async def consumer(id, num_items):
            items = []
            for i in range(num_items):
                item = await test_queue.get(block=True)
                items.append(item)
            return items

        # Create tasks
        producers = [asyncio.create_task(producer(i, 3)) for i in range(3)]
        consumers = [asyncio.create_task(consumer(i, 3)) for i in range(3)]

        # Wait for all to complete
        await asyncio.gather(*producers, *consumers)

        # Queue should be empty
        is_empty = await test_queue.empty()
        assert is_empty is True

        # Verify all items were consumed
        all_items = []
        for consumer_task in consumers:
            all_items.extend(consumer_task.result())

        assert len(all_items) == 9  # 3 producers × 3 items each


# ============================================================================
# Integration Tests for Queue Client
# ============================================================================


class TestQueueClient:
    """Test the Queue client interface."""

    @pytest.mark.asyncio
    async def test_basic_put_get(self, test_queue):
        """Test basic put and get operations."""
        # Put an item
        await test_queue.put("test_item")

        # Check size
        size = await test_queue.size()
        assert size == 1

        # Get the item
        item = await test_queue.get()
        assert item == "test_item"

        # Check empty
        is_empty = await test_queue.empty()
        assert is_empty is True

    @pytest.mark.asyncio
    async def test_batch_operations(self, test_queue):
        """Test batch put and get operations."""
        items = ["batch1", "batch2", "batch3", "batch4"]

        # Put batch
        await test_queue.put_batch(items)

        size = await test_queue.size()
        assert size == len(items)

        # Get batch
        retrieved = await test_queue.get_batch(len(items))
        assert retrieved == items

        is_empty = await test_queue.empty()
        assert is_empty is True

    @pytest.mark.asyncio
    async def test_complex_data_types(self, test_queue):
        """Test with various data types."""
        test_data = [
            {"key": "value", "number": 42},
            [1, 2, 3, 4, 5],
            "simple string",
            42,
            3.14159,
            None,
            True,
            False,
        ]

        # Put all items (in smaller batches due to size limit)
        for item in test_data[:5]:  # First 5 items
            await test_queue.put(item)

        # Get first batch
        retrieved_data = []
        while not await test_queue.empty():
            item = await test_queue.get()
            retrieved_data.append(item)

        # Put remaining items
        for item in test_data[5:]:
            await test_queue.put(item)

        # Get remaining items
        while not await test_queue.empty():
            item = await test_queue.get()
            retrieved_data.append(item)

        assert retrieved_data == test_data

    @pytest.mark.asyncio
    async def test_queue_full_behavior(self, test_queue):
        """Test behavior when queue is full."""
        # Fill the queue (max_size=5)
        for i in range(5):
            await test_queue.put(f"item{i}")

        is_full = await test_queue.full()
        assert is_full is True

        # Try non-blocking put
        with pytest.raises(QueueFullError):
            await test_queue.put("overflow", block=False)

    @pytest.mark.asyncio
    async def test_queue_empty_behavior(self, test_queue):
        """Test behavior when queue is empty."""
        is_empty = await test_queue.empty()
        assert is_empty is True

        # Try non-blocking get
        with pytest.raises(QueueEmptyError):
            await test_queue.get(block=False)

    @pytest.mark.asyncio
    async def test_clear_operation(self, test_queue):
        """Test clearing the queue."""
        # Put some items
        items = ["clear1", "clear2", "clear3"]
        await test_queue.put_batch(items)

        size = await test_queue.size()
        assert size == 3

        # Clear the queue
        await test_queue.clear()

        size = await test_queue.size()
        assert size == 0
        is_empty = await test_queue.empty()
        assert is_empty is True

    @pytest.mark.asyncio
    async def test_queue_properties(self, test_queue):
        """Test queue properties."""
        assert test_queue.name.startswith("test_queue_")
        assert test_queue.path.startswith("/user/queues/test_queue_")


# ============================================================================
# Concurrency Tests
# ============================================================================


@actor
class ProducerActor:
    """Producer actor that puts items into a queue."""

    def __init__(self, queue_ref, num_items: int = 10, delay: float = 0.01):
        self.queue_ref = queue_ref
        self.num_items = num_items
        self.delay = delay

    async def start_producing(self, ctx: ActorContext):
        """Start producing items using method calls."""
        produced_items = []

        for i in range(self.num_items):
            item = f"producer_{ctx.actor_id}_item_{i}"

            # Use method call on the queue actor
            result = await self.queue_ref.put_item.ask(item, True)
            if result.success:
                produced_items.append(item)

            await asyncio.sleep(self.delay)

        return {
            "producer_id": ctx.actor_id,
            "produced_count": len(produced_items),
            "items": produced_items,
        }


@actor
class ConsumerActor:
    """Consumer actor that gets items from a queue."""

    def __init__(self, queue_ref, max_items: int = 10, delay: float = 0.01):
        self.queue_ref = queue_ref
        self.max_items = max_items
        self.delay = delay

    async def start_consuming(self, ctx: ActorContext):
        """Start consuming items using method calls."""
        consumed_items = []
        attempts = 0
        max_attempts = self.max_items * 2  # Prevent infinite loops

        while len(consumed_items) < self.max_items and attempts < max_attempts:
            attempts += 1

            # Use method call on the queue actor
            result = await self.queue_ref.get_item.ask(False)  # Non-blocking
            if result.success:
                consumed_items.append(result.item)
            else:
                await asyncio.sleep(self.delay)  # Wait a bit before trying again

        return {
            "consumer_id": ctx.actor_id,
            "consumed_count": len(consumed_items),
            "items": consumed_items,
        }


class TestConcurrency:
    """Test concurrent operations on the queue."""

    @pytest.mark.asyncio
    async def test_producer_consumer_pattern(self, actor_system):
        """Test producer-consumer pattern with queue actor."""
        # Create queue actor with larger capacity for this test
        config = QueueConfig(max_size=20)
        queue_ref = await actor_system.spawn(QueueActor, "/user/pc_queue", config)

        # Create producers
        producer1 = await actor_system.spawn(
            ProducerActor, "/user/producer1", queue_ref, 5, 0.01
        )
        producer2 = await actor_system.spawn(
            ProducerActor, "/user/producer2", queue_ref, 5, 0.01
        )

        # Create consumers
        consumer1 = await actor_system.spawn(
            ConsumerActor, "/user/consumer1", queue_ref, 6, 0.01
        )
        consumer2 = await actor_system.spawn(
            ConsumerActor, "/user/consumer2", queue_ref, 6, 0.01
        )

        # Start production first
        producer_results = await asyncio.gather(
            producer1.start_producing.ask(), producer2.start_producing.ask()
        )

        # Then start consumption
        consumer_results = await asyncio.gather(
            consumer1.start_consuming.ask(), consumer2.start_consuming.ask()
        )

        # Verify results
        total_produced = sum(r["produced_count"] for r in producer_results)
        total_consumed = sum(r["consumed_count"] for r in consumer_results)

        assert total_produced == 10  # 5 + 5
        assert total_consumed <= total_produced  # Can't consume more than produced

        # Get all produced items
        all_produced_items = []
        for result in producer_results:
            all_produced_items.extend(result["items"])

        # Get all consumed items
        all_consumed_items = []
        for result in consumer_results:
            all_consumed_items.extend(result["items"])

        # Verify no duplicates in consumed items
        assert len(all_consumed_items) == len(set(all_consumed_items))

        # Verify all consumed items were actually produced
        assert set(all_consumed_items).issubset(set(all_produced_items))

        await queue_ref.stop()


# ============================================================================
# Performance Tests
# ============================================================================


class TestPerformance:
    """Test queue performance under various conditions."""

    @pytest.mark.asyncio
    async def test_high_throughput_operations(self, actor_system):
        """Test high throughput operations."""
        # Create queue with large capacity
        config = QueueConfig(max_size=1000)
        queue_ref = await actor_system.spawn(QueueActor, "/user/perf_queue", config)

        num_items = 100  # Reduced for faster tests
        items = [f"perf_item_{i}" for i in range(num_items)]

        import time

        # Measure put performance
        start_time = time.time()
        for item in items:
            result = await queue_ref.put_item.ask(item, True)
            assert result.success, f"Put failed for {item}"
        put_duration = time.time() - start_time

        # Measure get performance
        start_time = time.time()
        retrieved_items = []
        for _ in range(num_items):
            result = await queue_ref.get_item.ask(True)
            if result.success:
                retrieved_items.append(result.item)
        get_duration = time.time() - start_time

        put_rate = num_items / put_duration if put_duration > 0 else float("inf")
        get_rate = (
            len(retrieved_items) / get_duration if get_duration > 0 else float("inf")
        )

        print(f"Put rate: {put_rate:.0f} items/sec")
        print(f"Get rate: {get_rate:.0f} items/sec")

        assert retrieved_items == items
        assert put_rate > 10  # Should handle at least 10 items/sec
        assert get_rate > 10  # Should handle at least 10 items/sec

        await queue_ref.stop()

    @pytest.mark.asyncio
    async def test_batch_vs_individual_performance(self, actor_system):
        """Compare batch vs individual operations performance."""
        config = QueueConfig(max_size=1000)
        queue_ref = await actor_system.spawn(
            QueueActor, "/user/batch_perf_queue", config
        )

        num_items = 50  # Reduced for faster tests
        items = [f"batch_perf_item_{i}" for i in range(num_items)]

        import time

        # Test individual puts
        start_time = time.time()
        for item in items:
            result = await queue_ref.put_item.ask(item, True)
            assert result.success
        individual_put_time = time.time() - start_time

        # Clear queue
        await queue_ref.clear_queue.ask()

        # Test batch put
        start_time = time.time()
        result = await queue_ref.put_batch_items.ask(items, True)
        assert result.success
        batch_put_time = time.time() - start_time

        print(f"Individual put time: {individual_put_time:.3f}s")
        print(f"Batch put time: {batch_put_time:.3f}s")

        if batch_put_time > 0:
            speedup = individual_put_time / batch_put_time
            print(f"Batch speedup: {speedup:.1f}x")
            # Batch operations should be faster or at least comparable
            assert batch_put_time <= individual_put_time * 1.5  # Allow some variance

        await queue_ref.stop()


# ============================================================================
# Error Handling Tests
# ============================================================================


class TestErrorHandling:
    """Test error handling scenarios."""

    @pytest.mark.asyncio
    async def test_invalid_operations_with_client(self, test_queue):
        """Test handling of invalid operations with client interface."""
        # Test getting from empty queue
        with pytest.raises(QueueEmptyError):
            await test_queue.get(block=False)

        # Fill queue to capacity
        for i in range(5):  # max_size = 5
            await test_queue.put(f"item{i}")

        # Test putting to full queue
        with pytest.raises(QueueFullError):
            await test_queue.put("overflow", block=False)

        # Test batch get with more items than available
        await test_queue.clear()
        await test_queue.put("single_item")

        # This should return only 1 item, not raise an error for non-blocking
        items = await test_queue.get_batch(5, block=False)
        assert len(items) == 1
        assert items[0] == "single_item"

    @pytest.mark.asyncio
    async def test_invalid_operations_with_actor(self, queue_actor_ref):
        """Test handling of invalid operations with actor method calls."""
        # Test getting from empty queue
        result = await queue_actor_ref.get_item.ask(False)
        assert result.success is False
        assert "empty" in result.error.lower()

        # Fill queue to capacity
        for i in range(5):  # max_size = 5
            result = await queue_actor_ref.put_item.ask(f"item{i}", True)
            assert result.success

        # Test putting to full queue
        result = await queue_actor_ref.put_item.ask("overflow", False)
        assert result.success is False
        assert "full" in result.error.lower()

        # Test batch get with more items than available
        await queue_actor_ref.clear_queue.ask()
        await queue_actor_ref.put_item.ask("single_item", True)

        # This should return only 1 item for non-blocking
        result = await queue_actor_ref.get_batch_items.ask(5, False)
        assert result.success is True
        assert len(result.items) == 1
        assert result.items[0] == "single_item"


# ============================================================================
# Factory Function Tests
# ============================================================================


class TestFactoryFunctions:
    """Test the factory functions for creating queues."""

    @pytest.mark.asyncio
    async def test_create_queue(self, actor_system):
        """Test create_queue factory function."""
        queue = await create_queue(actor_system, name="factory_queue", max_size=10)

        assert queue.name == "factory_queue"
        assert queue.path == "/user/queues/factory_queue"

        # Test basic operations
        await queue.put("factory_test")
        item = await queue.get()
        assert item == "factory_test"

    @pytest.mark.asyncio
    async def test_get_queue(self, actor_system):
        """Test get_queue factory function."""
        # First create a queue
        original_queue = await create_queue(
            actor_system, name="existing_queue", max_size=10
        )
        await original_queue.put("existing_item")

        # Then get reference to it
        retrieved_queue = get_queue(actor_system, "existing_queue")

        assert retrieved_queue.name == "existing_queue"
        assert retrieved_queue.path == "/user/queues/existing_queue"

        # Should be able to access the same data
        item = await retrieved_queue.get()
        assert item == "existing_item"


@pytest.mark.asyncio
async def test_queue_serialization(actor_system):
    """Test queue serialization and deserialization."""
    queue_name = f"serial_queue_{uuid.uuid4().hex[:8]}"
    queue = await create_queue(actor_system, name=queue_name, max_size=5)

    class TaskExecutor:
        def __init__(self, q):
            self.queue = q

        async def add(self, x, y):
            await self.queue.put({"state": "running"})
            res = x + y
            await self.queue.put({"state": "completed", "result": res})
            return res

    class TaskMonitor:
        def __init__(self, q):
            self.queue = q

        async def on_start(self, ctx):
            asyncio.create_task(self.monitor())

        async def monitor(self):
            states = []
            while True:
                item = await self.queue.get()
                print(f"Monitor received: {item}")
                states.append(item)
                if item.get("state") == "completed":
                    break
            return states

    executor_ref = await actor_system.spawn(TaskExecutor, "task_executor", queue)
    monitor_ref = await actor_system.spawn(TaskMonitor, "task_monitor", queue)

    res = await executor_ref.add(3, 4)
    assert res == 7, "3 + 4 should equal 7"
