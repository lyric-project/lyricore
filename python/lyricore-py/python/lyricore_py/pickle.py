try:
    import cloudpickle as pickle
except ImportError:
    import pickle


def dumps(obj, protocol=None):
    """
    Deserializes an object to a byte string using cloudpickle or pickle.

    :param obj: The object to serialize
    :param protocol: Deserialization protocol, if None uses default protocol
    :return: The serialized byte string of the object
    """
    if protocol is None:
        return pickle.dumps(obj)
    else:
        return pickle.dumps(obj, protocol=protocol)


def loads(data):
    """Deserialize a byte string to an object using cloudpickle or pickle.

    :param data: The byte string to deserialize
    :return: The deserialized object
    """
    return pickle.loads(data)
