from typing import Any

import pytest

from lyricore import ActorContext, ActorSystem
from lyricore.tests.conftest import actor_system


@pytest.mark.asyncio
async def test_local_class_basic(actor_system: ActorSystem):
    """Test basic local class support"""

    class Counter:
        def __init__(self, cnt):
            self.count = cnt

        async def handle_message(self, message: Any, ctx: ActorContext) -> str:
            if message == "increment":
                self.count += 1
                return f"Count incremented to {self.count}"
            elif message == "get_count":
                return f"Current count is {self.count}"
            else:
                return "Unknown command"

    print("Testing local class support...")
    print(f"Class module: {Counter.__module__}")
    print(f"Class qualname: {Counter.__qualname__}")
    print(f"Is local class: {'.<locals>.' in Counter.__qualname__}")

    # This should now work properly
    counter_actor = await actor_system.spawn(Counter, "counter_actor", 5)

    # Test functionality
    response = await counter_actor.ask("get_count")
    assert response == "Current count is 5"

    response = await counter_actor.ask("increment")
    assert response == "Count incremented to 6"

    response = await counter_actor.ask("get_count")
    assert response == "Current count is 6"


@pytest.mark.asyncio
async def test_local_class_with_closure(actor_system: ActorSystem):
    """Test local class with closure"""

    multiplier = 3  # Outer variable
    base_name = "TestActor"  # Another outer variable

    class MultiplierActor:
        def __init__(self, base=1):
            super().__init__()
            self.base = base
            self.multiplier = multiplier  # Capture outer variable
            self.name = base_name  # Capture another outer variable

        async def handle_message(self, message: Any, ctx: ActorContext) -> Any:
            if isinstance(message, (int, float)):
                result = message * self.base * self.multiplier
                return f"{self.name}: {result}"
            elif message == "get_info":
                return f"{self.name} - base: {self.base}, multiplier: {self.multiplier}"
            return "Unknown message"

    # Create Actor
    actor = await actor_system.spawn(MultiplierActor, "multiplier", 2)

    # Test basic calculation
    result = await actor.ask(5)  # 5 * 2 * 3 = 30
    assert result == "TestActor: 30"

    # Test info retrieval
    info = await actor.ask("get_info")
    assert info == "TestActor - base: 2, multiplier: 3"


@pytest.mark.asyncio
async def test_local_class_with_imports(actor_system: ActorSystem):
    """Test local class using external modules"""

    import json
    import time
    from datetime import datetime

    class JsonActor:
        def __init__(self):
            super().__init__()
            self.created_at = datetime.now()

        async def handle_message(self, message: Any, ctx: ActorContext) -> Any:
            if message == "get_time":
                return {
                    "current_time": time.time(),
                    "created_at": self.created_at.isoformat(),
                }
            elif isinstance(message, dict):
                # Use imported json module
                return json.dumps(message, indent=2)
            return "Unknown message"

    actor = await actor_system.spawn(JsonActor, "json_actor")

    # Test time retrieval
    time_info = await actor.ask("get_time")
    assert isinstance(time_info, dict)
    assert "current_time" in time_info
    assert "created_at" in time_info

    # Test JSON serialization
    test_data = {"name": "test", "value": 42}
    json_result = await actor.ask(test_data)
    assert isinstance(json_result, str)
    assert "test" in json_result
    assert "42" in json_result


@pytest.mark.asyncio
async def test_nested_local_class(actor_system: ActorSystem):
    """Test nested local class definition"""

    def create_calculator_class(operation_name: str):
        """Factory function to dynamically create calculator class"""

        class Calculator:
            def __init__(self, initial_value=0):
                super().__init__()
                self.value = initial_value
                self.operation = operation_name  # Capture parameter
                print(
                    f"Creating {self.operation} calculator with initial value: {self.value}"
                )

            async def handle_message(self, message: Any, ctx: ActorContext) -> Any:
                print(
                    f"Handle message in {self.operation} calculator: {message}, current value: {self.value}"
                )
                if isinstance(message, (int, float)):
                    if self.operation == "add":
                        self.value += message
                    elif self.operation == "multiply":
                        self.value *= message
                    return self.value
                elif message == "get_value":
                    return self.value
                elif message == "reset":
                    self.value = 0
                    return "Reset"
                return "Unknown operation"

        return Calculator

    # Create two different calculator classes
    AdderClass = create_calculator_class("add")
    MultiplierClass = create_calculator_class("multiply")

    # Create Actor instances
    adder = await actor_system.spawn(AdderClass, "adder", 10)
    multiplier = await actor_system.spawn(MultiplierClass, "multiplier", 1)

    # Test adder
    result = await adder.ask(5)  # 10 + 5 = 15
    assert result == 15

    result = await adder.ask(3)  # 15 + 3 = 18
    assert result == 18

    # Test multiplier
    result = await multiplier.ask(4)  # 1 * 4 = 4
    assert result == 4

    result = await multiplier.ask(3)  # 4 * 3 = 12
    assert result == 12


@pytest.mark.asyncio
async def test_module_level_class_still_works(actor_system: ActorSystem):
    """Ensure module-level classes still work properly"""

    # This class is defined at module level, should take the fast path
    counter_actor = await actor_system.spawn(GlobalCounter, "global_counter", 100)

    response = await counter_actor.ask("get_count")
    assert response == "Current count is 100"

    response = await counter_actor.ask("increment")
    assert response == "Count incremented to 101"


# Module-level test class
class GlobalCounter:
    def __init__(self, cnt=0):
        super().__init__()
        self.count = cnt

    async def handle_message(self, message: Any, ctx: ActorContext) -> str:
        if message == "increment":
            self.count += 1
            return f"Count incremented to {self.count}"
        elif message == "get_count":
            return f"Current count is {self.count}"
        else:
            return "Unknown command"


@pytest.mark.asyncio
async def test_distributed_scenario_simulation(actor_system: ActorSystem):
    """Simulate distributed scenario: serialization and transmission test"""

    multiplier = 7  # Simulate closure variable

    class DistributedActor:
        def __init__(self, node_name: str):
            super().__init__()
            self.node_name = node_name
            self.multiplier = multiplier
            self.processed_count = 0

        async def handle_message(self, message: Any, ctx: ActorContext) -> Any:
            self.processed_count += 1

            if isinstance(message, (int, float)):
                result = message * self.multiplier
                return {
                    "node": self.node_name,
                    "result": result,
                    "processed_count": self.processed_count,
                }
            elif message == "status":
                return {
                    "node": self.node_name,
                    "multiplier": self.multiplier,
                    "processed_count": self.processed_count,
                }
            return "Unknown message"

    # Simulate creating Actors on "different nodes"
    actor1 = await actor_system.spawn(DistributedActor, "node1_actor", "Node-A")
    actor2 = await actor_system.spawn(DistributedActor, "node2_actor", "Node-B")

    # Test that both Actors work properly
    result1 = await actor1.ask(10)  # 10 * 7 = 70
    assert result1["node"] == "Node-A"
    assert result1["result"] == 70
    assert result1["processed_count"] == 1

    result2 = await actor2.ask(5)  # 5 * 7 = 35
    assert result2["node"] == "Node-B"
    assert result2["result"] == 35
    assert result2["processed_count"] == 1

    # Verify state independence
    status1 = await actor1.ask("status")
    status2 = await actor2.ask("status")

    assert status1["node"] == "Node-A"
    assert status2["node"] == "Node-B"
    assert status1["multiplier"] == status2["multiplier"] == 7
    assert status1["processed_count"] == 2  # Processed calculation and status query
    assert status2["processed_count"] == 2
