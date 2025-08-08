from typing import Any, List

import pytest

from lyricore import ActorContext, ActorSystem
from lyricore.tests.conftest import actor_multiple_systems


@pytest.mark.asyncio
async def test_multiple_actor_systems(actor_multiple_systems: List[ActorSystem]):
    """Test multiple actor systems with remote actors"""

    class CounterActor:
        def __init__(self, start=0):
            super().__init__()
            self.count = start

        async def on_start(self, ctx: ActorContext) -> None:
            print("Slow Remote Actor started with initial count:", self.count)

        async def on_stop(self, ctx: ActorContext) -> None:
            print("Slow Remote Actor stopped.")

        async def handle_message(self, message, ctx):
            actor_id = ctx.actor_id
            print(f"[{actor_id}]Context in handle_message: {ctx}, message: {message}")
            if message == "inc":
                self.count += 1
                return self.count
            elif message == "get":
                return self.count
            elif hasattr(message, "value"):
                return f"Data received: {message.value}"
            return None

    system1 = actor_multiple_systems[0]
    system2 = actor_multiple_systems[1]
    actor1 = await system1.spawn(CounterActor, "counter")
    remote_path = (
        f"lyricore://{system1.system_name}@{system1.listen_address}/user/counter"
    )
    remote_counter = await system2.actor_of(remote_path)
    result = await remote_counter.ask("get")
    assert result == 0, f"Remote counter initial value should be 0, got {result}"
    for i in range(100):
        await remote_counter.ask("inc")
    result = await remote_counter.ask("get")

    assert result == 100, f"Remote counter value should be 100, got {result}"

    class Data:
        def __init__(self, value):
            self.value = value

    data = Data("Hello, World!")
    res = await remote_counter.ask(data)
    print(f"Send to remote Actor 的result: {res}")
