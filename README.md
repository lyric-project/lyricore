# Lyricore

A foundational runtime engine designed to orchestrate and execute AI Agents with precision and harmony.

## Installation

```bash
pip install lyricore
```

## Usage

**Create an actor system:**

```python
import lyricore as lc

system = lc.ActorSystem("simple_example")
await system.start()
```

**Define an actor:**

```python
class Counter:
    def __init__(self):
        self.value = 0
    async def add(self, x):
        self.value += x
        return self.value
    async def current(self):
        return self.value
```

**Create an actor instance:**

```python
counter_ref = await system.spawn(Counter, "counter")
```

**Send a message and await the response:**

```python
result = await counter_ref.add(5)
print(result)  # Output: 5
```

**Send a message without the response:**
```python
result = await counter_ref.add.tell(5)
print(result)  # Output: None
```
