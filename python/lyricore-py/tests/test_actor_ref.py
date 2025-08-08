from typing import Any

import pytest

from lyricore import ActorContext, ActorSystem, on
from lyricore.tests.conftest import actor_system


@pytest.mark.asyncio
async def test_create_actor_with_ref(actor_system):
    class Adder:
        @on
        async def add(self, x, y, ctx: ActorContext):
            return x + y

    class Calculator:
        def __init__(self, adder_ref):
            self.adder_ref = adder_ref

        @on
        async def add(self, x, y, ctx: ActorContext):
            result = await self.adder_ref.add(x, y)
            return result
            # return x + y

    adder_ref = await actor_system.spawn(Adder, "adder_actor")
    assert 2 == await adder_ref.add(1, 1), "1 + 1 should equal 2"

    calc_ref = await actor_system.spawn(Calculator, "calc_actor", adder_ref)

    result = await calc_ref.add(3, 5)
    assert result == 8, "3 + 5 should equal 8"
