import base64
from dataclasses import dataclass
from typing import Optional

from . import pickle


@dataclass
class ActorConstructionTask:
    """The class representing a task to construct an Actor instance."""

    constructor_func: bytes
    constructor_args: list
    constructor_kwargs: dict
    function_hash: str
    module_name: str
    class_name: str
    capture_globals: Optional[bytes] = None


class ActorFunctionDeserializer:
    """Actor deserializer for creating Actor instances from serialized tasks."""

    @staticmethod
    def execute_construction_task(task: ActorConstructionTask):
        """Run the construction task to create an Actor instance."""
        try:
            # Deserialize the constructor function
            constructor = pickle.loads(task.constructor_func)

            # Execute the constructor function with provided arguments and keyword arguments
            actor_instance = constructor()
            # TODO: Add validation to ensure the instance is of the correct type
            return actor_instance

        except Exception as e:
            raise RuntimeError(f"Failed to execute actor construction task: {e}")

    @staticmethod
    def create_construction_task_from_dict(task_dict: dict) -> ActorConstructionTask:
        """Build an ActorConstructionTask from a dictionary."""
        return ActorConstructionTask(
            constructor_func=base64.b64decode(task_dict["constructor_func"]),
            constructor_args=task_dict["constructor_args"],
            constructor_kwargs=task_dict["constructor_kwargs"],
            function_hash=task_dict["function_hash"],
            module_name=task_dict["module_name"],
            class_name=task_dict["class_name"],
            capture_globals=base64.b64decode(task_dict["capture_globals"])
            if task_dict.get("capture_globals")
            else None,
        )


def deserialize_and_create_actor(task_dict: dict):
    """Create an Actor instance from a serialized task dictionary."""
    task = ActorFunctionDeserializer.create_construction_task_from_dict(task_dict)
    return ActorFunctionDeserializer.execute_construction_task(task)
