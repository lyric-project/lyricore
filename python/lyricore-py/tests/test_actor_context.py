from typing import Any, Dict, List

import pytest

from lyricore import ActorContext, ActorSystem
from lyricore.tests.conftest import actor_system


@pytest.mark.asyncio
async def test_create_sub_actor(actor_system: ActorSystem):
    class ChildActor:
        def __init__(self, name: str):
            super().__init__()
            self.name = name
            self.count = 0

        async def handle_message(self, message: Any, ctx: ActorContext) -> str:
            if message == "increment":
                self.count += 1
                return f"{self.name} count incremented to {self.count}"
            elif message == "get_count":
                return f"{self.name} current count is {self.count}"
            elif message == "are_you_ok":
                return f"{self.name} is ok"
            else:
                return "Unknown command"

    class ParentActor:
        def __init__(self):
            super().__init__()
            self.child_actor = None

        async def handle_message(self, message: Any, ctx: ActorContext) -> str:
            if message == "create_child":
                self.child_actor = await ctx.spawn(
                    ChildActor, "child_actor", "child_actor"
                )
                return await self.child_actor.ask("are_you_ok")
            elif message == "get_child_count":
                if self.child_actor:
                    return await self.child_actor.ask("get_count")
                else:
                    return "No child actor exists"
            elif message == "increment_child_count":
                if self.child_actor:
                    return await self.child_actor.ask("increment")
                else:
                    return "No child actor exists"
            else:
                return "Unknown command"

    pf = await actor_system.spawn(ParentActor, "parent_actor")
    res = await pf.ask("create_child")
    assert res == "child_actor is ok"
    assert await pf.ask("get_child_count") == "child_actor current count is 0"
    for i in range(100):
        res = await pf.ask("increment_child_count")
        assert res == "child_actor count incremented to {}".format(i + 1)
    assert await pf.ask("get_child_count") == "child_actor current count is 100"


@pytest.mark.asyncio
async def test_map_reduce_word_count(actor_system: ActorSystem):
    class MapperActor:
        """Mapper Actor - responsible for processing text fragments and generating word counts"""

        def __init__(self, mapper_id: str):
            super().__init__()
            self.mapper_id = mapper_id

        async def handle_message(
            self, message: Any, ctx: ActorContext
        ) -> Dict[str, int]:
            if isinstance(message, dict) and message.get("action") == "map":
                text = message.get("text", "")
                # Simple word splitting and counting
                words = text.lower().split()
                word_count = {}
                for word in words:
                    # Remove punctuation
                    clean_word = "".join(c for c in word if c.isalnum())
                    if clean_word:
                        word_count[clean_word] = word_count.get(clean_word, 0) + 1

                return {"mapper_id": self.mapper_id, "word_count": word_count}
            else:
                return {"error": "Unknown command"}

    class ReducerActor:
        """Reducer Actor - responsible for merging results from different Mappers"""

        def __init__(self):
            super().__init__()
            self.final_count = {}

        async def handle_message(self, message: Any, ctx: ActorContext) -> Any:
            if isinstance(message, dict):
                if message.get("action") == "reduce":
                    word_count = message.get("word_count", {})
                    # Merge new word counts into final result
                    for word, count in word_count.items():
                        self.final_count[word] = self.final_count.get(word, 0) + count
                    return {"status": "reduced", "total_words": len(self.final_count)}

                elif message.get("action") == "get_result":
                    return {"final_count": self.final_count}

                elif message.get("action") == "reset":
                    self.final_count = {}
                return {"status": "reset"}

            return {"error": "Unknown command"}

    class MasterActor:
        """Master Actor - coordinates the entire MapReduce process"""

        def __init__(self):
            super().__init__()
            self.mappers = []
            self.reducer = None

        async def handle_message(self, message: Any, ctx: ActorContext) -> Any:
            if isinstance(message, dict):
                if message.get("action") == "init":
                    # Create Reducer
                    self.reducer = await ctx.spawn(ReducerActor, "reducer")

                    # Create multiple Mappers
                    num_mappers = message.get("num_mappers", 3)
                    for i in range(num_mappers):
                        mapper = await ctx.spawn(
                            MapperActor, f"mapper_{i}", f"mapper_{i}"
                        )
                        self.mappers.append(mapper)

                    return {"status": "initialized", "mappers": len(self.mappers)}

                elif message.get("action") == "process":
                    texts = message.get("texts", [])

                    # Distribute texts to different Mappers
                    for i, text in enumerate(texts):
                        mapper_idx = i % len(self.mappers)
                        mapper_result = await self.mappers[mapper_idx].ask(
                            {"action": "map", "text": text}
                        )

                        # Send Mapper results to Reducer
                        if "word_count" in mapper_result:
                            await self.reducer.ask(
                                {
                                    "action": "reduce",
                                    "word_count": mapper_result["word_count"],
                                }
                            )

                    # Get final result
                    final_result = await self.reducer.ask({"action": "get_result"})
                    return final_result

            return {"error": "Unknown command"}

    # Test MapReduce Word Count
    master = await actor_system.spawn(MasterActor, "master")

    # Initialize system
    init_result = await master.ask({"action": "init", "num_mappers": 3})
    assert init_result["status"] == "initialized"
    assert init_result["mappers"] == 3

    # Prepare test texts
    test_texts = [
        "hello world hello",
        "world python programming",
        "hello python world programming",
        "actor system mapreduce example",
        "python actor model distributed computing",
    ]

    # Process texts
    result = await master.ask({"action": "process", "texts": test_texts})

    # Verify results
    assert "final_count" in result
    final_count = result["final_count"]

    # Verify expected word counts
    assert final_count["hello"] == 3  # "hello" appears 3 times
    assert final_count["world"] == 3  # "world" appears 3 times
    assert final_count["python"] == 3  # "python" appears 3 times
    assert final_count["programming"] == 2  # "programming" appears 2 times
    assert final_count["actor"] == 2  # "actor" appears 2 times

    print("MapReduce Word Count Results:")
    for word, count in sorted(final_count.items()):
        print(f"  {word}: {count}")

    # Verify total number of unique words
    assert len(final_count) > 0
    print(f"Total unique words: {len(final_count)}")
