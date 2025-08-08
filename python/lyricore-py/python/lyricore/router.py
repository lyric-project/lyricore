import inspect
from typing import Any, Callable, Dict, Optional, Type, Union

from .error import ActorNoRouteError


class MessageRouter:
    """The message router for handling different message types and method calls."""

    def __init__(self, owner: Any = None):
        self.owner = owner
        self.handlers: Dict[Type, Callable] = {}
        self.pattern_handlers: Dict[str, Callable] = {}
        self.default_handler: Optional[Callable] = None

    def register(self, message_type: Union[Type, str], handler: Callable):
        """Register a message handler for a specific type or pattern."""
        if isinstance(message_type, type):
            self.handlers[message_type] = handler
        else:
            self.pattern_handlers[message_type] = handler

    def set_default(self, handler: Callable):
        """Set a default handler for messages that do not match any registered type."""
        self.default_handler = handler

    async def route(self, message: Any, context: Any = None) -> Any:
        """Route the message to the appropriate handler based on its type or pattern."""
        if isinstance(message, dict) and "_method_call" in message:
            return await self._route_method_call(message, context)
        # Try to match the message type directly
        handler = self.handlers.get(type(message))

        if handler:
            if inspect.iscoroutinefunction(handler):
                return await handler(message, context)
            else:
                return handler(message, context)

        # Try to match the message with registered patterns
        if isinstance(message, str):
            for pattern, handler in self.pattern_handlers.items():
                if message.startswith(pattern):
                    if inspect.iscoroutinefunction(handler):
                        return await handler(message, context)
                    else:
                        return handler(message, context)

        # If the message is a dict, try to route based on 'type' field
        if isinstance(message, dict) and "type" in message:
            msg_type = message["type"]
            if msg_type in self.pattern_handlers:
                handler = self.pattern_handlers[msg_type]
                if inspect.iscoroutinefunction(handler):
                    return await handler(message, context)
                else:
                    return handler(message, context)

        # Use the default handler if no specific handler is found
        if self.default_handler:
            if inspect.iscoroutinefunction(self.default_handler):
                return await self.default_handler(message, context)
            else:
                return self.default_handler(message, context)

        raise ActorNoRouteError(
            f"No handler found for message: {message}. "
            "Ensure the message type is registered or a default handler is set."
        )

    async def _route_method_call(self, message: Dict, context: Any) -> Any:
        """Invoke a method call message with the given context."""
        call_info = message["_method_call"]
        method_name = call_info["method"]
        args = call_info.get("args", ())
        kwargs = call_info.get("kwargs", {})

        # Object must have an owner with the method
        if not hasattr(self.owner, method_name):
            raise AttributeError(f"Actor has no method '{method_name}'")

        method = getattr(self.owner, method_name)
        if not callable(method):
            raise TypeError(f"'{method_name}' is not callable")

        # Check the method signature to determine if ctx/context should be passed
        signature = inspect.signature(method)
        params = list(signature.parameters.keys())

        # If the last parameter is ctx/context, pass context
        if params and params[-1] in ["ctx", "context"]:
            if inspect.iscoroutinefunction(method):
                return await method(*args, ctx=context, **kwargs)
            else:
                return method(*args, ctx=context, **kwargs)
        else:
            # Without ctx/context, just call the method directly
            if inspect.iscoroutinefunction(method):
                return await method(*args, **kwargs)
            else:
                return method(*args, **kwargs)


def on(message_type: Union[Type, str, None] = None):
    """Wrapper for message handlers.

    @on(StartTask)
    async def handle_start(self, msg: StartTask, ctx):
        ...

    @on("ping")
    async def handle_ping(self, msg: str, ctx):
        return "pong"

    @on
    async def handle_default(self, msg, ctx):
        ...
    """

    def decorator(func):
        if message_type is None:
            func._is_default_handler = True
        else:
            func._message_type = message_type
            func._is_message_handler = True
        return func

    # wrapper for @on() without parameters
    if callable(message_type) and not isinstance(message_type, type):
        func = message_type
        func._is_default_handler = True
        return func

    # if message_type is None:
    return decorator


def _has_message_handlers(cls: Type) -> bool:
    """Chenck if the class has message handlers or public methods."""
    has_decorators = False
    has_public_methods = False

    for name in dir(cls):
        if name.startswith("_"):
            continue
        attr = getattr(cls, name)
        if callable(attr):
            # Check if the method has message handler decorators
            if hasattr(attr, "_is_message_handler") or hasattr(
                attr, "_is_default_handler"
            ):
                has_decorators = True
            # Check if the method is a public method (not a framework built-in method)
            elif name not in ["on_start", "on_stop", "on_message", "handle_message"]:
                has_public_methods = True

    return has_decorators or has_public_methods


def _setup_message_routing(instance):
    """Setup message routing for the given instance."""
    router = MessageRouter(instance)

    # Scan the instance for methods with message handler decorators
    for name in dir(instance):
        if name.startswith("_"):
            continue

        # Get class-level attribute descriptor
        class_attr = getattr(type(instance), name, None)

        # Skip property attributes
        if isinstance(class_attr, property):
            continue

        attr = getattr(instance, name)
        if not callable(attr):
            continue

        # Check if the method has message handler decorators
        if hasattr(attr, "_is_message_handler"):
            message_type = getattr(attr, "_message_type")
            router.register(message_type, attr)

        # Check if the method is a default handler
        elif hasattr(attr, "_is_default_handler"):
            router.set_default(attr)

    return router
