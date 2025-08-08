from typing import Any

import pytest

from lyricore import ActorContext, ActorSystem
from lyricore.tests.conftest import actor_system


@pytest.mark.asyncio
async def test_nested_local_class_with_function_serialization(
    actor_system: ActorSystem,
):
    """Test nested local class with function serialization version - should solve closure issues"""

    def create_calculator_class(operation_name: str):
        """Factory function to dynamically create calculator class"""

        class Calculator:
            def __init__(self, initial_value=0):
                super().__init__()
                self.value = initial_value
                self.operation = operation_name  # Capture closure variable
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

    # Create two different calculator classes (containing different closure variables)
    AdderClass = create_calculator_class("add")
    MultiplierClass = create_calculator_class("multiply")

    print("=== Testing Function Serialization Approach ===")
    print(f"AdderClass module: {AdderClass.__module__}")
    print(f"AdderClass qualname: {AdderClass.__qualname__}")
    print(f"MultiplierClass module: {MultiplierClass.__module__}")
    print(f"MultiplierClass qualname: {MultiplierClass.__qualname__}")

    # Create Actor instances - now using function serialization
    adder = await actor_system.spawn(AdderClass, "adder", 10)
    multiplier = await actor_system.spawn(MultiplierClass, "multiplier", 1)

    # Test adder
    print("\n=== Testing Adder ===")
    result = await adder.ask(5)  # 10 + 5 = 15
    print(f"Adder result: {result}")
    assert result == 15

    result = await adder.ask(3)  # 15 + 3 = 18
    print(f"Adder result: {result}")
    assert result == 18

    # Test multiplier
    print("\n=== Testing Multiplier ===")
    result = await multiplier.ask(4)  # 1 * 4 = 4
    print(f"Multiplier result: {result}")
    assert result == 4  # This should be correct now!

    result = await multiplier.ask(3)  # 4 * 3 = 12
    print(f"Multiplier result: {result}")
    assert result == 12


@pytest.mark.asyncio
async def test_complex_closure_capture(actor_system: ActorSystem):
    """Test complex closure variable capture"""

    # Complex closure environment
    base_multiplier = 2
    operation_configs = {
        "add": {"symbol": "+", "neutral": 0},
        "multiply": {"symbol": "*", "neutral": 1},
    }

    def create_advanced_calculator(operation_type: str, extra_factor: int):
        """Create advanced calculator with multi-level closure variables"""

        config = operation_configs[operation_type]

        class AdvancedCalculator:
            def __init__(self, initial_value=None):
                super().__init__()
                # Use closure variables
                self.value = (
                    initial_value if initial_value is not None else config["neutral"]
                )
                self.operation = operation_type
                self.symbol = config["symbol"]
                self.base_multiplier = base_multiplier
                self.extra_factor = extra_factor

                print(f"Creating {self.operation} calculator:")
                print(f"  Initial value: {self.value}")
                print(f"  Symbol: {self.symbol}")
                print(f"  Base multiplier: {self.base_multiplier}")
                print(f"  Extra factor: {self.extra_factor}")

            async def handle_message(self, message: Any, ctx: ActorContext) -> Any:
                if isinstance(message, (int, float)):
                    if self.operation == "add":
                        # Complex addition: (value + message) * base_multiplier + extra_factor
                        self.value = (
                            self.value + message
                        ) * self.base_multiplier + self.extra_factor
                    elif self.operation == "multiply":
                        # Complex multiplication: (value * message) * base_multiplier + extra_factor
                        self.value = (
                            self.value * message
                        ) * self.base_multiplier + self.extra_factor

                    return {
                        "value": self.value,
                        "operation": self.operation,
                        "symbol": self.symbol,
                        "applied_factors": {
                            "base_multiplier": self.base_multiplier,
                            "extra_factor": self.extra_factor,
                        },
                    }
                elif message == "get_info":
                    return {
                        "value": self.value,
                        "operation": self.operation,
                        "symbol": self.symbol,
                        "base_multiplier": self.base_multiplier,
                        "extra_factor": self.extra_factor,
                    }
                return "Unknown operation"

        return AdvancedCalculator

    # Create two advanced calculators
    AdvancedAdder = create_advanced_calculator("add", 10)
    AdvancedMultiplier = create_advanced_calculator("multiply", 5)

    # Create instances
    adder = await actor_system.spawn(AdvancedAdder, "advanced_adder", 0)
    multiplier = await actor_system.spawn(AdvancedMultiplier, "advanced_multiplier", 1)

    # Test complex operations
    print("\n=== Testing Advanced Adder ===")
    result = await adder.ask(3)  # (0 + 3) * 2 + 10 = 16
    print(f"Advanced adder result: {result}")
    assert result["value"] == 16
    assert result["operation"] == "add"
    assert result["symbol"] == "+"
    assert result["applied_factors"]["base_multiplier"] == 2
    assert result["applied_factors"]["extra_factor"] == 10

    print("\n=== Testing Advanced Multiplier ===")
    result = await multiplier.ask(4)  # (1 * 4) * 2 + 5 = 13
    print(f"Advanced multiplier result: {result}")
    assert result["value"] == 13
    assert result["operation"] == "multiply"
    assert result["symbol"] == "*"
    assert result["applied_factors"]["base_multiplier"] == 2
    assert result["applied_factors"]["extra_factor"] == 5


@pytest.mark.asyncio
async def test_module_level_class_compatibility(actor_system: ActorSystem):
    """Ensure module-level classes still work properly"""

    # This class is defined at module level
    counter_actor = await actor_system.spawn(GlobalCounter, "global_counter", 100)

    response = await counter_actor.ask("get_count")
    assert response == "Current count is 100"

    response = await counter_actor.ask("increment")
    assert response == "Current count is 101"


@pytest.mark.asyncio
async def test_distributed_scenario_simulation(actor_system: ActorSystem):
    """Simulate distributed scenario: function serialization and transmission"""

    # Simulate configurations on different nodes
    node_configs = {
        "node_a": {"multiplier": 3, "offset": 10},
        "node_b": {"multiplier": 5, "offset": 20},
    }

    def create_node_calculator(node_name: str):
        """Create calculator for specific node"""
        config = node_configs[node_name]

        class NodeCalculator:
            def __init__(self, initial_value=0):
                super().__init__()
                self.value = initial_value
                self.node_name = node_name
                self.multiplier = config["multiplier"]  # Closure variable
                self.offset = config["offset"]  # Closure variable
                self.processed_count = 0

                print(f"Creating calculator for {self.node_name}:")
                print(f"  Multiplier: {self.multiplier}")
                print(f"  Offset: {self.offset}")

            async def handle_message(self, message: Any, ctx: ActorContext) -> Any:
                self.processed_count += 1

                if isinstance(message, (int, float)):
                    # Use node-specific calculation logic
                    result = (self.value + message) * self.multiplier + self.offset
                    self.value = result

                    return {
                        "node": self.node_name,
                        "result": result,
                        "multiplier": self.multiplier,
                        "offset": self.offset,
                        "processed_count": self.processed_count,
                    }
                elif message == "get_status":
                    return {
                        "node": self.node_name,
                        "value": self.value,
                        "multiplier": self.multiplier,
                        "offset": self.offset,
                        "processed_count": self.processed_count,
                    }
                return "Unknown message"

        return NodeCalculator

    # Create calculators for different nodes
    NodeACalculator = create_node_calculator("node_a")
    NodeBCalculator = create_node_calculator("node_b")

    # Create on system (simulate remote deployment)
    calc_a = await actor_system.spawn(NodeACalculator, "calc_node_a", 0)
    calc_b = await actor_system.spawn(NodeBCalculator, "calc_node_b", 0)

    # Test node A calculator
    result_a = await calc_a.ask(5)  # (0 + 5) * 3 + 10 = 25
    assert result_a["node"] == "node_a"
    assert result_a["result"] == 25
    assert result_a["multiplier"] == 3
    assert result_a["offset"] == 10

    # Test node B calculator
    result_b = await calc_b.ask(5)  # (0 + 5) * 5 + 20 = 45
    assert result_b["node"] == "node_b"
    assert result_b["result"] == 45
    assert result_b["multiplier"] == 5
    assert result_b["offset"] == 20

    # Verify configuration independence
    status_a = await calc_a.ask("get_status")
    status_b = await calc_b.ask("get_status")

    assert status_a["multiplier"] != status_b["multiplier"]
    assert status_a["offset"] != status_b["offset"]
    assert status_a["processed_count"] == 2
    assert status_b["processed_count"] == 2


# Module-level test class (for compatibility testing)
class GlobalCounter:
    def __init__(self, cnt=0):
        super().__init__()
        self.count = cnt

    async def handle_message(self, message: Any, ctx: ActorContext) -> str:
        if message == "increment":
            self.count += 1
            return f"Current count is {self.count}"
        elif message == "get_count":
            return f"Current count is {self.count}"
        else:
            return "Unknown command"
