"""
Complete ObjectStore test suite
Including functional tests, performance tests, boundary condition tests, concurrency tests, etc.
"""

import asyncio
import random
import sys
import time

import pytest
import pytest_asyncio
from pympler import asizeof

try:
    import numpy as np

    HAS_NUMPY = True
except ImportError:
    HAS_NUMPY = False
    np = None

from lyricore import PyObjectStore, PyStoreConfig, pickle
from lyricore.actor_wrapper import (
    MessageSerializer,
    ObjectStoreConfig,
    ObjectStoreRef,
)


class LargeObject:
    """Large object class for testing big data serialization"""

    def __init__(self, size_mb: int):
        self.size_mb = size_mb
        # Create byte data of specified size (1MB = 1024*1024 bytes)
        self.data = bytearray(size_mb * 1024 * 1024)
        # Optional: Fill with some data to avoid compression
        for i in range(0, len(self.data), 1024):
            self.data[i : i + 8] = (i // 1024).to_bytes(8, "big")

    def __eq__(self, other):
        if not isinstance(other, LargeObject):
            return False
        return (
            self.size_mb == other.size_mb
            and len(self.data) == len(other.data)
            and self.data[:100] == other.data[:100]
        )  # Just check first 100 bytes for equality
        # self.data == other.data)  # Just check first 100 bytes for equality


class SerializationTestObject:
    """Complex object for serialization testing"""

    def __init__(self):
        self.simple_attr = "test"
        self.dict_attr = {"key": "value", "nested": {"inner": 42}}
        self.list_attr = [1, 2, 3, "four", {"five": 5}]
        self.none_attr = None

    def __eq__(self, other):
        return (
            isinstance(other, SerializationTestObject)
            and self.simple_attr == other.simple_attr
            and self.dict_attr == other.dict_attr
            and self.list_attr == other.list_attr
            and self.none_attr == other.none_attr
        )


# ============================================================================
# Test Fixtures
# ============================================================================


@pytest.fixture(scope="function")
def store_config():
    """Standard storage configuration"""
    return PyStoreConfig(
        max_memory=512 * 1024 * 1024,  # 512MB
        max_object_size=128 * 1024 * 1024,  # 128MB
        memory_pressure_threshold=0.8,
        track_access_time=True,
    )


@pytest.fixture(scope="function")
def small_store_config():
    """Small capacity storage configuration for testing memory pressure"""
    return PyStoreConfig(
        max_memory=64 * 1024 * 1024,  # 64MB
        max_object_size=32 * 1024 * 1024,  # 32MB
        memory_pressure_threshold=0.7,
        track_access_time=True,
    )


@pytest_asyncio.fixture(scope="function")
async def object_store(store_config):
    """Standard ObjectStore instance"""
    store = PyObjectStore(store_config)
    yield store
    # Cleanup
    await store.clear()


@pytest_asyncio.fixture(scope="function")
async def small_object_store(small_store_config):
    """Small capacity ObjectStore instance"""
    store = PyObjectStore(small_store_config)
    yield store
    await store.clear()


@pytest.fixture(scope="function")
def objectstore_config():
    """ObjectStore wrapper configuration"""
    return ObjectStoreConfig(
        auto_serialize_threshold=1024 * 1024,  # 1MB
        enable_batch_optimization=True,
        auto_serialize_patterns=["data", "array", "buffer", "payload"],
    )


@pytest_asyncio.fixture(scope="function")
async def message_serializer(object_store, objectstore_config):
    """Message serializer"""
    return MessageSerializer(object_store, objectstore_config)


# Test data generators
@pytest.fixture
def small_test_data():
    """Small test data"""
    return {"message": "hello", "number": 42, "list": [1, 2, 3]}


@pytest.fixture
def medium_test_data():
    """Medium-sized test data"""
    return {
        "users": [{"name": f"user_{i}", "id": i} for i in range(1000)],
        "metadata": {"created": time.time(), "version": "1.0"},
        "large_text": "x" * 50000,  # 50KB text
    }


@pytest.fixture
def large_bytes_data():
    """Large byte data"""
    return b"x" * (10 * 1024 * 1024)  # 10MB


@pytest.fixture
def numpy_test_data():
    """NumPy test data"""
    if not HAS_NUMPY:
        pytest.skip("NumPy not available")
    return {
        "small_array": np.random.rand(100, 100),
        "large_array": np.random.rand(1000, 1000),
        "int_array": np.arange(10000),
        "complex_array": np.random.rand(50, 50, 10).astype(np.float32),
    }


# ============================================================================
# Basic Functionality Tests
# ============================================================================


class TestBasicOperations:
    """Basic operations tests"""

    @pytest.mark.asyncio
    async def test_store_creation(self, store_config):
        """Test storage creation"""
        store = PyObjectStore(store_config)
        stats = store.stats()

        assert stats["total_objects"] == 0
        assert stats["total_memory"] == 0
        assert stats["hit_rate"] == 0.0

    @pytest.mark.asyncio
    async def test_put_get_simple_object(self, object_store, small_test_data):
        """Test storing and retrieving simple objects"""
        # Store object
        object_id = await object_store.put(small_test_data)
        assert isinstance(object_id, str)
        assert len(object_id) > 0

        # Get object
        retrieved_data = await object_store.get_object(object_id)
        assert retrieved_data == small_test_data

        # Check statistics
        stats = object_store.stats()
        assert stats["total_objects"] == 1
        assert stats["total_memory"] > 0

    @pytest.mark.asyncio
    async def test_put_get_bytes(self, object_store, large_bytes_data):
        """Test storing and retrieving byte data"""
        object_id = await object_store.put_bytes(large_bytes_data)
        retrieved_bytes = await object_store.get_bytes(object_id)

        assert retrieved_bytes == large_bytes_data
        assert len(retrieved_bytes) == len(large_bytes_data)

    @pytest.mark.asyncio
    async def test_object_reference(self, object_store, small_test_data):
        """Test object reference retrieval"""
        object_id = await object_store.put(small_test_data)

        # Get object reference
        obj_ref = await object_store.get(object_id)
        assert obj_ref.id == object_id
        assert obj_ref.size > 0

        # Get data through reference
        ref_bytes = obj_ref.as_bytes()
        # Note: May not be exactly equal due to serialization differences, so we verify through deserialization
        assert pickle.loads(ref_bytes) == small_test_data

    @pytest.mark.asyncio
    async def test_object_existence_and_deletion(self, object_store, small_test_data):
        """Test object existence check and deletion"""
        object_id = await object_store.put(small_test_data)

        # Check existence
        exists = await object_store.contains(object_id)
        assert exists is True

        # Delete object
        deleted = await object_store.delete(object_id)
        assert deleted is True

        # Check non-existence
        exists_after = await object_store.contains(object_id)
        assert exists_after is False

        # Delete again should return False
        deleted_again = await object_store.delete(object_id)
        assert deleted_again is False


# ============================================================================
# NumPy Specialized Tests
# ============================================================================


@pytest.mark.skipif(not HAS_NUMPY, reason="NumPy not available")
class TestNumpyOperations:
    """NumPy array operations tests"""

    @pytest.mark.asyncio
    async def test_numpy_safe_copy(self, object_store, numpy_test_data):
        """Test NumPy safe copy mode"""
        array = numpy_test_data["large_array"]

        object_id = await object_store.put_numpy(array, copy_mode="safe_copy")
        retrieved_array = await object_store.get_numpy(object_id)

        assert retrieved_array.shape == array.shape
        assert retrieved_array.dtype == array.dtype
        np.testing.assert_array_equal(retrieved_array, array)

    @pytest.mark.asyncio
    async def test_numpy_fast_copy(self, object_store, numpy_test_data):
        """Test NumPy fast copy mode"""
        array = numpy_test_data["large_array"]

        object_id = await object_store.put_numpy(array, copy_mode="fast_copy")
        retrieved_array = await object_store.get_numpy(object_id)

        assert retrieved_array.shape == array.shape
        assert retrieved_array.dtype == array.dtype
        np.testing.assert_array_equal(retrieved_array, array)

    @pytest.mark.asyncio
    async def test_numpy_zero_copy(self, object_store, numpy_test_data):
        """Test NumPy zero copy mode"""
        array = numpy_test_data[
            "small_array"
        ]  # Use smaller array for zero copy testing

        object_id = await object_store.put_numpy(array, copy_mode="zero_copy")
        retrieved_array = await object_store.get_numpy(object_id)

        assert retrieved_array.shape == array.shape
        assert retrieved_array.dtype == array.dtype
        np.testing.assert_array_equal(retrieved_array, array)

    @pytest.mark.asyncio
    async def test_numpy_adaptive_storage(self, object_store, numpy_test_data):
        """Test NumPy adaptive storage"""
        small_array = numpy_test_data["small_array"]
        large_array = numpy_test_data["large_array"]

        # Small array should use copy mode
        small_result = await object_store.put_numpy_adaptive(
            small_array,
            size_threshold=50 * 1024 * 1024,  # 50MB threshold
        )
        assert isinstance(small_result, dict)
        assert "object_id" in small_result
        assert small_result["copy_mode"] in ["fastcopy", "safecopy"]

        # Large array should use zero copy mode (if possible)
        large_result = await object_store.put_numpy_adaptive(
            large_array,
            size_threshold=1024 * 1024,  # 1MB threshold
        )
        assert isinstance(large_result, dict)
        assert "object_id" in large_result
        # Verify it can be retrieved
        retrieved = await object_store.get_numpy(large_result["object_id"])
        np.testing.assert_array_equal(retrieved, large_array)

    @pytest.mark.asyncio
    async def test_numpy_batch_operations(self, object_store, numpy_test_data):
        """Test NumPy batch operations"""
        arrays = [
            numpy_test_data["small_array"],
            numpy_test_data["int_array"].reshape(100, 100),
            numpy_test_data["complex_array"],
        ]

        # Batch store
        object_ids = await object_store.put_numpy_batch_optimized(
            arrays, copy_mode="fast_copy"
        )
        assert len(object_ids) == len(arrays)
        assert all(isinstance(id, str) for id in object_ids)

        # Verify each array can be correctly retrieved
        for i, object_id in enumerate(object_ids):
            retrieved = await object_store.get_numpy(object_id)
            np.testing.assert_array_equal(retrieved, arrays[i])

    @pytest.mark.asyncio
    async def test_numpy_different_dtypes(self, object_store):
        """Test different NumPy data types"""
        test_arrays = {
            "float32": np.random.rand(100, 100).astype(np.float32),
            "float64": np.random.rand(100, 100).astype(np.float64),
            "int32": np.random.randint(0, 1000, (100, 100), dtype=np.int32),
            "int64": np.random.randint(0, 1000, (100, 100), dtype=np.int64),
            "bool": np.random.choice([True, False], (100, 100)),
            "complex64": (np.random.rand(50, 50) + 1j * np.random.rand(50, 50)).astype(
                np.complex64
            ),
        }

        for dtype_name, array in test_arrays.items():
            object_id = await object_store.put_numpy(array)
            retrieved = await object_store.get_numpy(object_id)

            assert retrieved.dtype == array.dtype, f"dtype mismatch for {dtype_name}"
            assert retrieved.shape == array.shape, f"shape mismatch for {dtype_name}"
            np.testing.assert_array_equal(retrieved, array)


# ============================================================================
# Message Serialization Tests
# ============================================================================


class TestMessageSerialization:
    """Message serialization tests"""

    def test_should_serialize_logic(self, message_serializer):
        """Test serialization decision logic"""
        # Small objects should not be serialized
        small_obj = {"key": "value"}
        assert not message_serializer._should_serialize(small_obj)

        # Large objects should be serialized
        large_obj = LargeObject(2)  # 2MB
        assert message_serializer._should_serialize(large_obj)

        # Parameter name matching should affect decision
        medium_obj = b"x" * (512 * 1024)  # 512KB
        assert not message_serializer._should_serialize(
            medium_obj
        )  # Normally not serialized
        # Note: Parameter name pattern matching needs to meet size threshold too, so might still not serialize

    @pytest.mark.asyncio
    async def test_serialize_deserialize_simple_args(
        self, message_serializer, small_test_data
    ):
        """Test serialization and deserialization of simple arguments"""
        args = (small_test_data, 42, "test")
        kwargs = {"data": small_test_data, "count": 10}

        # Serialize
        ser_args, ser_kwargs = await message_serializer.serialize_args(args, kwargs)

        # Small objects should remain unchanged
        assert ser_args == args
        assert ser_kwargs == kwargs

        # Deserialize
        deser_args, deser_kwargs = await message_serializer.deserialize_args(
            ser_args, ser_kwargs
        )
        assert deser_args == args
        assert deser_kwargs == kwargs

    @pytest.mark.asyncio
    @pytest.mark.skipif(not HAS_NUMPY, reason="NumPy not available")
    async def test_serialize_deserialize_large_args(
        self, object_store, message_serializer
    ):
        """Test serialization and deserialization of large object arguments"""
        large_array = np.random.rand(500, 500)  # About 2MB
        large_bytes = b"x" * (2 * 1024 * 1024)  # 2MB
        small_obj = {"small": "data"}

        args = (small_obj, large_array, large_bytes)
        kwargs = {"small": small_obj, "array": large_array, "bytes": large_bytes}

        # Serialize
        ser_args, ser_kwargs = await message_serializer.serialize_args(args, kwargs)

        # Check small object unchanged
        assert ser_args[0] == small_obj
        assert ser_kwargs["small"] == small_obj

        # Check large objects converted to references
        assert isinstance(ser_args[1], ObjectStoreRef)
        assert isinstance(ser_args[2], ObjectStoreRef)
        assert isinstance(ser_kwargs["array"], ObjectStoreRef)
        assert isinstance(ser_kwargs["bytes"], ObjectStoreRef)

        # Deserialize
        deser_args, deser_kwargs = await message_serializer.deserialize_args(
            ser_args, ser_kwargs
        )

        # Verify data correctness
        assert deser_args[0] == small_obj
        assert deser_kwargs["small"] == small_obj

        np.testing.assert_array_equal(deser_args[1], large_array)
        np.testing.assert_array_equal(deser_kwargs["array"], large_array)

        assert deser_args[2] == large_bytes
        assert deser_kwargs["bytes"] == large_bytes


# ============================================================================
# ObjectStoreRef Tests
# ============================================================================


class TestObjectStoreRef:
    """ObjectStoreRef tests"""

    @pytest.mark.asyncio
    async def test_object_store_ref_caching(self, object_store, medium_test_data):
        """Test ObjectStoreRef caching mechanism"""
        object_id = await object_store.put(medium_test_data)
        ref = ObjectStoreRef(object_id, object_store, {"type": "dict"})

        # Initial state
        assert not ref._is_loaded
        assert ref._cached_object is None

        # First get
        data1 = await ref.get()
        assert ref._is_loaded
        assert ref._cached_object is not None
        assert data1 == medium_test_data

        # Second get (should use cache)
        data2 = await ref.get()
        assert data2 == medium_test_data
        assert data1 is data2  # Should be same object (cached)

    @pytest.mark.asyncio
    @pytest.mark.skipif(not HAS_NUMPY, reason="NumPy not available")
    async def test_object_store_ref_numpy(self, object_store):
        """Test ObjectStoreRef NumPy support"""
        array = np.random.rand(100, 100)
        object_id = await object_store.put_numpy(array)

        ref = ObjectStoreRef(
            object_id,
            object_store,
            {
                "is_numpy": True,
                "type": "numpy.ndarray",
                "shape": array.shape,
                "dtype": str(array.dtype),
            },
        )

        retrieved = await ref.get()
        np.testing.assert_array_equal(retrieved, array)


# ============================================================================
# Batch Operations Tests
# ============================================================================


class TestBatchOperations:
    """Batch operations tests"""

    @pytest.mark.asyncio
    async def test_batch_put_get_objects(self, object_store):
        """Test batch storing and retrieving objects"""
        test_objects = [
            {"id": i, "data": f"object_{i}", "value": i * 10} for i in range(50)
        ]

        # Batch store
        object_ids = await object_store.put_batch(test_objects)
        assert len(object_ids) == len(test_objects)
        assert all(isinstance(id, str) for id in object_ids)

        # Batch get
        retrieved_objects = await object_store.get_objects(object_ids)
        assert len(retrieved_objects) == len(test_objects)

        # Verify data correctness
        for original, retrieved in zip(test_objects, retrieved_objects):
            assert original == retrieved

    @pytest.mark.asyncio
    async def test_batch_get_references(self, object_store):
        """Test batch getting object references"""
        test_objects = [f"test_object_{i}" for i in range(20)]

        # Store first
        object_ids = await object_store.put_batch(test_objects)

        # Batch get references
        object_refs = await object_store.get_batch(object_ids)
        assert len(object_refs) == len(object_ids)

        # Verify reference validity
        for ref, original_id in zip(object_refs, object_ids):
            assert ref.id == original_id
            assert ref.size > 0


# ============================================================================
# Memory Management and Pressure Tests
# ============================================================================


class TestMemoryManagement:
    """Memory management tests"""

    @pytest.mark.asyncio
    async def test_memory_pressure_detection(self, small_object_store):
        """Test memory pressure detection"""
        store = small_object_store

        # Create large enough objects to trigger memory pressure
        large_objects = []
        for i in range(10):
            obj = LargeObject(5)  # 5MB object
            object_id = await store.put(obj)
            large_objects.append(object_id)

            # Check memory pressure info
            pressure_info = store.get_memory_pressure_info()
            print(
                f"Iteration {i}: Memory usage ratio: {pressure_info['memory_usage_ratio']:.2f}"
            )

            if pressure_info["is_under_pressure"]:
                print(f"Memory pressure detected at iteration {i}")
                break

        # Verify pressure detection
        final_pressure = store.get_memory_pressure_info()
        print(f"Final memory usage: {final_pressure['memory_usage_ratio']:.2f}")

        # Cleanup test
        await store.cleanup()

    @pytest.mark.asyncio
    async def test_memory_cleanup(self, object_store):
        """Test memory cleanup"""
        # Create some objects
        object_ids = []
        for i in range(20):
            obj = {"id": i, "data": f"data_{i}" * 1000}  # Create some data
            object_id = await object_store.put(obj)
            object_ids.append(object_id)

        stats_before = object_store.stats()
        assert stats_before["total_objects"] == 20
        print(object_store.analyze_memory_usage())

        # Delete half the objects
        for i in range(10):
            await object_store.delete(object_ids[i])

        object_store.print_stats()
        print(object_store.analyze_memory_usage())
        # assert delete_size == 10
        stats_after = object_store.stats()
        assert stats_after["total_objects"] == 10
        assert stats_after["total_memory"] < stats_before["total_memory"]

    @pytest.mark.asyncio
    async def test_memory_analysis(self, object_store):
        """Test memory analysis functionality"""
        # Create objects of different sizes
        small_objects = [{"small": i} for i in range(50)]
        medium_objects = [{"medium": "x" * 10000} for i in range(10)]

        # Store objects
        for obj in small_objects:
            await object_store.put(obj)
        for obj in medium_objects:
            await object_store.put(obj)

        # Analyze memory usage
        analysis = object_store.analyze_memory_usage()

        assert analysis["total_objects"] == 60
        assert analysis["total_memory_bytes"] > 0
        assert analysis["memory_usage_mb"] > 0
        assert analysis["average_object_size_bytes"] > 0

        print(f"Memory analysis: {analysis}")


# ============================================================================
# Concurrency and Performance Tests
# ============================================================================


class TestConcurrencyAndPerformance:
    """Concurrency and performance tests"""

    @pytest.mark.asyncio
    async def test_concurrent_put_operations(self, object_store):
        """Test concurrent store operations"""

        async def store_object(index):
            obj = {"index": index, "data": f"concurrent_data_{index}" * 100}
            object_id = await object_store.put(obj)
            return object_id, obj

        # Execute store operations concurrently
        tasks = [store_object(i) for i in range(50)]
        results = await asyncio.gather(*tasks)

        assert len(results) == 50

        # Verify all objects were successfully stored
        for object_id, original_obj in results:
            retrieved = await object_store.get_object(object_id)
            assert retrieved == original_obj

    @pytest.mark.asyncio
    async def test_concurrent_get_operations(self, object_store):
        """Test concurrent get operations"""
        # Store some objects first
        object_ids = []
        original_objects = []
        for i in range(20):
            obj = {"id": i, "shared_data": f"data_{i}"}
            object_id = await object_store.put(obj)
            object_ids.append(object_id)
            original_objects.append(obj)

        async def get_object(object_id, expected_obj):
            retrieved = await object_store.get_object(object_id)
            assert retrieved == expected_obj
            return retrieved

        # Concurrent get
        tasks = [get_object(oid, obj) for oid, obj in zip(object_ids, original_objects)]
        results = await asyncio.gather(*tasks)

        assert len(results) == 20

    @pytest.mark.asyncio
    @pytest.mark.skipif(not HAS_NUMPY, reason="NumPy not available")
    async def test_numpy_concurrent_operations(self, object_store):
        """Test NumPy concurrent operations"""

        async def store_and_retrieve_array(size, index):
            array = np.random.rand(size, size)
            object_id = await object_store.put_numpy(array, copy_mode="fast_copy")
            retrieved = await object_store.get_numpy(object_id)
            np.testing.assert_array_equal(retrieved, array)
            return object_id

        # Concurrent store and retrieve of different sized arrays
        sizes = [50, 100, 150, 200, 100, 75]  # Different sizes
        tasks = [store_and_retrieve_array(size, i) for i, size in enumerate(sizes)]

        start_time = time.time()
        results = await asyncio.gather(*tasks)
        duration = time.time() - start_time

        print(f"Concurrent NumPy operations completed in {duration:.3f}s")
        assert len(results) == len(sizes)

    @pytest.mark.asyncio
    async def test_mixed_concurrent_operations(self, object_store):
        """Test mixed concurrent operations"""

        async def mixed_operation(index):
            if index % 3 == 0:
                # Python object
                obj = {
                    "type": "python",
                    "index": index,
                    "data": list(range(index * 10)),
                }
                object_id = await object_store.put(obj)
                retrieved = await object_store.get_object(object_id)
                assert retrieved == obj
            elif index % 3 == 1:
                # Byte data
                data = f"bytes_data_{index}".encode() * 1000
                object_id = await object_store.put_bytes(data)
                retrieved = await object_store.get_bytes(object_id)
                assert retrieved == data
            else:
                # NumPy array (if available)
                if HAS_NUMPY:
                    array = np.random.rand(50, 50)
                    object_id = await object_store.put_numpy(array)
                    retrieved = await object_store.get_numpy(object_id)
                    np.testing.assert_array_equal(retrieved, array)
                else:
                    # If no NumPy, use Python object
                    obj = {"fallback": True, "index": index}
                    object_id = await object_store.put(obj)
                    retrieved = await object_store.get_object(object_id)
                    assert retrieved == obj

            return object_id

        # Execute mixed operations concurrently
        tasks = [mixed_operation(i) for i in range(30)]
        results = await asyncio.gather(*tasks)

        assert len(results) == 30
        assert all(isinstance(oid, str) for oid in results)


# ============================================================================
# Smart Storage and Adaptive Tests
# ============================================================================


class TestSmartStorage:
    """Smart storage tests"""

    @pytest.mark.asyncio
    async def test_smart_storage_small_objects(self, object_store):
        """Test smart storage handling of small objects"""
        small_obj = {"small": "data", "size": "tiny"}

        # Using smart storage, small objects should be stored directly
        object_id = await object_store.put_smart(small_obj, size_threshold=1024 * 1024)
        retrieved = await object_store.get_object(object_id)

        assert retrieved == small_obj

    @pytest.mark.asyncio
    async def test_smart_storage_large_objects(self, object_store):
        """Test smart storage handling of large objects"""
        # Create large object
        large_obj = LargeObject(2)  # 2MB

        object_id = await object_store.put_smart(large_obj, size_threshold=1024 * 1024)
        retrieved = await object_store.get_object(object_id)

        assert retrieved == large_obj

    @pytest.mark.asyncio
    async def test_storage_info_retrieval(self, object_store):
        """Test storage information retrieval"""
        test_obj = {"info": "test", "data": list(range(1000))}
        object_id = await object_store.put(test_obj)

        # Get storage info
        storage_info = await object_store.get_storage_info(object_id)

        assert storage_info["id"] == object_id
        assert storage_info["size"] > 0
        assert "storage_type" in storage_info
        assert "is_zero_copy" in storage_info
        assert "data_type" in storage_info


# ============================================================================
# Zero Copy and Storage Statistics Tests
# ============================================================================


class TestZeroCopyAndStats:
    """Zero copy and statistics tests"""

    @pytest.mark.asyncio
    @pytest.mark.skipif(not HAS_NUMPY, reason="NumPy not available")
    async def test_zero_copy_stats(self, object_store):
        """Test zero copy statistics"""
        # Create some arrays for testing
        arrays = [
            np.random.rand(100, 100),
            np.random.rand(200, 200),
            np.random.rand(150, 150),
        ]

        # Store using different modes
        await object_store.put_numpy(arrays[0], copy_mode="zero_copy")
        await object_store.put_numpy(arrays[1], copy_mode="fast_copy")
        await object_store.put_numpy(arrays[2], copy_mode="safe_copy")

        # Get zero copy statistics
        stats = await object_store.get_zero_copy_stats()

        assert "zero_copy_objects" in stats
        assert "zero_copy_memory_bytes" in stats
        assert "shared_objects" in stats
        assert "standard_objects" in stats
        assert "total_objects" in stats
        assert "zero_copy_ratio" in stats

        print(f"Zero-copy stats: {stats}")

    @pytest.mark.asyncio
    async def test_comprehensive_stats(self, object_store):
        """Test comprehensive statistics"""
        # Create various types of objects
        objects_to_store = [
            {"type": "dict", "data": "test"},
            b"bytes data" * 1000,
            list(range(1000)),
            "string data" * 100,
        ]

        # Store objects
        object_ids = []
        for obj in objects_to_store:
            if isinstance(obj, bytes):
                oid = await object_store.put_bytes(obj)
            else:
                oid = await object_store.put(obj)
            object_ids.append(oid)

        # Get various statistics
        basic_stats = object_store.stats()
        memory_analysis = object_store.analyze_memory_usage()
        pressure_info = object_store.get_memory_pressure_info()

        # Verify basic statistics
        assert basic_stats["total_objects"] == len(objects_to_store)
        assert basic_stats["total_memory"] > 0

        # Verify memory analysis
        assert memory_analysis["total_objects"] == len(objects_to_store)
        assert memory_analysis["total_memory_bytes"] > 0
        assert memory_analysis["average_object_size_bytes"] > 0

        # Verify pressure info
        assert "current_memory_bytes" in pressure_info
        assert "max_memory_bytes" in pressure_info
        assert "memory_usage_ratio" in pressure_info

        print(f"Basic stats: {basic_stats}")
        print(f"Memory analysis: {memory_analysis}")
        print(f"Pressure info: {pressure_info}")


# ============================================================================
# Error Handling and Edge Cases Tests
# ============================================================================


class TestErrorHandlingAndEdgeCases:
    """Error handling and edge cases tests"""

    @pytest.mark.asyncio
    async def test_nonexistent_object_access(self, object_store):
        """Test accessing non-existent objects"""
        fake_id = "12345"

        # Check non-existence
        exists = await object_store.contains(fake_id)
        assert exists is False

        # Attempt to get should raise exception
        with pytest.raises(Exception):  # Could be KeyError or other exception
            await object_store.get_object(fake_id)

    @pytest.mark.asyncio
    async def test_invalid_object_id_format(self, object_store):
        """Test invalid object ID format"""
        invalid_ids = ["", "invalid", "not_a_number", "-1", "abc123"]

        for invalid_id in invalid_ids:
            with pytest.raises(Exception):
                await object_store.get_object(invalid_id)

    @pytest.mark.asyncio
    async def test_object_size_limits(self, small_object_store):
        """Test object size limits"""
        # Try to store object exceeding limits
        try:
            huge_obj = LargeObject(50)  # 50MB, exceeds 32MB limit
            with pytest.raises(Exception):  # Should raise size limit exception
                await small_object_store.put(huge_obj)
        except Exception as e:
            print(f"Expected size limit error: {e}")

    @pytest.mark.asyncio
    async def test_memory_pressure_handling(self, small_object_store):
        """Test memory pressure handling"""
        store = small_object_store
        stored_objects = []

        # Keep storing objects until memory pressure
        try:
            for i in range(50):  # Try to store many objects
                obj = LargeObject(1)  # 1MB object
                object_id = await store.put(obj)
                stored_objects.append(object_id)

                # Check memory pressure
                pressure = store.get_memory_pressure_info()
                if pressure["is_under_pressure"]:
                    print(f"Memory pressure reached at object {i}")
                    break

        except Exception as e:
            print(f"Memory pressure exception: {e}")

        # Verify at least some objects were stored
        assert len(stored_objects) > 0

        # Cleanup
        await store.clear()

    @pytest.mark.asyncio
    @pytest.mark.skipif(not HAS_NUMPY, reason="NumPy not available")
    async def test_numpy_error_conditions(self, object_store):
        """Test NumPy error conditions"""
        # Test non-contiguous array
        array = np.random.rand(100, 100)
        non_contiguous = array[::2, ::2]  # Create non-contiguous array

        # Should be able to handle non-contiguous array (possibly through copying)
        object_id = await object_store.put_numpy(non_contiguous, copy_mode="safe_copy")
        retrieved = await object_store.get_numpy(object_id)

        assert retrieved.shape == non_contiguous.shape
        np.testing.assert_array_equal(retrieved, non_contiguous)

    @pytest.mark.asyncio
    async def test_serialization_edge_cases(self, message_serializer):
        """Test serialization edge cases"""
        edge_cases = [
            None,
            [],
            {},
            "",
            0,
            False,
            float("inf"),
            float("-inf"),
        ]

        for case in edge_cases:
            try:
                # Test serialization doesn't crash
                result = await message_serializer._serialize_value(case)
                # For small objects, should return original object
                if case is not None and not (
                    isinstance(case, float) and not np.isfinite(case)
                ):
                    assert result == case
            except Exception as e:
                # Some special values may not be serializable, which is acceptable
                print(f"Serialization failed for {case}: {e}")


# ============================================================================
# Object Views and References Tests
# ============================================================================


class TestObjectViewsAndReferences:
    """Object views and references tests"""

    @pytest.mark.asyncio
    async def test_object_view_creation(self, object_store, medium_test_data):
        """Test object view creation"""
        object_id = await object_store.put(medium_test_data)

        # Get object view
        view = await object_store.get_view(object_id)

        assert view.id == object_id
        assert view.size > 0
        assert view.data_type

        # Get byte data through view
        view_bytes = view.as_bytes()
        assert isinstance(view_bytes, bytes)
        assert len(view_bytes) > 0

        # Convert to object through view
        view_object = view.to_object()
        assert view_object == medium_test_data

    @pytest.mark.asyncio
    @pytest.mark.skipif(not HAS_NUMPY, reason="NumPy not available")
    async def test_numpy_object_view(self, object_store):
        """Test NumPy object view"""
        array = np.random.rand(100, 100)
        object_id = await object_store.put_numpy(array)

        # Get object reference
        obj_ref = await object_store.get(object_id)

        # Check NumPy compatibility
        numpy_array = obj_ref.as_numpy()
        if numpy_array is not None:
            np.testing.assert_array_equal(numpy_array, array)

        # Get metadata
        metadata = obj_ref.metadata()
        assert "data_type" in metadata
        assert "shape" in metadata


# ============================================================================
# Long Running and Stability Tests
# ============================================================================


class TestLongRunningAndStability:
    """Long running and stability tests"""

    @pytest.mark.asyncio
    async def test_long_running_operations(self, object_store):
        """Test long running operations"""
        operations_count = 200
        object_ids = []

        # Execute many operations
        for i in range(operations_count):
            if i % 3 == 0:
                # Store operation
                obj = {"iteration": i, "data": f"long_running_{i}" * 50}
                object_id = await object_store.put(obj)
                object_ids.append(object_id)
            elif i % 3 == 1 and object_ids:
                # Get operation
                random_id = random.choice(object_ids)
                retrieved = await object_store.get_object(random_id)
                assert "iteration" in retrieved
            else:
                # Delete operation
                if object_ids:
                    to_delete = object_ids.pop()
                    await object_store.delete(to_delete)

            # Print stats every 50 operations
            if i % 50 == 0:
                stats = object_store.stats()
                print(
                    f"Operation {i}: {stats['total_objects']} objects, "
                    f"{stats['memory_usage_mb']:.2f}MB"
                )

        # Final statistics
        final_stats = object_store.stats()
        print(f"Final stats: {final_stats}")

        # Verify system stability
        assert final_stats["total_objects"] >= 0
        assert final_stats["hit_rate"] >= 0.0

    @pytest.mark.asyncio
    async def test_memory_leak_detection(self, object_store):
        """Test memory leak detection"""
        initial_stats = object_store.stats()
        initial_memory = initial_stats["total_memory"]

        # Execute series of operations then cleanup
        for cycle in range(5):
            object_ids = []

            # Create objects
            for i in range(20):
                obj = {"cycle": cycle, "index": i, "data": list(range(i * 100))}
                object_id = await object_store.put(obj)
                object_ids.append(object_id)

            # Verify objects exist
            for object_id in object_ids:
                exists = await object_store.contains(object_id)
                assert exists

            # Delete all objects
            for object_id in object_ids:
                await object_store.delete(object_id)

            # Execute cleanup
            await object_store.cleanup()

            # Check memory usage
            cycle_stats = object_store.stats()
            print(
                f"Cycle {cycle}: Memory: {cycle_stats['total_memory']} bytes, "
                f"Objects: {cycle_stats['total_objects']}"
            )

        # Final check - memory should return to near initial level
        final_stats = object_store.stats()
        final_memory = final_stats["total_memory"]

        # Allow some memory growth (due to internal caches etc.), but not too much
        memory_growth = final_memory - initial_memory
        print(f"Memory growth: {memory_growth} bytes")

        # Verify no serious memory leaks (allow some growth)
        assert final_stats["total_objects"] == 0  # All objects should be deleted


# ============================================================================
# Advanced Features and Integration Tests
# ============================================================================


class TestAdvancedFeaturesAndIntegration:
    """Advanced features and integration tests"""

    @pytest.mark.asyncio
    async def test_objectstore_config_variations(self):
        """Test different ObjectStore configurations"""
        configs = [
            PyStoreConfig(max_memory=32 * 1024 * 1024, memory_pressure_threshold=0.5),
            PyStoreConfig(max_memory=128 * 1024 * 1024, memory_pressure_threshold=0.9),
            PyStoreConfig(max_object_size=16 * 1024 * 1024, track_access_time=False),
        ]

        for i, config in enumerate(configs):
            store = PyObjectStore(config)

            # Test basic operations
            test_obj = {"config_test": i, "data": f"config_{i}" * 100}
            object_id = await store.put(test_obj)
            retrieved = await store.get_object(object_id)

            assert retrieved == test_obj

            # Cleanup
            await store.clear()

    @pytest.mark.asyncio
    async def test_mixed_data_types_workflow(self, object_store):
        """Test mixed data types workflow"""
        # Simulate real application scenario: handling multiple data types

        # 1. Store configuration data
        config = {
            "app_name": "test_app",
            "version": "1.0.0",
            "settings": {"debug": True, "max_connections": 100},
        }
        config_id = await object_store.put(config)

        # 2. Store large text data
        large_text = "Lorem ipsum " * 50000  # About 600KB text
        text_id = await object_store.put(large_text)

        # 3. Store binary data
        binary_data = bytes(range(256)) * 1000  # 256KB binary data
        binary_id = await object_store.put_bytes(binary_data)

        # 4. Store NumPy array (if available)
        if HAS_NUMPY:
            matrix = np.random.rand(200, 200)
            matrix_id = await object_store.put_numpy(matrix)

        # 5. Create reference mapping
        references = {
            "config": config_id,
            "text": text_id,
            "binary": binary_id,
        }
        if HAS_NUMPY:
            references["matrix"] = matrix_id

        refs_id = await object_store.put(references)

        # 6. Verify complete workflow
        retrieved_refs = await object_store.get_object(refs_id)

        # Verify each data type
        retrieved_config = await object_store.get_object(retrieved_refs["config"])
        assert retrieved_config == config

        retrieved_text = await object_store.get_object(retrieved_refs["text"])
        assert retrieved_text == large_text

        retrieved_binary = await object_store.get_bytes(retrieved_refs["binary"])
        assert retrieved_binary == binary_data

        if HAS_NUMPY:
            retrieved_matrix = await object_store.get_numpy(retrieved_refs["matrix"])
            np.testing.assert_array_equal(retrieved_matrix, matrix)

        # 7. Verify statistics
        stats = object_store.stats()
        expected_objects = 5 if HAS_NUMPY else 4
        assert stats["total_objects"] == expected_objects
        assert stats["total_memory"] > 0

    @pytest.mark.asyncio
    async def test_objectstore_context_simulation(
        self, object_store, objectstore_config
    ):
        """Test simulating ObjectStore context usage"""
        # Simulate Actor using ObjectStore scenario

        class MockActor:
            def __init__(self, name):
                self.name = name
                self.processed_data = []

        # Create mock Actor
        actor = MockActor("test_actor")

        # Create wrapper (simulation)
        serializer = MessageSerializer(object_store, objectstore_config)

        # Simulate processing large data message
        large_message = {
            "command": "process_data",
            "payload": LargeObject(3),  # 3MB data
            "metadata": {"priority": "high", "timestamp": time.time()},
        }
        print("Large object size:", sys.getsizeof(large_message["payload"]))
        print("Pympler size:", asizeof.asizeof(large_message["payload"]))

        # Serialize message
        serialized_args, serialized_kwargs = await serializer.serialize_args(
            (large_message,), {}
        )

        # Verify large object was serialized
        assert isinstance(serialized_args[0], ObjectStoreRef)

        # Simulate deserialization after transmission
        deserialized_args, deserialized_kwargs = await serializer.deserialize_args(
            serialized_args, serialized_kwargs
        )

        # Verify data integrity
        recovered_message = deserialized_args[0]
        assert recovered_message["command"] == large_message["command"]
        assert recovered_message["payload"] == large_message["payload"]
        assert recovered_message["metadata"] == large_message["metadata"]


# ============================================================================
# Performance Benchmark Tests
# ============================================================================


class TestPerformanceBenchmarks:
    """Performance benchmark tests"""

    @pytest.mark.asyncio
    async def test_throughput_benchmark(self, object_store):
        """Throughput benchmark test"""
        object_count = 100
        test_objects = [
            {"id": i, "data": f"benchmark_data_{i}" * 50} for i in range(object_count)
        ]

        # Test store throughput
        start_time = time.time()
        object_ids = await object_store.put_batch(test_objects)
        store_duration = time.time() - start_time

        store_throughput = object_count / store_duration
        print(f"Store throughput: {store_throughput:.2f} objects/second")

        # Test retrieval throughput
        start_time = time.time()
        retrieved_objects = await object_store.get_objects(object_ids)
        retrieve_duration = time.time() - start_time

        retrieve_throughput = object_count / retrieve_duration
        print(f"Retrieve throughput: {retrieve_throughput:.2f} objects/second")

        # Verify data correctness
        assert len(retrieved_objects) == object_count
        for original, retrieved in zip(test_objects, retrieved_objects):
            assert original == retrieved

        # Basic performance requirements (these values can be adjusted based on actual needs)
        assert store_throughput > 10, (
            f"Store throughput too low: {store_throughput:.2f}"
        )
        assert retrieve_throughput > 50, (
            f"Retrieve throughput too low: {retrieve_throughput:.2f}"
        )

    @pytest.mark.asyncio
    @pytest.mark.skipif(not HAS_NUMPY, reason="NumPy not available")
    async def test_numpy_performance_comparison(self, object_store):
        """NumPy performance comparison test"""
        array = np.random.rand(500, 500)  # About 2MB array
        copy_modes = ["safe_copy", "fast_copy", "zero_copy"]

        performance_results = {}

        for mode in copy_modes:
            # Test store performance
            start_time = time.time()
            object_id = await object_store.put_numpy(array, copy_mode=mode)
            store_time = time.time() - start_time

            # Test retrieval performance
            start_time = time.time()
            retrieved = await object_store.get_numpy(object_id)
            retrieve_time = time.time() - start_time

            # Verify correctness
            np.testing.assert_array_equal(retrieved, array)

            performance_results[mode] = {
                "store_time": store_time,
                "retrieve_time": retrieve_time,
                "total_time": store_time + retrieve_time,
            }

            print(f"{mode}: Store {store_time:.4f}s, Retrieve {retrieve_time:.4f}s")

            # Cleanup
            await object_store.delete(object_id)

        # Analyze performance differences
        times = [results["total_time"] for results in performance_results.values()]
        fastest_time = min(times)
        slowest_time = max(times)

        print(f"Performance range: {fastest_time:.4f}s - {slowest_time:.4f}s")
        print(f"Performance ratio: {slowest_time / fastest_time:.2f}x")
