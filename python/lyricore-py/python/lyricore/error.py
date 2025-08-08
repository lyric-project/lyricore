class ActorError(Exception):
    """Base exception for all Actor-related errors."""

    pass


class ActorNotFoundError(ActorError):
    """Raised when an actor cannot be found at the specified path."""

    pass


class ActorStoppedError(ActorError):
    """Raised when trying to send a message to a stopped actor."""

    pass


class MessageTimeoutError(ActorError):
    """Raised when an ask operation times out."""

    pass


class ActorHandlerError(ActorError):
    """Raised when there is an error in the actor's message handler."""

    pass


class ActorNoRouteError(ActorError):
    """Raised when no route is found for a given message."""

    pass
