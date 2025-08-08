import asyncio
import datetime
import uuid
from dataclasses import dataclass
from enum import Enum
from typing import Any

import pytest

from lyricore import ActorContext, ActorSystem
from lyricore.tests.conftest import actor_system


# Test data type definitions
@dataclass
class UserData:
    """Test data class"""

    name: str
    age: int
    email: str

    def to_dict(self):
        return {"name": self.name, "age": self.age, "email": self.email}


class Status(Enum):
    """Test enum"""

    ACTIVE = "active"
    INACTIVE = "inactive"
    PENDING = "pending"


class ComplexData:
    """Complex object type"""

    def __init__(self, data: dict):
        self.data = data
        self.timestamp = datetime.datetime.now()
        self.id = str(uuid.uuid4())

    def __eq__(self, other):
        if not isinstance(other, ComplexData):
            return False
        return self.data == other.data


class MessageTypeTestActor:
    """Actor specifically for testing various message types"""

    def __init__(self):
        super().__init__()
        self.received_messages = []
        self.message_count = 0

    async def handle_message(self, message: Any, ctx: ActorContext) -> Any:
        """Handle various types of messages"""
        self.message_count += 1
        self.received_messages.append(message)

        # Handle different message types accordingly
        if isinstance(message, str):
            return self._handle_string_message(message)
        elif isinstance(message, float):
            return self._handle_float_message(message)
        elif isinstance(message, bool):
            return self._handle_bool_message(message)
        elif isinstance(message, int):
            return self._handle_int_message(message)
        elif isinstance(message, list):
            return self._handle_list_message(message)
        elif isinstance(message, dict):
            return self._handle_dict_message(message)
        elif isinstance(message, tuple):
            return self._handle_tuple_message(message)
        elif isinstance(message, set):
            return self._handle_set_message(message)
        elif isinstance(message, bytes):
            return self._handle_bytes_message(message)
        elif message is None:
            return self._handle_none_message()
        elif isinstance(message, UserData):
            return self._handle_userdata_message(message)
        elif isinstance(message, Status):
            return self._handle_enum_message(message)
        elif isinstance(message, ComplexData):
            return self._handle_complex_message(message)
        elif hasattr(message, "__dict__"):
            # Handle custom objects
            return self._handle_custom_object_message(message)
        else:
            return f"Unknown message type: {type(message).__name__}"

    def _handle_string_message(self, message: str) -> str:
        """Handle string messages"""
        if message == "get_stats":
            return {
                "message_count": self.message_count,
                "received_types": [
                    type(msg).__name__ for msg in self.received_messages
                ],
            }
        elif message.startswith("echo:"):
            return message[5:]  # Return content after echo:
        elif message.startswith("reverse:"):
            return message[8:][::-1]  # Return reversed string
        else:
            return f"String received: '{message}' (length: {len(message)})"

    def _handle_int_message(self, message: int) -> dict:
        """Handle integer messages"""
        return {
            "type": "integer",
            "value": message,
            "is_positive": message > 0,
            "is_even": message % 2 == 0,
            "square": message**2,
        }

    def _handle_float_message(self, message: float) -> dict:
        """Handle float messages"""
        return {
            "type": "float",
            "value": message,
            "rounded": round(message, 2),
            "is_integer": message.is_integer(),
        }

    def _handle_bool_message(self, message: bool) -> dict:
        """Handle boolean messages"""
        return {
            "type": "boolean",
            "value": message,
            "negated": not message,
            "as_string": str(message).lower(),
        }

    def _handle_list_message(self, message: list) -> dict:
        """Handle list messages"""
        return {
            "type": "list",
            "length": len(message),
            "items": message,
            "first": message[0] if message else None,
            "last": message[-1] if message else None,
            "sum": sum(x for x in message if isinstance(x, (int, float, bool))),
            "types": [type(item).__name__ for item in message],
        }

    def _handle_dict_message(self, message: dict) -> dict:
        """Handle dictionary messages"""
        return {
            "type": "dictionary",
            "keys": list(message.keys()),
            "values": list(message.values()),
            "size": len(message),
            "has_nested": any(isinstance(v, (dict, list)) for v in message.values()),
            "processed_data": message,
        }

    def _handle_tuple_message(self, message: tuple) -> dict:
        """Handle tuple messages"""
        return {
            "type": "tuple",
            "length": len(message),
            "items": list(message),  # Convert to list for JSON serialization
            "first": message[0] if message else None,
            "last": message[-1] if message else None,
        }

    def _handle_set_message(self, message: set) -> dict:
        """Handle set messages"""
        return {
            "type": "set",
            "size": len(message),
            "items": list(message),  # Convert to list
            "is_empty": len(message) == 0,
        }

    def _handle_bytes_message(self, message: bytes) -> dict:
        """Handle bytes messages"""
        try:
            decoded = message.decode("utf-8")
        except UnicodeDecodeError:
            decoded = str(message)

        return {
            "type": "bytes",
            "length": len(message),
            "decoded": decoded,
            "hex": message.hex(),
        }

    def _handle_none_message(self) -> dict:
        """Handle None messages"""
        return {"type": "none", "value": None, "is_none": True}

    def _handle_userdata_message(self, message: UserData) -> dict:
        """Handle custom data class messages"""
        return {
            "type": "UserData",
            "name": message.name,
            "age": message.age,
            "email": message.email,
            "is_adult": message.age >= 18,
            "data_dict": message.to_dict(),
        }

    def _handle_enum_message(self, message: Status) -> dict:
        """Handle enum messages"""
        return {
            "type": "Status",
            "value": message.value,
            "name": message.name,
            "is_active": message == Status.ACTIVE,
        }

    def _handle_complex_message(self, message: ComplexData) -> dict:
        """Handle complex object messages"""
        return {
            "type": "ComplexData",
            "data": message.data,
            "id": message.id,
            "timestamp": message.timestamp.isoformat(),
            "data_keys": list(message.data.keys())
            if isinstance(message.data, dict)
            else [],
        }

    def _handle_custom_object_message(self, message: Any) -> dict:
        """Handle other custom objects"""
        return {
            "type": "custom_object",
            "class_name": type(message).__name__,
            "attributes": {k: str(v) for k, v in message.__dict__.items()},
            "has_dict": hasattr(message, "__dict__"),
        }


@pytest.mark.asyncio
class TestMessageTypes:
    """Test message handling for different data types"""

    async def test_string_messages(self, actor_system: ActorSystem):
        """Test string messages"""
        actor = await actor_system.spawn(MessageTypeTestActor, "string_test_actor")

        # Test regular string
        result = await actor.ask("hello world")
        assert "String received: 'hello world'" in result
        assert "length: 11" in result

        # Test echo functionality
        echo_result = await actor.ask("echo:test message")
        assert echo_result == "test message"

        # Test reverse functionality
        reverse_result = await actor.ask("reverse:hello")
        assert reverse_result == "olleh"

        # Test empty string
        empty_result = await actor.ask("")
        assert "length: 0" in empty_result

        # Test Unicode string
        unicode_result = await actor.ask("Hello World")
        assert "Hello World" in unicode_result

        # Use tell to send message (no response expected)
        await actor.tell("silent message")

        # Verify message was received
        stats = await actor.ask("get_stats")
        assert stats["message_count"] >= 6  # At least 6 messages received

    async def test_numeric_messages(self, actor_system: ActorSystem):
        """Test numeric type messages"""
        actor = await actor_system.spawn(MessageTypeTestActor, "numeric_test_actor")

        # Test positive integer
        result = await actor.ask(42)
        assert result["type"] == "integer"
        assert result["value"] == 42
        assert result["is_positive"] is True
        assert result["is_even"] is True
        assert result["square"] == 1764

        # Test negative integer
        result = await actor.ask(-7)
        assert result["value"] == -7
        assert result["is_positive"] is False
        assert result["is_even"] is False

        # Test zero
        result = await actor.ask(0)
        assert result["value"] == 0
        assert result["is_positive"] is False
        assert result["is_even"] is True

        # Test float
        result = await actor.ask(3.14159)
        assert result["type"] == "float"
        assert result["value"] == 3.14159
        assert result["rounded"] == 3.14
        assert result["is_integer"] is False

        # Test integer float
        result = await actor.ask(5.0)
        assert result["is_integer"] is True

    async def test_boolean_messages(self, actor_system: ActorSystem):
        """Test boolean messages"""
        actor = await actor_system.spawn(MessageTypeTestActor, "bool_test_actor")

        # Test True
        result = await actor.ask(True)
        assert result["type"] == "boolean"
        assert result["value"] is True
        assert result["negated"] is False
        assert result["as_string"] == "true"

        # Test False
        result = await actor.ask(False)
        assert result["value"] is False
        assert result["negated"] is True
        assert result["as_string"] == "false"

    async def test_collection_messages(self, actor_system: ActorSystem):
        """Test collection type messages"""
        actor = await actor_system.spawn(MessageTypeTestActor, "collection_test_actor")

        # Test list
        test_list = [1, 2, "three", 4.5, True]
        result = await actor.ask(test_list)
        assert result["type"] == "list"
        assert result["length"] == 5
        assert result["items"] == test_list
        assert result["first"] == 1
        assert result["last"] is True
        assert result["sum"] == 8.5  # 1 + 2 + 4.5 + 1(True)
        assert "int" in result["types"]
        assert "str" in result["types"]

        # Test empty list
        empty_result = await actor.ask([])
        assert empty_result["length"] == 0
        assert empty_result["first"] is None
        assert empty_result["sum"] == 0

        # Test dictionary
        test_dict = {
            "name": "Alice",
            "age": 30,
            "skills": ["Python", "Rust"],
            "active": True,
        }
        result = await actor.ask(test_dict)
        assert result["type"] == "dictionary"
        assert set(result["keys"]) == {"name", "age", "skills", "active"}
        assert result["size"] == 4
        assert result["has_nested"] is True  # skills is a list
        assert result["processed_data"] == test_dict

        # Test tuple
        test_tuple = (1, "two", 3.0)
        result = await actor.ask(test_tuple)
        assert result["type"] == "tuple"
        assert result["length"] == 3
        assert result["items"] == [1, "two", 3.0]
        assert result["first"] == 1
        assert result["last"] == 3.0

        # Test set
        test_set = {1, 2, 3, 2, 1}  # Duplicate elements will be removed
        result = await actor.ask(test_set)
        assert result["type"] == "set"
        assert result["size"] == 3  # Only 3 elements after deduplication
        assert set(result["items"]) == {1, 2, 3}
        assert result["is_empty"] is False

        # Test empty set
        empty_set_result = await actor.ask(set())
        assert empty_set_result["is_empty"] is True

    async def test_special_types(self, actor_system: ActorSystem):
        """Test special type messages"""
        actor = await actor_system.spawn(MessageTypeTestActor, "special_test_actor")

        # Test None
        result = await actor.ask(None)
        assert result["type"] == "none"
        assert result["value"] is None
        assert result["is_none"] is True

        # Test bytes
        test_bytes = b"Hello, bytes!"
        result = await actor.ask(test_bytes)
        assert result["type"] == "bytes"
        assert result["length"] == len(test_bytes)
        assert result["decoded"] == "Hello, bytes!"
        assert result["hex"] == test_bytes.hex()

        # Test binary data
        binary_data = bytes([0x48, 0x65, 0x6C, 0x6C, 0x6F])  # "Hello"
        result = await actor.ask(binary_data)
        assert result["decoded"] == "Hello"

    async def test_custom_dataclass(self, actor_system: ActorSystem):
        """Test custom data class"""
        actor = await actor_system.spawn(MessageTypeTestActor, "dataclass_test_actor")

        # Create test user data
        user = UserData(name="Alice", age=25, email="alice@example.com")

        result = await actor.ask(user)
        assert result["type"] == "UserData"
        assert result["name"] == "Alice"
        assert result["age"] == 25
        assert result["email"] == "alice@example.com"
        assert result["is_adult"] is True
        assert result["data_dict"] == user.to_dict()

        # Test minor user
        minor = UserData(name="Bob", age=16, email="bob@example.com")
        result = await actor.ask(minor)
        assert result["is_adult"] is False

    async def test_enum_messages(self, actor_system: ActorSystem):
        """Test enum type messages"""
        actor = await actor_system.spawn(MessageTypeTestActor, "enum_test_actor")

        # Test different enum values
        for status in Status:
            result = await actor.ask(status)
            assert result["type"] == "Status"
            assert result["value"] == status.value
            assert result["name"] == status.name
            assert result["is_active"] == (status == Status.ACTIVE)

    async def test_complex_objects(self, actor_system: ActorSystem):
        """Test complex object messages"""
        actor = await actor_system.spawn(MessageTypeTestActor, "complex_test_actor")

        # Create complex object
        complex_data = ComplexData(
            {
                "users": [{"name": "Alice"}, {"name": "Bob"}],
                "settings": {"theme": "dark", "notifications": True},
                "metadata": {"version": "1.0", "created": "2024-01-01"},
            }
        )

        result = await actor.ask(complex_data)
        assert result["type"] == "ComplexData"
        assert result["data"] == complex_data.data
        assert result["id"] == complex_data.id
        assert "timestamp" in result
        assert set(result["data_keys"]) == {"users", "settings", "metadata"}

    async def test_nested_data_structures(self, actor_system: ActorSystem):
        """Test nested data structures"""
        actor = await actor_system.spawn(MessageTypeTestActor, "nested_test_actor")

        # Create deeply nested data structure
        nested_data = {
            "level1": {
                "level2": {
                    "level3": {"data": [1, 2, 3], "info": "deep nesting"},
                    "another_list": [
                        {"item": 1},
                        {"item": 2, "nested": {"value": "test"}},
                    ],
                },
                "simple_value": 42,
            },
            "top_level_list": [
                {"name": "item1"},
                {"name": "item2", "details": {"count": 5}},
            ],
        }

        result = await actor.ask(nested_data)
        assert result["type"] == "dictionary"
        assert result["has_nested"] is True
        assert result["processed_data"] == nested_data
        assert "level1" in result["keys"]
        assert "top_level_list" in result["keys"]

    async def test_mixed_message_sequence(self, actor_system: ActorSystem):
        """Test mixed message sequence"""
        actor = await actor_system.spawn(MessageTypeTestActor, "sequence_test_actor")

        # Send a series of different message types
        messages = [
            "hello",
            42,
            [1, 2, 3],
            {"key": "value"},
            True,
            None,
            3.14,
            b"bytes",
            UserData("Test", 30, "test@test.com"),
            Status.ACTIVE,
        ]

        results = []
        for msg in messages:
            if isinstance(msg, str) and msg == "hello":
                # Use tell to send the first message
                await actor.tell(msg)
            else:
                # Use ask to send other messages
                result = await actor.ask(msg)
                results.append(result)

        # Verify all message types were received
        stats = await actor.ask("get_stats")
        assert stats["message_count"] >= len(messages)

        # Verify received types include the various types we sent
        received_types = stats["received_types"]
        expected_types = {
            "str",
            "int",
            "list",
            "dict",
            "bool",
            "NoneType",
            "float",
            "bytes",
            "UserData",
            "Status",
        }
        actual_types = set(received_types)

        # Check if most expected types are included
        common_types = expected_types.intersection(actual_types)
        assert len(common_types) >= 7  # At least 7 types included

    async def test_large_data_messages(self, actor_system: ActorSystem):
        """Test large data messages"""
        actor = await actor_system.spawn(MessageTypeTestActor, "large_data_test_actor")

        # Create large list
        large_list = list(range(10000))
        result = await actor.ask(large_list)
        assert result["type"] == "list"
        assert result["length"] == 10000
        assert result["first"] == 0
        assert result["last"] == 9999
        assert result["sum"] == sum(large_list)

        # Create large dictionary
        large_dict = {f"key_{i}": f"value_{i}" for i in range(1000)}
        result = await actor.ask(large_dict)
        assert result["type"] == "dictionary"
        assert result["size"] == 1000
        assert "key_0" in result["keys"]
        assert "key_999" in result["keys"]

        # Create large string
        large_string = "x" * 50000
        result = await actor.ask(large_string)
        assert "length: 50000" in result

    async def test_unicode_and_special_characters(self, actor_system: ActorSystem):
        """Test Unicode and special characters"""
        actor = await actor_system.spawn(MessageTypeTestActor, "unicode_test_actor")

        # Test various Unicode characters
        unicode_messages = [
            "Hello, World!",  # English
            "Здравствуй, мир!",  # Russian
            "こんにちは、世界！",  # Japanese
            "🌍🌎🌏 Emoji test 🚀🎉",  # Emoji
            "Special chars: !@#$%^&*()_+-=[]{}|;':\"<>?,./",  # Special characters
            "Math symbols: ∑∏∆√∞±≤≥≠",  # Mathematical symbols
        ]

        for msg in unicode_messages:
            result = await actor.ask(msg)
            assert msg in result
            assert f"length: {len(msg)}" in result

    async def test_error_handling(self, actor_system: ActorSystem):
        """Test error handling scenarios"""
        actor = await actor_system.spawn(MessageTypeTestActor, "error_test_actor")

        # These should all be handled properly without throwing exceptions
        try:
            # Test various edge cases
            await actor.ask(float("inf"))  # Infinity
            await actor.ask(float("-inf"))  # Negative infinity
            # Note: float('nan') may not serialize properly, so we skip this test

            # Test very large numbers
            await actor.ask(2**63 - 1)  # Maximum 64-bit integer

            # Test empty data structures
            await actor.ask([])
            await actor.ask({})
            await actor.ask(())
            await actor.ask(set())

            # All of these should be handled properly
            assert True

        except Exception as e:
            pytest.fail(f"Error handling test failed with exception: {e}")


@pytest.mark.asyncio
class TestMessageTypePerformance:
    """Test performance of message types"""

    async def test_message_throughput_by_type(self, actor_system: ActorSystem):
        """Test throughput for different message types"""
        actor = await actor_system.spawn(MessageTypeTestActor, "performance_test_actor")

        # Test processing speed for different message types
        test_cases = [
            ("string", "test_string"),
            ("integer", 42),
            ("list", [1, 2, 3, 4, 5]),
            ("dict", {"key1": "value1", "key2": "value2"}),
            ("userdata", UserData("Test", 25, "test@test.com")),
        ]

        import time

        for type_name, message in test_cases:
            start_time = time.time()

            # Send 100 messages of the same type
            tasks = []
            for _ in range(100):
                tasks.append(actor.ask(message))

            await asyncio.gather(*tasks)

            end_time = time.time()
            duration = end_time - start_time
            throughput = 100 / duration

            print(f"{type_name} throughput: {throughput:.2f} messages/second")

            # Basic performance assertion - should complete within reasonable time
            assert duration < 10.0, (
                f"{type_name} messages took too long: {duration:.2f}s"
            )
