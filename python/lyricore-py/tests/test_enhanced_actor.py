"""
Pytest test suite for ObjectStore integration with Actor system
"""

import tracemalloc

tracemalloc.start()
import numpy as np
import pytest


@pytest.fixture
def numpy_available():
    """Check if numpy is available"""
    try:
        import numpy as np

        return True
    except ImportError:
        return False


@pytest.fixture
def small_test_data():
    """Small test data that should not be serialized"""
    return {"message": "hello", "number": 42, "list": [1, 2, 3]}


@pytest.fixture
def large_test_array(numpy_available):
    """Large numpy array that should be serialized"""
    if not numpy_available:
        pytest.skip("NumPy not available")
    return np.random.rand(200, 200, 10).astype(np.float32)  # ~16MB


@pytest.fixture
def large_bytes_data():
    """Large bytes data that should be serialized"""
    return b"x" * (5 * 1024 * 1024)  # 5MB


# Import tests
class TestImports:
    """Test that all required modules can be imported"""

    def test_import_objectstore_config(self):
        """Test ObjectStoreConfig import"""
        from lyricore.actor_wrapper import ObjectStoreConfig

        assert ObjectStoreConfig is not None

    def test_import_rust_components(self):
        """Test Rust component imports"""
        from lyricore._lyricore import PyObjectStore, PyStoreConfig

        assert PyObjectStore is not None
        assert PyStoreConfig is not None


# ObjectStore configuration tests
class TestObjectStoreConfig:
    """Test ObjectStore configuration"""

    def test_default_config_creation(self):
        """Test creating ObjectStoreConfig with defaults"""
        from lyricore.actor_wrapper import ObjectStoreConfig

        config = ObjectStoreConfig()
        assert config.auto_serialize_threshold == 1024 * 1024  # 1MB
        assert config.enable_batch_optimization is True
        assert isinstance(config.auto_serialize_patterns, list)
        assert "data" in config.auto_serialize_patterns

    def test_custom_config_creation(self):
        """Test creating ObjectStoreConfig with custom values"""
        from lyricore.actor_wrapper import ObjectStoreConfig

        config = ObjectStoreConfig(
            auto_serialize_threshold=5 * 1024 * 1024,  # 5MB
            enable_batch_optimization=False,
            auto_serialize_patterns=["array", "buffer"],
        )
        assert config.auto_serialize_threshold == 5 * 1024 * 1024
        assert config.enable_batch_optimization is False
        assert config.auto_serialize_patterns == ["array", "buffer"]

    def test_config_auto_serialize_types(self, numpy_available):
        """Test that auto_serialize_types is set correctly"""
        from lyricore.actor_wrapper import ObjectStoreConfig

        config = ObjectStoreConfig()
        assert bytes in config.auto_serialize_types
        assert bytearray in config.auto_serialize_types

        if numpy_available:
            import numpy as np

            assert np.ndarray in config.auto_serialize_types


# ObjectStore creation and basic operations tests
class TestObjectStoreOperations:
    """Test ObjectStore creation and basic operations"""

    @pytest.fixture
    def objectstore(self):
        """Create a test ObjectStore instance"""
        from lyricore._lyricore import PyObjectStore, PyStoreConfig

        config = PyStoreConfig(
            max_memory=128 * 1024 * 1024,  # 128MB
            max_object_size=64 * 1024 * 1024,  # 64MB
            memory_pressure_threshold=0.8,
            track_access_time=True,
        )
        return PyObjectStore(config)

    def test_objectstore_creation(self, objectstore):
        """Test ObjectStore creation"""
        assert objectstore is not None

        # Test stats access
        stats = objectstore.stats()
        assert isinstance(stats, dict)
        assert "total_objects" in stats
        assert stats["total_objects"] == 0

    @pytest.mark.asyncio
    async def test_store_retrieve_object(self, objectstore, small_test_data):
        """Test storing and retrieving a Python object"""
        # Store object
        object_id = await objectstore.put(small_test_data)
        assert isinstance(object_id, str)
        assert len(object_id) > 0

        # Retrieve object
        retrieved_data = await objectstore.get_object(object_id)
        assert retrieved_data == small_test_data

        # Check stats
        stats = objectstore.stats()
        assert stats["total_objects"] == 1

    @pytest.mark.asyncio
    async def test_store_retrieve_bytes(self, objectstore, large_bytes_data):
        """Test storing and retrieving bytes data"""
        # Store bytes
        object_id = await objectstore.put_bytes(large_bytes_data)
        assert isinstance(object_id, str)

        # Retrieve bytes
        retrieved_bytes = await objectstore.get_bytes(object_id)
        assert retrieved_bytes == large_bytes_data
        assert len(retrieved_bytes) == len(large_bytes_data)

    @pytest.mark.asyncio
    async def test_store_retrieve_numpy(self, objectstore, large_test_array):
        """Test storing and retrieving NumPy arrays"""
        # Store array
        object_id = await objectstore.put_numpy(large_test_array)
        assert isinstance(object_id, str)

        # Retrieve array
        retrieved_array = await objectstore.get_numpy(object_id)
        assert retrieved_array.shape == large_test_array.shape
        assert retrieved_array.dtype == large_test_array.dtype
        np.testing.assert_array_equal(retrieved_array, large_test_array)

    @pytest.mark.asyncio
    async def test_object_exists_and_delete(self, objectstore, small_test_data):
        """Test object existence check and deletion"""
        # Store object
        object_id = await objectstore.put(small_test_data)

        # Check existence
        exists = await objectstore.contains(object_id)
        assert exists is True

        # Delete object
        deleted = await objectstore.delete(object_id)
        assert deleted is True

        # Check non-existence
        exists_after = await objectstore.contains(object_id)
        assert exists_after is False

        # Try to delete again
        deleted_again = await objectstore.delete(object_id)
        assert deleted_again is False


# Message serialization tests
class TestMessageSerialization:
    """Test message serialization logic"""

    @pytest.fixture
    def serializer(self):
        """Create a MessageSerializer instance"""
        from lyricore._lyricore import PyObjectStore, PyStoreConfig
        from lyricore.actor_wrapper import MessageSerializer, ObjectStoreConfig

        store_config = PyStoreConfig(max_memory=64 * 1024 * 1024)
        store = PyObjectStore(store_config)

        objectstore_config = ObjectStoreConfig(
            auto_serialize_threshold=1024 * 1024  # 1MB
        )

        return MessageSerializer(store, objectstore_config)

    def test_should_serialize_small_object(self, serializer, small_test_data):
        """Test that small objects are not serialized"""
        should_serialize = serializer._should_serialize(small_test_data)
        assert should_serialize is False

    def test_should_serialize_large_array(self, serializer, large_test_array):
        """Test that large arrays are serialized"""
        should_serialize = serializer._should_serialize(large_test_array)
        assert should_serialize is True

        # Check size calculation
        assert large_test_array.nbytes > serializer.config.auto_serialize_threshold

    def test_should_serialize_large_bytes(self, serializer, large_bytes_data):
        """Test that large bytes data is serialized"""
        should_serialize = serializer._should_serialize(large_bytes_data)
        assert should_serialize is True

        assert len(large_bytes_data) > serializer.config.auto_serialize_threshold

    def test_should_serialize_by_param_name(self, serializer):
        """Test serialization based on parameter name patterns"""
        # Small data that normally wouldn't be serialized
        small_data = b"x" * 1000  # 1KB

        # Should not serialize normally
        assert serializer._should_serialize(small_data) is False

        # Should serialize when param name matches pattern
        assert (
            serializer._should_serialize(small_data, param_name="data_buffer") is False
        )  # Still too small

        # Larger data with matching param name
        medium_data = b"x" * (512 * 1024)  # 512KB (under threshold but has pattern)
        assert (
            serializer._should_serialize(medium_data, param_name="array_data") is False
        )  # Under threshold

    @pytest.mark.asyncio
    async def test_serialize_deserialize_args(
        self, serializer, small_test_data, large_test_array
    ):
        """Test argument serialization and deserialization"""
        args = (small_test_data, large_test_array)
        kwargs = {"small": small_test_data, "large": large_test_array}

        # Serialize
        serialized_args, serialized_kwargs = await serializer.serialize_args(
            args, kwargs
        )

        # Check that small data is unchanged
        assert serialized_args[0] == small_test_data
        assert serialized_kwargs["small"] == small_test_data

        # Check that large data is converted to ObjectStoreRef
        from lyricore.actor_wrapper import ObjectStoreRef

        assert isinstance(serialized_args[1], ObjectStoreRef)
        assert isinstance(serialized_kwargs["large"], ObjectStoreRef)

        # Deserialize
        deserialized_args, deserialized_kwargs = await serializer.deserialize_args(
            serialized_args, serialized_kwargs
        )

        # Check that everything is restored
        assert deserialized_args[0] == small_test_data
        assert deserialized_kwargs["small"] == small_test_data

        # Check array restoration
        np.testing.assert_array_equal(deserialized_args[1], large_test_array)
        np.testing.assert_array_equal(deserialized_kwargs["large"], large_test_array)


# Performance and edge case tests
class TestEdgeCases:
    """Test edge cases and error conditions"""

    @pytest.mark.asyncio
    async def test_objectstore_ref_caching(self):
        """Test that ObjectStoreRef caches loaded objects"""
        from lyricore._lyricore import PyObjectStore, PyStoreConfig
        from lyricore.actor_wrapper import ObjectStoreRef

        # Create store and store an object
        store_config = PyStoreConfig(max_memory=64 * 1024 * 1024)
        store = PyObjectStore(store_config)

        test_data = {"cached": "data"}
        object_id = await store.put(test_data)

        # Create ObjectStoreRef
        ref = ObjectStoreRef(object_id, store, {"type": "dict"})
        assert not ref._is_loaded

        # First access
        data1 = await ref.get()
        assert ref._is_loaded
        assert data1 == test_data

        # Second access (should use cache)
        data2 = await ref.get()
        assert data2 == test_data
        assert data1 is data2  # Should be the same cached object

    @pytest.mark.asyncio
    async def test_serialization_fallback(self, numpy_available):
        """Test that serialization gracefully handles errors"""
        from lyricore._lyricore import PyObjectStore, PyStoreConfig
        from lyricore.actor_wrapper import MessageSerializer, ObjectStoreConfig

        # Create serializer
        store_config = PyStoreConfig(max_memory=64 * 1024 * 1024)
        store = PyObjectStore(store_config)
        objectstore_config = ObjectStoreConfig(
            auto_serialize_threshold=100
        )  # Very low threshold
        serializer = MessageSerializer(store, objectstore_config)

        # Test with non-serializable object
        class NonSerializable:
            def __reduce__(self):
                raise TypeError("Cannot serialize this object")

        obj = NonSerializable()
        # Should not raise an exception, should return original object
        result = await serializer._serialize_value(obj)
        assert result is obj  # Should return original object on serialization failure

    def test_config_validation(self):
        """Test configuration validation"""
        from lyricore.actor_wrapper import ObjectStoreConfig

        # Test with invalid patterns
        config = ObjectStoreConfig(auto_serialize_patterns=[])
        assert config.auto_serialize_patterns == []

        # Test with very low threshold
        config = ObjectStoreConfig(auto_serialize_threshold=1)
        assert config.auto_serialize_threshold == 1
