from typing import List, Optional, Union

from . import pickle
from ._lyricore import PyEventBus, PyTopicClassifier


class EventBus:
    def __init__(self):
        self._event_bus = PyEventBus()
        self._store = None

    async def subscribe(
        self, actor_ref, topic: Optional[Union[str, List[str]]] = None
    ) -> str:
        """Subscribe an actor to the event bus with an optional topic filter.

        TODO: The performance is reduced when subscribing multiple actors
        """
        store = actor_ref.curr_store
        if not self._store and store:
            self._store = store
        classifier = None
        if topic:
            if isinstance(topic, str):
                topic = [topic]
            classifier = PyTopicClassifier(topic)
        return await self._event_bus.subscribe(actor_ref.raw_ref, classifier)

    async def unsubscribe(self, subscription_id: str) -> bool:
        return await self._event_bus.unsubscribe(subscription_id)

    async def unsubscribe_actor(self, actor_ref) -> bool:
        return await self._event_bus.unsubscribe_actor(actor_ref.raw_ref)

    async def publish(self, event_data, topic: Optional[str] = None) -> int:
        # Serialize event_data if necessary
        from .actor_wrapper import ObjectStoreActorRef
        from .object_store import get_global_object_store

        store = self._store or get_global_object_store()
        if not store:
            raise RuntimeError("Global object store is not initialized.")
        ref = ObjectStoreActorRef(None, store)
        serialized_message = await ref._serialize_message(event_data)
        # Serialize the message to bytes
        serialized_message_bytes = pickle.dumps(serialized_message)
        return await self._event_bus.publish(serialized_message_bytes, topic)

    async def get_stats(self):
        return await self._event_bus.get_stats()

    async def shutdown(self):
        return await self._event_bus.shutdown()
