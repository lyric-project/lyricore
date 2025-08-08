"""
Actor Factory and Distributed Actor System Implementation

This module provides the ActorFactory for remote actor creation and
the DistributedActorSystem that integrates with cluster managers.
"""

import asyncio
import logging
import time
from typing import Any, Dict, List, Optional, Type, Union

import psutil

from ..actor_wrapper import (
    _create_actor_init_dict,
    _wrap_actor_class,
)
from ..proxy_ref import EnhancedObjectStoreActorRef
from ..py_actor import ActorContext, ActorRef, ActorSystem
from .cluster_manager import (
    ClusterManager,
    ClusterMode,
    CreateActorRequest,
    FixedClusterConfig,
    NodeInfo,
    NodeStats,
    NodeStatus,
    RayClusterConfig,
    SpawnConstraints,
    create_cluster_manager,
)

logger = logging.getLogger(__name__)


# ============================================================================
# Actor Factory
# ============================================================================


class ActorFactory:
    """
    Factory for creating and managing actors on a cluster node.
    This actor runs on each node and handles remote actor creation requests.
    """

    def __init__(self, node_id: str):
        # self.actor_system = actor_system
        self.node_id = node_id
        self.local_actors: Dict[str, ActorRef] = {}
        self.creation_requests: Dict[str, CreateActorRequest] = {}
        self.stats = NodeStats(node_id=node_id, cpu_usage=1.0, memory_usage=1.0)

    async def on_start(self, ctx: ActorContext):
        """Called when the ActorFactory starts."""
        logger.info(f"ActorFactory started on node {self.node_id}")

        # Start periodic stats reporting
        asyncio.create_task(self._stats_reporting_loop(ctx))

    # @on
    async def handle_message(self, message: Any, ctx: ActorContext):
        """Handle incoming messages."""
        if isinstance(message, dict):
            if message.get("type") == "create_actor":
                return await self.create_actor(
                    CreateActorRequest(**message["request"]), ctx
                )
            elif message.get("type") == "destroy_actor":
                return await self.destroy_actor(message["actor_path"])
            elif message.get("type") == "get_stats":
                return await self.get_node_stats()
            elif message.get("type") == "list_actors":
                return await self.list_local_actors()

        logger.warning(f"Unknown message type received: {message}")
        return {"status": "error", "message": "Unknown message type"}

    async def create_actor(
        self, request: CreateActorRequest, ctx: ActorContext
    ) -> Dict[str, Any]:
        """Create an actor based on the request."""
        try:
            logger.info(
                f"Creating actor {request.actor_path} from node {request.requester_node}"
            )

            # Store the creation request
            self.creation_requests[request.actor_path] = request

            # Deserialize the actor construction task
            construction_task = request.actor_class_info

            # Extract class information
            _module_name = construction_task.get("module_name")
            _class_name = construction_task.get("class_name")

            # Spawn the actor in the local system
            actor_ref = await ctx._spawn_construction_task(
                construction_task, request.actor_path
            )

            # Track the local actor
            self.local_actors[request.actor_path] = actor_ref
            self.stats.active_actors += 1

            logger.info(f"Successfully created actor {request.actor_path}")

            return {
                "status": "success",
                "actor_path": request.actor_path,
                "node_id": self.node_id,
                "creation_time": time.time(),
            }

        except Exception as e:
            logger.error(f"Failed to create actor {request.actor_path}: {e}")
            return {
                "status": "error",
                "actor_path": request.actor_path,
                "error": str(e),
                "node_id": self.node_id,
            }

    async def destroy_actor(self, actor_path: str) -> Dict[str, Any]:
        """Destroy a local actor."""
        try:
            if actor_path in self.local_actors:
                actor_ref = self.local_actors[actor_path]
                actor_ref.stop()
                del self.local_actors[actor_path]
                self.stats.active_actors -= 1

                # Clean up creation request
                if actor_path in self.creation_requests:
                    del self.creation_requests[actor_path]

                logger.info(f"Successfully destroyed actor {actor_path}")
                return {
                    "status": "success",
                    "actor_path": actor_path,
                    "node_id": self.node_id,
                }
            else:
                return {
                    "status": "error",
                    "actor_path": actor_path,
                    "error": "Actor not found",
                    "node_id": self.node_id,
                }

        except Exception as e:
            logger.error(f"Failed to destroy actor {actor_path}: {e}")
            return {
                "status": "error",
                "actor_path": actor_path,
                "error": str(e),
                "node_id": self.node_id,
            }

    async def get_node_stats(self) -> NodeStats:
        """Get current node statistics."""
        try:
            # Update system resource usage
            cpu_percent = psutil.cpu_percent(interval=0.1)
            memory = psutil.virtual_memory()

            self.stats.cpu_usage = cpu_percent / 100.0
            self.stats.memory_usage = memory.percent / 100.0
            self.stats.active_actors = len(self.local_actors)
            self.stats.pending_requests = len(self.creation_requests)
            self.stats.last_updated = time.time()

            # GPU usage (requires nvidia-ml-py if available)
            try:
                import pynvml

                pynvml.nvmlInit()
                handle = pynvml.nvmlDeviceGetHandleByIndex(0)
                gpu_util = pynvml.nvmlDeviceGetUtilizationRates(handle)
                self.stats.gpu_usage = gpu_util.gpu / 100.0
            except:
                self.stats.gpu_usage = 0.0

            return self.stats

        except Exception as e:
            logger.error(f"Error getting node stats: {e}")
            return self.stats

    async def list_local_actors(self) -> List[str]:
        """List all actors running on this node."""
        return list(self.local_actors.keys())

    async def _stats_reporting_loop(self, ctx: ActorContext):
        """Periodically report node statistics."""
        while True:
            try:
                await asyncio.sleep(30)  # Report every 30 seconds
                stats = await self.get_node_stats()
                logger.debug(
                    f"Node stats: CPU={stats.cpu_usage:.1%}, "
                    f"Memory={stats.memory_usage:.1%}, "
                    f"Actors={stats.active_actors}"
                )

                # In a real implementation, you might send these stats
                # to the cluster manager for load balancing decisions

            except Exception as e:
                logger.error(f"Error in stats reporting loop: {e}")


# ============================================================================
# Distributed Actor System
# ============================================================================


class DistributedActorSystem(ActorSystem):
    """
    Enhanced ActorSystem with distributed capabilities.
    Supports spawning actors on remote nodes through cluster management.
    """

    def __init__(
        self,
        system_name: str,
        cluster_mode: ClusterMode,
        cluster_config: Union[FixedClusterConfig, RayClusterConfig],
        listen_address: str = "127.0.0.1:50051",
        node_id: Optional[str] = None,
        **kwargs,
    ):
        super().__init__(system_name, listen_address, **kwargs)
        self.cluster_mode = cluster_mode
        self.cluster_config = cluster_config
        self.node_id = node_id or f"{system_name}_{listen_address.replace(':', '_')}"
        self.cluster_manager: Optional[ClusterManager] = None
        self.local_factory: Optional[ActorRef] = None
        self._factory_refs: Dict[str, ActorRef] = {}  # node_id -> factory_ref
        self._is_connected = False

    async def start(self) -> None:
        """Start the distributed actor system."""
        logger.info(f"Starting DistributedActorSystem: {self.system_name}")

        # Start the base actor system
        await super().start()

        # Create and start cluster manager
        self.cluster_manager = create_cluster_manager(
            self.cluster_mode, self.cluster_config
        )
        await self.cluster_manager.start()

        # Start local ActorFactory
        self.local_factory = await super().spawn(
            ActorFactory, "/system/factory", self.node_id
        )
        logger.info(
            f"Local ActorFactory({self.local_factory.path}) started on node {self.node_id}"
        )

        # Register this node with the cluster
        local_node_info = await self._get_local_node_info()
        await self.cluster_manager.register_node(local_node_info)

    async def after_start(self) -> None:
        # Connect to other nodes if this is a fixed cluster
        if self.cluster_mode == ClusterMode.FIXED_CLUSTER:
            await self._connect_to_fixed_cluster_nodes()

        logger.info(f"DistributedActorSystem started successfully")

    async def shutdown(self) -> None:
        """Shutdown the distributed actor system."""
        logger.info("Shutting down DistributedActorSystem")

        # Unregister from cluster
        if self.cluster_manager:
            await self.cluster_manager.unregister_node(self.node_id)
            await self.cluster_manager.shutdown()

        # Shutdown the base actor system
        await super().shutdown()

    async def spawn(
        self,
        actor_class: Type,
        path: str,
        *args,
        constraints: Optional[SpawnConstraints] = None,
        **kwargs,
    ) -> EnhancedObjectStoreActorRef:
        """
        Enhanced spawn method that supports distributed actor creation.

        Args:
            actor_class: The actor class to spawn
            path: Actor path
            *args: Constructor arguments
            constraints: Spawning constraints for node selection
            **kwargs: Constructor keyword arguments

        Returns:
            EnhancedObjectStoreActorRef: Reference to the created actor
        """
        constraints = constraints or SpawnConstraints()

        # Select target node for spawning
        target_node = await self.cluster_manager.select_node_for_spawn(
            actor_class, constraints
        )

        if not target_node:
            raise RuntimeError("No suitable node found for spawning actor")

        if target_node.node_id == self.node_id:
            # Local spawning
            logger.debug(f"Spawning actor {path} locally")
            return await self._spawn_local(actor_class, path, *args, **kwargs)
        else:
            # Remote spawning
            logger.debug(f"Spawning actor {path} on remote node {target_node.node_id}")
            return await self._spawn_remote(
                actor_class, path, target_node, constraints, *args, **kwargs
            )

    async def _spawn_local(
        self, actor_class: Type, path: str, *args, **kwargs
    ) -> EnhancedObjectStoreActorRef:
        """Spawn an actor locally."""
        # Use the parent class spawn method
        result = await super().spawn(actor_class, path, *args, **kwargs)

        # Track the actor location
        await self.cluster_manager.track_actor_location(path, self.node_id)

        return result

    async def _spawn_remote(
        self,
        actor_class: Type,
        path: str,
        target_node: NodeInfo,
        constraints: SpawnConstraints,
        *args,
        **kwargs,
    ) -> EnhancedObjectStoreActorRef:
        """Spawn an actor on a remote node."""
        try:
            # Get or create connection to remote factory
            factory_ref = await self._get_factory_ref(target_node)

            # Prepare the creation request
            wrapped_class = _wrap_actor_class(actor_class)
            construction_task = _create_actor_init_dict(
                wrapped_class, self.objectstore_config, *args, **kwargs
            )

            request = CreateActorRequest(
                actor_class_info=construction_task,
                actor_path=path,
                constraints=constraints,
                requester_node=self.node_id,
            )

            # Send creation request to remote factory
            create_message = {
                "type": "create_actor",
                "request": {
                    "actor_class_info": request.actor_class_info,
                    "actor_path": request.actor_path,
                    "constraints": request.constraints.__dict__,
                    "requester_node": request.requester_node,
                },
            }

            response = await factory_ref.ask(create_message, timeout=30.0)

            logger.debug(f"Remote actor creation response: {response}")

            if response.get("status") != "success":
                raise RuntimeError(
                    f"Failed to create remote actor: {response.get('error', 'Unknown error')}"
                )

            # Track actor location
            await self.cluster_manager.track_actor_location(path, target_node.node_id)

            # Create reference to remote actor
            remote_path = (
                f"lyricore://{target_node.node_id}@{target_node.address}{path}"
            )
            base_ref = await self.actor_of(remote_path)

            return base_ref

        except Exception as e:
            logger.error(
                f"Failed to spawn remote actor {path} on {target_node.node_id}: {e}"
            )
            raise RuntimeError(f"Remote spawn failed: {e}")

    async def actor_of(self, path: str) -> EnhancedObjectStoreActorRef:
        """
        Enhanced actor_of that can handle both local and remote actor references.
        """
        # If it's already a full remote path, use parent implementation
        if path.startswith("lyricore://"):
            base_ref = await super().actor_of(path)
            return base_ref

        # Check if we know where this actor is located
        location = await self.cluster_manager.find_actor_location(path)

        if location and location.node_id != self.node_id:
            # Actor is on a remote node
            remote_path = f"lyricore://{location.node_id}@{location.address}{path}"
            return await super().actor_of(remote_path)
        else:
            # Actor is local or location unknown, try local first
            return await super().actor_of(path)

    async def _get_factory_ref(self, node: NodeInfo) -> ActorRef:
        """Get a reference to a remote node's ActorFactory."""
        if node.node_id in self._factory_refs:
            return self._factory_refs[node.node_id]

        # Create connection to remote factory
        factory_path = f"lyricore://{node.node_id}@{node.address}/system/factory"

        try:
            factory_ref = await super().actor_of(factory_path)
            self._factory_refs[node.node_id] = factory_ref
            return factory_ref
        except Exception as e:
            logger.error(f"Failed to connect to factory on {node.node_id}: {e}")
            raise RuntimeError(f"Cannot connect to remote factory: {e}")

    async def _get_local_node_info(self) -> NodeInfo:
        """Get information about the local node."""
        try:
            # Get system information
            cpu_count = psutil.cpu_count()
            memory = psutil.virtual_memory()

            # Try to get GPU information
            gpu_count = 0
            try:
                import pynvml

                pynvml.nvmlInit()
                gpu_count = pynvml.nvmlDeviceGetCount()
            except:
                pass

            capabilities = {
                "cpu": cpu_count,
                "memory": int(memory.total / (1024 * 1024)),  # MB
                "gpu": gpu_count,
            }

            return NodeInfo(
                node_id=self.node_id,
                address=self.listen_address,
                capabilities=capabilities,
                status=NodeStatus.HEALTHY,
                last_heartbeat=time.time(),
            )

        except Exception as e:
            logger.error(f"Error getting local node info: {e}")
            # Return minimal node info
            return NodeInfo(
                node_id=self.node_id,
                address=self.listen_address,
                capabilities={"cpu": 1, "memory": 1024, "gpu": 0},
                status=NodeStatus.HEALTHY,
            )

    async def _connect_to_fixed_cluster_nodes(self) -> None:
        """Connect to other nodes in a fixed cluster."""
        if not isinstance(self.cluster_config, FixedClusterConfig):
            return

        for node in self.cluster_config.nodes:
            if node.node_id != self.node_id:
                try:
                    await self.connect_to_node(node.node_id, node.address)
                    logger.info(f"Connected to cluster node: {node.node_id}")
                except Exception as e:
                    logger.warning(f"Failed to connect to node {node.node_id}: {e}")


# ============================================================================
# Utility Functions
# ============================================================================


async def create_distributed_actor_system(
    system_name: str,
    cluster_mode: ClusterMode,
    cluster_config: Union[FixedClusterConfig, RayClusterConfig],
    listen_address: str = "127.0.0.1:50051",
    node_id: Optional[str] = None,
    **kwargs,
) -> DistributedActorSystem:
    """
    Factory function to create and start a distributed actor system.

    Args:
        system_name: Name of the actor system
        cluster_mode: Type of cluster deployment
        cluster_config: Configuration for the cluster
        listen_address: Address to listen on
        node_id: Optional node identifier
        **kwargs: Additional arguments for ActorSystem

    Returns:
        Started DistributedActorSystem instance
    """
    system = DistributedActorSystem(
        system_name=system_name,
        cluster_mode=cluster_mode,
        cluster_config=cluster_config,
        listen_address=listen_address,
        node_id=node_id,
        **kwargs,
    )

    await system.start()
    return system
