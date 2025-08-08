import asyncio
import time

import pytest

from lyricore import PyObjectStore, PyStoreConfig

# _COPY_MODE = "zero_copy"
_COPY_MODE = "fast_copy"  # Use fast copy mode to avoid blocking
# _COPY_MODE = "safe_copy"  # Use standard copy mode, may cause blocking


@pytest.mark.asyncio
async def test_object_store_blocking_detection():
    """More precisely detect whether put_numpy and get_numpy block the async event loop"""
    import numpy as np

    config = PyStoreConfig(max_object_size=200 * 1024 * 1024)
    store = PyObjectStore(config)
    arr = np.random.rand(1000, 20000)  # About 152MB
    print("Byte size of array:", arr.nbytes / 1024 / 1024, "MB")

    timestamps = []
    counter = 0

    async def heartbeat_task():
        """Heartbeat task: record timestamp every 10ms"""
        nonlocal counter, timestamps
        while True:
            current_time = time.time()
            timestamps.append(current_time)
            counter += 1
            print(f"Heartbeat {counter}: {time.strftime('%H:%M:%S.%f')[:-3]}")
            await asyncio.sleep(0.01)  # 10ms interval

    print("=== Starting blocking detection test ===")

    # Start heartbeat task
    heartbeat = asyncio.create_task(heartbeat_task())

    # Let heartbeat task run for a while to establish baseline
    await asyncio.sleep(0.5)
    baseline_count = counter
    print(f"Baseline heartbeat count: {baseline_count}")

    # Test if put_numpy blocks
    print("Starting put_numpy test...")
    put_start_time = time.time()
    put_start_count = counter

    # ref = await store.put_numpy(arr)
    ref = await store.put_numpy(arr, copy_mode=_COPY_MODE)

    put_end_time = time.time()
    put_end_count = counter
    put_duration = put_end_time - put_start_time
    put_heartbeats_during = put_end_count - put_start_count

    print(f"put_numpy duration: {put_duration:.3f} seconds")
    print(f"Heartbeats during put_numpy: {put_heartbeats_during}")
    print(f"Expected heartbeats: {int(put_duration / 0.01)}")

    # Let system recover
    await asyncio.sleep(0.3)

    # Test if get_numpy blocks
    print("Starting get_numpy test...")
    get_start_time = time.time()
    get_start_count = counter

    retrieved_arr = await store.get_numpy(ref)

    get_end_time = time.time()
    get_end_count = counter
    get_duration = get_end_time - get_start_time
    get_heartbeats_during = get_end_count - get_start_count

    print(f"get_numpy duration: {get_duration:.3f} seconds")
    print(f"Heartbeats during get_numpy: {get_heartbeats_during}")
    print(f"Expected heartbeats: {int(get_duration / 0.1)}")

    # Stop heartbeat task
    heartbeat.cancel()
    try:
        await heartbeat
    except asyncio.CancelledError:
        pass

    # Analyze results
    print("\n=== Blocking analysis results ===")

    # Check put_numpy blocking
    put_expected_heartbeats = max(1, int(put_duration / 0.1))
    put_blocking_ratio = (
        put_heartbeats_during / put_expected_heartbeats
        if put_expected_heartbeats > 0
        else 0
    )

    print(f"put_numpy blocking status:")
    print(
        f"  - Actual heartbeats: {put_heartbeats_during}, Expected heartbeats: {put_expected_heartbeats}"
    )
    print(
        f"  - Heartbeat ratio: {put_blocking_ratio:.2f} (1.0 means non-blocking, 0.0 means completely blocked)"
    )
    print(
        f"  - Assessment: {'Blocking' if put_blocking_ratio < 0.5 else 'Non-blocking'}"
    )

    # Check get_numpy blocking
    get_expected_heartbeats = max(1, int(get_duration / 0.1))
    get_blocking_ratio = (
        get_heartbeats_during / get_expected_heartbeats
        if get_expected_heartbeats > 0
        else 0
    )

    print(f"get_numpy blocking status:")
    print(
        f"  - Actual heartbeats: {get_heartbeats_during}, Expected heartbeats: {get_expected_heartbeats}"
    )
    print(
        f"  - Heartbeat ratio: {get_blocking_ratio:.2f} (1.0 means non-blocking, 0.0 means completely blocked)"
    )
    print(
        f"  - Assessment: {'Blocking' if get_blocking_ratio < 0.5 else 'Non-blocking'}"
    )

    # Verify data correctness (optional)
    assert retrieved_arr.shape == arr.shape
    assert np.array_equal(
        retrieved_arr, arr
    )  # Uncomment if full verification is needed
    np.testing.assert_array_equal(
        retrieved_arr, arr, err_msg="Retrieved array does not match original array"
    )
    store.print_stats()


@pytest.mark.asyncio
async def test_concurrent_operations():
    """Test concurrent operation capability"""
    import numpy as np

    store = PyObjectStore()

    async def store_and_retrieve(size_mb: int, name: str):
        """Store and retrieve operation"""
        arr = np.random.rand(int(size_mb * 1024 * 1024 / 8))  # 8 bytes per float64

        start_time = time.time()
        print(f"{name}: Starting storage ({size_mb}MB)")

        ref = await store.put_numpy(arr, copy_mode=_COPY_MODE)
        mid_time = time.time()
        print(f"{name}: Storage completed, took {mid_time - start_time:.3f}s")

        retrieved = await store.get_numpy(ref)
        end_time = time.time()
        print(f"{name}: Retrieval completed, total time {end_time - start_time:.3f}s")

        return end_time - start_time

    print("=== Testing concurrent operations ===")

    # Execute multiple storage operations concurrently
    tasks = [
        store_and_retrieve(10, "Task-A"),
        store_and_retrieve(15, "Task-B"),
        store_and_retrieve(5, "Task-C"),
    ]

    start_time = time.time()
    results = await asyncio.gather(*tasks)
    total_time = time.time() - start_time

    print(f"\nConcurrent execution results:")
    print(f"  - Total time: {total_time:.3f}s")
    print(f"  - Individual task times: {[f'{r:.3f}s' for r in results]}")
    print(f"  - Average task time: {sum(results) / len(results):.3f}s")
    store.print_stats()


@pytest.mark.asyncio
async def test_gap_analysis():
    """Detect blocking through time gap analysis"""
    import numpy as np

    store_config = PyStoreConfig(max_object_size=1000 * 1024 * 1024)  # 100MB
    store = PyObjectStore(store_config)
    arr = np.random.rand(10000, 10000)  # Medium-sized array
    print("Byte size of array:", arr.nbytes / 1024 / 1024, "MB")

    timestamps = []

    async def timestamp_collector():
        """Collect timestamps"""
        while True:
            timestamps.append(time.time())
            await asyncio.sleep(0.05)  # 50ms interval

    print("=== Time gap analysis ===")

    collector = asyncio.create_task(timestamp_collector())

    # Collect baseline data
    await asyncio.sleep(0.5)
    baseline_count = len(timestamps)

    # Execute storage operation
    operation_start = len(timestamps)
    ref = await store.put_numpy(arr, copy_mode=_COPY_MODE)
    operation_end = len(timestamps)

    # Collect more data
    await asyncio.sleep(0.3)

    collector.cancel()
    try:
        await collector
    except asyncio.CancelledError:
        pass

    # Analyze time gaps
    if len(timestamps) > 1:
        gaps = [timestamps[i + 1] - timestamps[i] for i in range(len(timestamps) - 1)]
        avg_gap = sum(gaps) / len(gaps)
        max_gap = max(gaps)

        # Find maximum gap during operation
        operation_gaps = (
            gaps[operation_start:operation_end]
            if operation_end > operation_start
            else []
        )
        operation_max_gap = max(operation_gaps) if operation_gaps else 0

        print(f"Time gap analysis results:")
        print(f"  - Average gap: {avg_gap * 1000:.1f}ms")
        print(f"  - Maximum gap: {max_gap * 1000:.1f}ms")
        print(f"  - Maximum gap during operation: {operation_max_gap * 1000:.1f}ms")
        print(f"  - Number of gaps during operation: {len(operation_gaps)}")

        if (
            operation_max_gap > avg_gap * 5
        ):  # If max gap during operation exceeds average by 5x
            print(
                f"  - Assessment: Blocking detected (gap {operation_max_gap * 1000:.1f}ms exceeds expected)"
            )
        else:
            print(f"  - Assessment: No significant blocking detected")

    # Verify results
    retrieved = await store.get_numpy(ref)
    assert retrieved.shape == arr.shape
    store.print_stats()
    print(store.analyze_memory_usage())
