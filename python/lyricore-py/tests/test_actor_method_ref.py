from dataclasses import dataclass
from typing import Any, List

import pytest

from lyricore import ActorContext, ActorSystem, actor
from lyricore.router import on
from lyricore.tests.conftest import actor_multiple_systems, actor_system


# Define message types
@dataclass
class StartTask:
    task_id: str
    params: dict


@dataclass
class TaskResult:
    task_id: str
    result: any
    success: bool


# Example 1: Basic Calculator Actor
@actor
class CalculatorActor:
    def __init__(self):
        self.value = 0
        self.history: List[str] = []

    # Regular business method - supports ref.add.ask() calls
    async def add(self, x: int, y: int, ctx) -> int:
        """Addition operation - doesn't require ctx parameter"""
        result = x + y
        self.value = result
        self.history.append(f"add({x}, {y}) = {result}")
        return result

    async def multiply(self, x: int, y: int, ctx) -> int:
        """Multiplication operation - requires ctx parameter, framework passes it automatically"""
        result = x * y
        self.value = result
        self.history.append(f"multiply({x}, {y}) = {result}")
        # Can use ctx to perform operations like sending messages to other Actors
        return result

    def get_value(self) -> int:
        """Get current value - synchronous method"""
        return self.value

    def get_history(self) -> List[str]:
        """Get operation history"""
        return self.history.copy()

    def reset(self) -> None:
        """Reset calculator"""
        self.value = 0
        self.history.clear()

    # Traditional message handlers - still supported
    @on(StartTask)
    async def handle_start_task(self, msg: StartTask, ctx):
        """Handle start task message"""
        if msg.task_id == "reset":
            self.reset()
            return TaskResult(msg.task_id, "reset completed", True)
        else:
            return TaskResult(msg.task_id, "unknown task", False)

    @on("ping")
    async def handle_ping(self, msg: str, ctx):
        """Handle ping message"""
        return "pong"

    @on  # Default handler
    async def handle_default(self, msg, ctx):
        """Handle all other messages"""
        self.history.append(f"received unknown message: {msg}")
        return f"unknown message: {msg}"


# Example 2: Task Manager Actor
@actor
class TaskManagerActor:
    def __init__(self):
        self.tasks = {}
        self.completed_tasks = []

    async def create_task(self, task_id: str, description: str, ctx) -> bool:
        """Create new task"""
        if task_id in self.tasks:
            return False

        self.tasks[task_id] = {
            "id": task_id,
            "description": description,
            "status": "pending",
            "created_at": "now",  # In real applications, would use actual timestamp
        }
        return True

    async def complete_task(self, task_id: str) -> bool:
        """Complete task"""
        if task_id not in self.tasks:
            return False

        task = self.tasks.pop(task_id)
        task["status"] = "completed"
        self.completed_tasks.append(task)
        return True

    def list_tasks(self) -> dict:
        """List all tasks"""
        return {
            "pending": list(self.tasks.values()),
            "completed": self.completed_tasks.copy(),
        }

    async def get_task_count(self) -> int:
        """Get total task count"""
        return len(self.tasks) + len(self.completed_tasks)


@pytest.mark.asyncio
async def test_actor_basic_method_ref(actor_system: ActorSystem):
    """Test basic Actor method calls"""

    # Create Calculator Actor
    calc_ref = await actor_system.spawn(CalculatorActor, "/user/calculator")

    # Test addition
    result1 = await calc_ref.add.ask(10, 20)
    assert result1 == 30, "10 + 20 should equal 30"

    # Test multiplication
    result2 = await calc_ref.multiply.ask(5, 6)
    assert result2 == 30, "5 * 6 should equal 30"

    # Test getting current value
    current_value = await calc_ref.get_value.ask()
    assert current_value == 30, "Current value should be 30 after operations"
    #
    # Test reset
    await calc_ref.reset.tell()
    new_value = await calc_ref.get_value.ask()
    assert new_value == 0, "Value should be reset to 0"


@pytest.mark.asyncio
async def test_raw_actor_call(actor_system: ActorSystem):
    """Test raw Actor calls"""

    # Create Calculator Actor
    calc_ref = await actor_system.spawn(CalculatorActor, "/user/calculator")

    result1 = await calc_ref.ask(StartTask("reset", {}))
    assert result1.success, "Reset task should succeed"

    result2 = await calc_ref.ask("ping")
    assert result2 == "pong", "Ping should return 'pong'"

    result3 = await calc_ref.ask({"unknown": "message"})
    assert result3 == "unknown message: {'unknown': 'message'}", (
        "Unknown message should be handled correctly"
    )


@pytest.mark.asyncio
async def test_multiple_actor_systems_method_ref(
    actor_multiple_systems: List[ActorSystem],
):
    class MultipleCalculatorActor:
        def __init__(self):
            self.value = 0

        async def handle_message(self, opt_dict: dict, ctx: ActorContext) -> Any:
            """Handle operation with options"""
            if opt_dict.get("operation") == "add":
                x = opt_dict.get("x", 0)
                y = opt_dict.get("y", 0)
                self.value = x + y
                return {"value": self.value}
            elif opt_dict.get("operation") == "multiply":
                x = opt_dict.get("x", 1)
                y = opt_dict.get("y", 1)
                self.value = x * y
                return self.value
            elif opt_dict.get("operation") == "get_value":
                return self.value
            return None

    system1 = actor_multiple_systems[0]
    system2 = actor_multiple_systems[1]

    actor1 = await system1.spawn(MultipleCalculatorActor, "/user/calculator1")
    remote_path = (
        f"lyricore://{system1.system_name}@{system1.listen_address}/user/calculator1"
    )
    remote_calculator = await system2.actor_of(remote_path)

    # Test addition on remote actor
    result1 = await remote_calculator.ask({"operation": "add", "x": 10, "y": 20})
    assert result1 == {"value": 30}, "10 + 20 should equal 30"
    # Test multiplication on remote actor
    result2 = await remote_calculator.ask({"operation": "multiply", "x": 5, "y": 6})
    assert result2 == 30, "5 * 6 should equal 30"

    # Test getting current value from remote actor
    result3 = await remote_calculator.ask({"operation": "get_value"})
    assert result3 == 30, "Current value should be 30 after operations"
