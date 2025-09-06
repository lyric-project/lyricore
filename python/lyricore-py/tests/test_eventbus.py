from dataclasses import dataclass
from typing import Any

import pytest

from lyricore import ActorContext, ActorSystem, on
from lyricore.eventbus import EventBus
from lyricore.tests.conftest import actor_system


@pytest.mark.asyncio
async def test_event_bus(actor_system):
    event_bus = EventBus()

    @dataclass
    class Event:
        data: Any

    class EventListener:
        def __init__(self):
            self.event_data = None

        @on
        async def handle_event(self, event_data: Any, ctx: ActorContext):
            self.event_data = event_data

        async def curr_state(self):
            return self.event_data

    listener_ref = await actor_system.spawn(EventListener, "event_listener")
    await event_bus.subscribe(listener_ref)

    test_data = Event(data="test_event")
    await event_bus.publish(test_data)

    state = await listener_ref.curr_state.ask()
    assert state == test_data, "Event data should be received by the listener"

    stats = await event_bus.get_stats()
    assert stats.total_events_published == 1
    assert stats.total_subscriptions == 1
    assert stats.failed_deliveries == 0


@pytest.mark.asyncio
async def test_event_bus_with_topic(actor_system):
    event_bus = EventBus()

    @dataclass
    class Event:
        value: int

    class TopicListener:
        def __init__(self):
            self.value = 0

        @on
        async def handle_event(self, event_data: Event, ctx: ActorContext):
            self.value += event_data.value

        async def curr_state(self) -> int:
            return self.value

    listener_ref = await actor_system.spawn(TopicListener, "topic_listener")
    await event_bus.subscribe(listener_ref, topic="my_topic")

    for i in range(5):
        await event_bus.publish(Event(value=1), topic="my_topic")
        await event_bus.publish(Event(value=3), topic="unsubscribed_topic")

    state = await listener_ref.curr_state.ask()
    assert state == 5, (
        "Listener should have received events from subscribed topics only"
    )
