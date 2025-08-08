from contextlib import asynccontextmanager

import pytest
import pytest_asyncio

from lyricore.py_actor import ActorSystem


@asynccontextmanager
async def start_actor_system(**kwargs):
    """启动测试用的 Actor 系统"""
    system_name = kwargs.pop("system_name", "test_system")
    listen_address = kwargs.pop("listen_address", "127.0.0.1:50051")
    system = ActorSystem(system_name, listen_address, **kwargs)
    await system.start()
    try:
        yield system
    finally:
        await system.shutdown()


@pytest_asyncio.fixture
async def actor_system(request):
    """测试 Actor System"""
    param = getattr(request, "param", {})
    async with start_actor_system(**param) as actor_system:
        yield actor_system


@pytest_asyncio.fixture
async def actor_multiple_systems(request):
    """测试 Actor System 多个实例.
    返回 ActorSystem 列表"""

    param = getattr(request, "param", {})
    systems = []
    for i in range(3):
        system_name = f"test_system_{i}"
        listen_address = f"127.0.0.1:{50051 + i}"
        system = ActorSystem(system_name, listen_address, **param)
        await system.start()
        systems.append(system)
    # Connect all systems to each other
    for i in range(len(systems)):
        for j in range(len(systems)):
            if i != j:
                await systems[i].connect_to_node(
                    systems[j].system_name, systems[j].listen_address
                )
                await systems[j].connect_to_node(
                    systems[i].system_name, systems[i].listen_address
                )
    try:
        yield systems
    finally:
        for system in systems:
            await system.shutdown()
