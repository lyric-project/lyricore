import inspect
from typing import (
    Any,
    Dict,
    Optional,
    Type,
    TypeVar,
)

try:
    from typing import ParamSpec  # Python 3.10+
except ImportError:
    from typing_extensions import ParamSpec  # Python < 3.10

T = TypeVar("T")
P = ParamSpec("P")
R = TypeVar("R")


class MethodProxy:
    """The proxy for Actor methods, providing .tell() and .ask() interfaces."""

    def __init__(
        self,
        ref: "EnhancedObjectStoreActorRef",
        method_name: str,
        method_signature: Optional[inspect.Signature] = None,
    ):
        self._ref = ref
        self._method_name = method_name
        self._signature = method_signature

    async def tell(self, *args, **kwargs) -> None:
        """Send message（fire-and-forget）"""
        message = {
            "_method_call": {
                "method": self._method_name,
                "args": args,
                "kwargs": kwargs,
            }
        }
        await self._ref.tell(message)

    async def ask(self, *args, **kwargs) -> Any:
        """Send message and wait for response"""
        message = {
            "_method_call": {
                "method": self._method_name,
                "args": args,
                "kwargs": kwargs,
            }
        }
        return await self._ref.ask(message)

    def __call__(self, *args, **kwargs):
        """Support calling the method directly"""
        return self.ask(*args, **kwargs)


class EnhancedObjectStoreActorRef:
    """The enhanced ObjectStoreActorRef that supports method-level calls."""

    def __init__(self, original_ref: "ObjectStoreActorRef", actor_class: Type = None):
        self._ref = original_ref
        self._actor_class = actor_class
        self._method_cache: Dict[str, MethodProxy] = {}

        # If an actor_class is provided, create method proxies in advance
        if actor_class:
            self._create_method_proxies()

    def __getstate__(self):
        state = self.__dict__.copy()
        state["_method_cache"] = {}
        return state

    def __setstate__(self, state):
        self.__dict__.update(state)
        if self._actor_class:
            self._create_method_proxies()

    def _create_method_proxies(self):
        """For the Actor class, create proxies for public methods."""
        if not self._actor_class:
            return

        for name, method in inspect.getmembers(
            self._actor_class, predicate=inspect.isfunction
        ):
            # Skip private methods and special methods
            if name.startswith("_"):
                continue

            # Skip framework built-in methods
            if name in ["on_start", "on_stop", "on_message", "handle_message"]:
                continue

            # Get method signature
            try:
                signature = inspect.signature(method)
                # just keep business parameters(remove self and ctx)
                parameters = list(signature.parameters.values())[1:]  # remove 'self'

                # If the last parameter is ctx, remove it
                # TODO: maybe we can find a better way to handle ctx
                if parameters and parameters[-1].name in ["ctx", "context"]:
                    parameters = parameters[:-1]

                # Create a new signature with only business parameters
                new_signature = signature.replace(parameters=parameters)

                self._method_cache[name] = MethodProxy(self, name, new_signature)
            except Exception:
                # If not a valid method or cannot get signature, skip
                continue

    def __getattr__(self, name: str) -> MethodProxy:
        """The method to dynamically get method proxies or original ref attributes."""
        # Check if the method is already cached
        if name in self._method_cache:
            return self._method_cache[name]

        # Check if the name is an attribute of the original ref
        if hasattr(self._ref, name):
            return getattr(self._ref, name)

        # If no actor_class is provided, create a generic method proxy
        if not self._actor_class:
            proxy = MethodProxy(self, name, None)
            self._method_cache[name] = proxy
            return proxy

        # If the actor_class is provided but the method does not exist, raise an error
        raise AttributeError(
            f"'{self._actor_class.__name__}' object has no attribute '{name}'"
        )

    # The raw ref's basic methods are proxied here
    async def tell(self, message: Any) -> None:
        return await self._ref.tell(message)

    async def ask(self, message: Any, timeout: Optional[float] = None) -> Any:
        return await self._ref.ask(message, timeout)

    def stop(self):
        return self._ref.stop()

    @property
    def path(self):
        return self._ref.path
