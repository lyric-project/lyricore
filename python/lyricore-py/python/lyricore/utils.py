import sys
from typing import Any

try:
    import numpy as np

    HAS_NUMPY = True
except ImportError:
    HAS_NUMPY = False


def get_sizeof(obj: Any) -> int:
    """Calculate the size of an object in bytes."""
    # Base types
    if isinstance(obj, (int, float, str, bool)):
        return sys.getsizeof(obj)
    # Dictionaries
    elif isinstance(obj, dict):
        return sum(get_sizeof(key) + get_sizeof(value) for key, value in obj.items())
    # This handles lists, tuples, and sets
    elif isinstance(obj, (list, tuple, set)):
        return sum(get_sizeof(item) for item in obj)
    # If numpy is available, we can use its nbytes attribute for arrays
    elif HAS_NUMPY and isinstance(obj, np.ndarray):
        return obj.nbytes
    # Bytes and bytearray are handled separately
    elif isinstance(obj, (bytes, bytearray)):
        return len(obj)
    else:
        from pympler import asizeof

        return (
            asizeof.asizeof(obj) if hasattr(asizeof, "asizeof") else sys.getsizeof(obj)
        )
