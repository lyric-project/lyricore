"""
Comprehensive unit tests for the message routing system
Test file: test_message_routing.py
"""

from dataclasses import dataclass
from typing import Any, Dict, List, Optional

import pytest

from lyricore import ActorContext, ActorSystem, actor
from lyricore.py_actor import ActorError
from lyricore.router import on
from lyricore.tests.conftest import actor_system


@dataclass
class StartTask:
    """Start task message"""

    task_id: str
    params: Dict[str, Any]


@dataclass
class StopTask:
    """Stop task message"""

    task_id: str
    reason: Optional[str] = None


@dataclass
class GetStatus:
    """Get status message"""

    task_id: str


@dataclass
class ProcessData:
    """Process data message"""

    data: bytes
    metadata: Dict[str, Any]


class CustomMessage:
    """Custom message class"""

    def __init__(self, content: str):
        self.content = content


# ============================================================================
# Basic routing functionality tests
# ============================================================================


class TestBasicRouting:
    """Test basic message routing functionality"""

    @pytest.mark.asyncio
    async def test_type_based_routing(self, actor_system: ActorSystem):
        """Test type-based message routing"""

        @actor
        class TaskManager:
            def __init__(self):
                self.tasks = {}
                self.received_messages = []

            @on(StartTask)
            async def handle_start(self, msg: StartTask, ctx: ActorContext):
                self.tasks[msg.task_id] = {"status": "running", "params": msg.params}
                self.received_messages.append(("start", msg.task_id))
                return f"Task {msg.task_id} started"

            @on(StopTask)
            async def handle_stop(self, msg: StopTask, ctx: ActorContext):
                if msg.task_id in self.tasks:
                    self.tasks[msg.task_id]["status"] = "stopped"
                    self.received_messages.append(("stop", msg.task_id))
                    return f"Task {msg.task_id} stopped"
                return f"Task {msg.task_id} not found"

            @on(GetStatus)
            async def handle_status(self, msg: GetStatus, ctx: ActorContext):
                if msg.task_id in self.tasks:
                    return self.tasks[msg.task_id]
                return {"status": "not_found"}

        # Create Actor
        manager = await actor_system.spawn(TaskManager, "task_manager")

        # Test StartTask
        start_msg = StartTask("task1", {"priority": "high", "cpu": 2})
        result = await manager.ask(start_msg)
        assert result == "Task task1 started"

        # Test GetStatus
        status_msg = GetStatus("task1")
        status = await manager.ask(status_msg)
        assert status == {"status": "running", "params": {"priority": "high", "cpu": 2}}

        # Test StopTask
        stop_msg = StopTask("task1", "completed")
        result = await manager.ask(stop_msg)
        assert result == "Task task1 stopped"

        # Verify status has been updated
        status = await manager.ask(GetStatus("task1"))
        assert status["status"] == "stopped"

    @pytest.mark.asyncio
    async def test_string_pattern_routing(self, actor_system: ActorSystem):
        """Test string pattern-based routing"""

        @actor
        class CommandProcessor:
            def __init__(self):
                self.commands_received = []

            @on("ping")
            async def handle_ping(self, msg: str, ctx: ActorContext):
                return "pong"

            @on("echo:")
            async def handle_echo(self, msg: str, ctx: ActorContext):
                # Return content after echo:
                return msg.replace("echo:", "")

            @on("cmd:")
            async def handle_command(self, msg: str, ctx: ActorContext):
                self.commands_received.append(msg)
                command = msg.replace("cmd:", "")
                return f"Executed: {command}"

        processor = await actor_system.spawn(CommandProcessor, "command_processor")

        # Test exact match
        assert await processor.ask("ping") == "pong"

        # Test prefix match
        assert await processor.ask("echo:hello world") == "hello world"
        assert await processor.ask("cmd:start service") == "Executed: start service"

    @pytest.mark.asyncio
    async def test_dict_message_routing(self, actor_system: ActorSystem):
        """Test dictionary message routing"""

        @actor
        class DictHandler:
            def __init__(self):
                self.processed = []

            @on("process_data")
            async def handle_process(self, msg: dict, ctx: ActorContext):
                self.processed.append(msg)
                return {"processed": True, "id": msg.get("id")}

            @on("get_stats")
            async def handle_stats(self, msg: dict, ctx: ActorContext):
                return {"total_processed": len(self.processed)}

        _handler = await actor_system.spawn(DictHandler, "dict_handler")

        # Send dictionary message
        result = await _handler.ask(
            {"type": "process_data", "id": "123", "data": "test"}
        )
        assert result == {"processed": True, "id": "123"}

        stats = await _handler.ask({"type": "get_stats"})
        assert stats == {"total_processed": 1}


# ============================================================================
# Default handler and fallback mechanism tests
# ============================================================================


class TestDefaultHandling:
    """Test default handlers and fallback mechanisms"""

    @pytest.mark.asyncio
    async def test_default_handler(self, actor_system: ActorSystem):
        """Test default handler"""

        @actor
        class SmartActor:
            def __init__(self):
                self.unhandled = []

            @on(StartTask)
            async def handle_start(self, msg: StartTask, ctx: ActorContext):
                return f"Started: {msg.task_id}"

            @on
            async def handle_unknown(self, msg: Any, ctx: ActorContext):
                self.unhandled.append(msg)
                return f"Unknown message type: {type(msg).__name__}"

        smart_actor = await actor_system.spawn(SmartActor, "smart_actor")

        # Test registered message
        result = await smart_actor.ask(StartTask("task1", {}))
        assert result == "Started: task1"

        # Test unregistered message
        result = await smart_actor.ask(StopTask("task1"))
        assert result == "Unknown message type: StopTask"

        result = await smart_actor.ask("random string")
        assert result == "Unknown message type: str"

        result = await smart_actor.ask(CustomMessage("test"))
        assert result == "Unknown message type: CustomMessage"

    @pytest.mark.asyncio
    async def test_mixed_traditional_and_routed(self, actor_system: ActorSystem):
        """Test mixed traditional on_message/handle_message and routing handlers"""

        @actor
        class HybridActor:
            def __init__(self):
                self.log = []

            # Use routing to handle specific messages
            @on(StartTask)
            async def handle_start(self, msg: StartTask, ctx: ActorContext):
                self.log.append(f"routed: {msg.task_id}")
                return "handled by router"

            # Traditional handle_message as fallback
            async def handle_message(self, msg: Any, ctx: ActorContext):
                self.log.append(f"traditional: {msg}")
                if msg == "ping":
                    return "pong"
                return f"handled traditionally: {msg}"

        hybrid = await actor_system.spawn(HybridActor, "hybrid_actor")

        # Routed message
        result = await hybrid.ask(StartTask("task1", {}))
        assert result == "handled by router"

        # Traditional message handling
        result = await hybrid.ask("ping")
        assert result == "pong"

        result = await hybrid.ask("test")
        assert result == "handled traditionally: test"


# ============================================================================
# Complex scenario tests
# ============================================================================


class TestComplexScenarios:
    """Test complex usage scenarios"""

    @pytest.mark.asyncio
    async def test_actor_with_child_actors(self, actor_system: ActorSystem):
        """Test scenario with child actors
        TODO: This may be blocking if the child actors are not properly managed.
        """

        @actor
        class Worker:
            def __init__(self, worker_id: str):
                self.worker_id = worker_id
                self.tasks_completed = 0

            @on("work")
            async def do_work(self, msg: str, ctx: ActorContext):
                self.tasks_completed += 1
                return f"Worker {self.worker_id} completed task #{self.tasks_completed}"

            @on("stats")
            async def get_stats(self, msg: str, ctx: ActorContext):
                return {"worker_id": self.worker_id, "completed": self.tasks_completed}

        @actor
        class Supervisor:
            def __init__(self):
                self.workers = []

            @on("init")
            async def initialize(self, msg: str, ctx: ActorContext):
                # Create child actors
                for i in range(3):
                    worker = await ctx.spawn(Worker, f"worker_{i}", f"w{i}")
                    self.workers.append(worker)
                return f"Initialized {len(self.workers)} workers"

            @on("distribute_work")
            async def distribute(self, msg: str, ctx: ActorContext):
                results = []
                for i, worker in enumerate(self.workers):
                    result = await worker.ask("work")
                    results.append(result)
                return results

            @on("get_all_stats")
            async def all_stats(self, msg: str, ctx: ActorContext):
                stats = []
                for worker in self.workers:
                    stat = await worker.ask("stats")
                    stats.append(stat)
                return stats

        supervisor = await actor_system.spawn(Supervisor, "supervisor")

        # Initialize
        result = await supervisor.ask("init")
        assert result == "Initialized 3 workers"

        # Distribute work
        results = await supervisor.ask("distribute_work")
        assert len(results) == 3
        assert "completed task #1" in results[0]

        # Distribute work again
        results = await supervisor.ask("distribute_work")
        assert "completed task #2" in results[0]

        # Get statistics
        stats = await supervisor.ask("get_all_stats")
        assert len(stats) == 3
        for i, stat in enumerate(stats):
            assert stat["worker_id"] == f"w{i}"
            assert stat["completed"] == 2

    @pytest.mark.asyncio
    async def test_state_machine_actor(self, actor_system: ActorSystem):
        """Test state machine actor"""

        @actor
        class StateMachine:
            def __init__(self):
                self.state = "idle"
                self.data = None

            @on("start")
            async def handle_start(self, msg: str, ctx: ActorContext):
                if self.state == "idle":
                    self.state = "running"
                    return "Started"
                return f"Cannot start from state: {self.state}"

            @on("process")
            async def handle_process(self, msg: str, ctx: ActorContext):
                if self.state == "running":
                    self.state = "processing"
                    self.data = "processed"
                    return "Processing"
                return f"Cannot process from state: {self.state}"

            @on("stop")
            async def handle_stop(self, msg: str, ctx: ActorContext):
                if self.state in ["running", "processing"]:
                    self.state = "stopped"
                    return "Stopped"
                return f"Cannot stop from state: {self.state}"

            @on("reset")
            async def handle_reset(self, msg: str, ctx: ActorContext):
                self.state = "idle"
                self.data = None
                return "Reset"

            @on("get_state")
            async def get_state(self, msg: str, ctx: ActorContext):
                return {"state": self.state, "data": self.data}

        machine = await actor_system.spawn(StateMachine, "state_machine")

        # Test state transitions
        assert await machine.ask("start") == "Started"
        state = await machine.ask("get_state")
        assert state["state"] == "running"

        assert await machine.ask("process") == "Processing"
        state = await machine.ask("get_state")
        assert state["state"] == "processing"
        assert state["data"] == "processed"

        assert await machine.ask("stop") == "Stopped"
        state = await machine.ask("get_state")
        assert state["state"] == "stopped"

        # Test invalid transitions
        result = await machine.ask("start")
        assert "Cannot start" in result

        # Reset
        assert await machine.ask("reset") == "Reset"
        state = await machine.ask("get_state")
        assert state["state"] == "idle"
        assert state["data"] is None


# ============================================================================
# Performance and concurrency tests
# ============================================================================


class TestPerformanceAndConcurrency:
    """Test performance and concurrency scenarios"""

    @pytest.mark.asyncio
    async def test_concurrent_message_handling(self, actor_system: ActorSystem):
        """Test concurrent message handling"""

        @actor
        class ConcurrentActor:
            def __init__(self):
                self.counter = 0
                self.messages = []

            @on("increment")
            async def increment(self, msg: str, ctx: ActorContext):
                self.counter += 1
                return self.counter

            @on("add")
            async def add_value(self, msg: dict, ctx: ActorContext):
                value = msg.get("value", 0)
                self.counter += value
                return self.counter

            @on("get")
            async def get_value(self, msg: str, ctx: ActorContext):
                return self.counter

        concurrent_actor = await actor_system.spawn(ConcurrentActor, "concurrent")

        # Send messages concurrently
        tasks = []

        # 100 increments
        for _ in range(100):
            await concurrent_actor.tell("increment")

        # 50 add operations
        for i in range(50):
            await concurrent_actor.tell({"type": "add", "value": i})

        # Verify final result
        final_value = await concurrent_actor.ask("get")
        # 100 times +1 and sum(0..49) = 1225
        expected = 100 + sum(range(50))
        assert final_value == expected

    @pytest.mark.asyncio
    async def test_high_throughput(self, actor_system: ActorSystem):
        """Test high throughput scenario"""

        @actor
        class HighThroughputActor:
            def __init__(self):
                self.processed = 0

            @on(ProcessData)
            async def process(self, msg: ProcessData, ctx: ActorContext):
                self.processed += 1
                # Simulate some processing
                return {"processed": self.processed, "size": len(msg.data)}

            @on("stats")
            async def get_stats(self, msg: str, ctx: ActorContext):
                return {"total_processed": self.processed}

        actor_ref = await actor_system.spawn(HighThroughputActor, "high_throughput")

        # Send large number of messages
        num_messages = 1000
        data_size = 1024  # 1KB per message

        tasks = []
        for i in range(num_messages):
            msg = ProcessData(
                data=b"x" * data_size, metadata={"id": i, "timestamp": i * 1000}
            )
            await actor_ref.tell(msg)
        stats = await actor_ref.ask("stats")
        assert stats["total_processed"] == num_messages


# ============================================================================
# Error handling tests
# ============================================================================


class TestErrorHandling:
    """Test error handling scenarios"""

    @pytest.mark.asyncio
    async def test_handler_exception(self, actor_system: ActorSystem):
        """Test handler throwing exceptions"""

        @actor
        class FaultyActor:
            @on("safe")
            async def safe_handler(self, msg: str, ctx: ActorContext):
                return "safe response"

            @on("error")
            async def error_handler(self, msg: str, ctx: ActorContext):
                raise ValueError("Intentional error")

            @on("divide")
            async def divide_handler(self, msg: dict, ctx: ActorContext):
                a = msg.get("a", 0)
                b = msg.get("b", 1)
                return a / b  # Possible division by zero error

        faulty = await actor_system.spawn(FaultyActor, "faulty")

        # Normal handling
        assert await faulty.ask("safe") == "safe response"

        # Handler exception
        with pytest.raises(Exception):
            await faulty.ask("error")

        # Division by zero error
        with pytest.raises(Exception):
            await faulty.ask({"type": "divide", "a": 10, "b": 0})

        # Actor should still be alive
        assert await faulty.ask("safe") == "safe response"

    @pytest.mark.asyncio
    async def test_no_matching_handler(self, actor_system: ActorSystem):
        """Test case with no matching handler"""

        @actor
        class PartialActor:
            @on(StartTask)
            async def handle_start(self, msg: StartTask, ctx: ActorContext):
                return "started"

            # No default handler

        partial = await actor_system.spawn(PartialActor, "partial")

        # Matching message
        result = await partial.ask(StartTask("task1", {}))
        assert result == "started"

        # Non-matching message - should return None or be handled by traditional handle_message
        with pytest.raises(ActorError):
            await partial.ask(StopTask("task1"))


# ============================================================================
# Integration tests
# ============================================================================


# @pytest.mark.integration
class TestIntegration:
    """Complete integration tests"""

    @pytest.mark.asyncio
    async def test_complete_workflow(self, actor_system: ActorSystem):
        """Test complete workflow"""

        @dataclass
        class Job:
            job_id: str
            tasks: List[str]

        @dataclass
        class JobResult:
            job_id: str
            results: List[str]
            status: str

        @actor
        class JobProcessor:
            def __init__(self):
                self.jobs = {}
                self.results = {}

            @on(Job)
            async def process_job(self, job: Job, ctx: ActorContext):
                self.jobs[job.job_id] = job

                # Process each task
                results = []
                for task in job.tasks:
                    # Simulate processing
                    result = f"Completed: {task}"
                    results.append(result)

                job_result = JobResult(
                    job_id=job.job_id, results=results, status="completed"
                )
                self.results[job.job_id] = job_result
                return job_result

            @on(dict)
            async def get_status(self, msg: dict, _ctx: ActorContext):
                job_id = msg.get("job_id")
                if job_id in self.results:
                    return self.results[job_id]
                elif job_id in self.jobs:
                    return JobResult(job_id=job_id, results=[], status="processing")
                else:
                    return JobResult(job_id=job_id, results=[], status="not_found")

            @on
            async def handle_other(self, msg: Any, ctx: ActorContext):
                return f"Unhandled message: {msg}"

        processor = await actor_system.spawn(JobProcessor, "job_processor")

        # Submit job
        job = Job(job_id="job001", tasks=["task1", "task2", "task3"])
        result = await processor.ask(job)

        assert isinstance(result, JobResult)
        assert result.job_id == "job001"
        assert result.status == "completed"
        assert len(result.results) == 3
        assert all("Completed:" in r for r in result.results)

        # Query status
        status = await processor.ask({"type": "get_job_status", "job_id": "job001"})
        assert status.status == "completed"

        # Query non-existent job
        status = await processor.ask({"type": "get_job_status", "job_id": "job999"})
        assert status.status == "not_found"

        # Unhandled message
        response = await processor.ask("random message")
        assert response == "Unhandled message: random message"
