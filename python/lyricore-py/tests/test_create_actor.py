from typing import Any

import pytest

from lyricore import ActorContext, ActorSystem
from lyricore.tests.conftest import actor_system


@pytest.mark.asyncio
async def test_create_actor_simple(actor_system: ActorSystem):
    class Counter:
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

    print("module: ", Counter.__module__)
    print("class: ", Counter.__qualname__)
    type_name = f"{Counter.__module__}.{Counter.__qualname__}"
    print("Full type name:", type_name)
    print(Counter)
    # Create an actor instance
    counter_actor = await actor_system.spawn(Counter, "counter_actor", 0)
    for _ in range(1000):
        res = await counter_actor.ask("increment")
    num = await counter_actor.ask("get_count")
    assert num == "Current count is 1000"


@pytest.mark.asyncio
async def test_create_many_actors(actor_system: ActorSystem):
    class Counter:
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

    refs = []
    for _ in range(200):
        counter_actor = await actor_system.spawn(Counter, f"counter_actor_{_}", 0)
        refs.append(counter_actor)
    for ref in refs:
        for _ in range(100):
            _ = await ref.tell("increment")
        num = await ref.ask("get_count")
        assert num == f"Current count is 100", (
            f"Expected count to be 10, got {num} for actor {ref.path}"
        )
