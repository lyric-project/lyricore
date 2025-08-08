"""
Distributed Actor Cluster Management Implementation

This module provides cluster management capabilities for distributed actor systems,
supporting FixedCluster and Ray deployment modes.
"""

import asyncio
import logging
import time
from abc import ABC, abstractmethod
from dataclasses import dataclass, field
from enum import Enum
from typing import Any, Dict, List, Optional, Type, Union

# Optional Ray import
try:
    import ray
    from ray.util.state import list_nodes

    HAS_RAY = True
except ImportError:
    HAS_RAY = False
    ray = None

logger = logging.getLogger(__name__)


# ============================================================================
# Data Structures and Enums
# ============================================================================


class NodeStatus(Enum):
    HEALTHY = "healthy"
    UNHEALTHY = "unhealthy"
    STARTING = "starting"
    STOPPING = "stopping"
    UNKNOWN = "unknown"


class ClusterMode(Enum):
    FIXED_CLUSTER = "fixed_cluster"
    KUBERNETES = "kubernetes"
    RAY = "ray"


@dataclass
class NodeInfo:
    """Information about a cluster node."""

    node_id: str
    address: str  # host:port
    capabilities: Dict[str, Any] = field(default_factory=dict)
    load: float = 0.0  # 0.0 to 1.0
    status: NodeStatus = NodeStatus.UNKNOWN
    last_heartbeat: float = field(default_factory=time.time)
    actor_count: int = 0
    metadata: Dict[str, Any] = field(default_factory=dict)

    def is_healthy(self, heartbeat_timeout: float = 30.0) -> bool:
        """Check if the node is healthy based on status and heartbeat."""
        if self.status != NodeStatus.HEALTHY:
            return False
        return time.time() - self.last_heartbeat < heartbeat_timeout


@dataclass
class SpawnConstraints:
    """Constraints for actor spawning."""

    preferred_node: Optional[str] = None
    cpu_requirements: int = 1
    gpu_requirements: int = 0
    memory_requirements: int = 0  # MB
    affinity_rules: List[str] = field(default_factory=list)
    anti_affinity_rules: List[str] = field(default_factory=list)
    max_attempts: int = 3


@dataclass
class CreateActorRequest:
    """Request to create an actor on a specific node."""

    actor_class_info: Dict[str, Any]  # ActorConstructionTask dict
    actor_path: str
    constraints: SpawnConstraints
    requester_node: str
    creation_time: float = field(default_factory=time.time)


@dataclass
class NodeStats:
    """Statistics about a node's current state."""

    node_id: str
    cpu_usage: float  # 0.0 to 1.0
    memory_usage: float  # 0.0 to 1.0
    gpu_usage: float = 0.0  # 0.0 to 1.0
    active_actors: int = 0
    pending_requests: int = 0
    last_updated: float = field(default_factory=time.time)


# ============================================================================
# Load Balancing Strategies
# ============================================================================


class LoadBalancingStrategy(ABC):
    """Abstract base class for load balancing strategies."""

    @abstractmethod
    async def select_node(
        self, nodes: List[NodeInfo], constraints: SpawnConstraints
    ) -> Optional[NodeInfo]:
        """Select the best node for spawning an actor."""
        pass


class RoundRobinStrategy(LoadBalancingStrategy):
    """Simple round-robin load balancing."""

    def __init__(self):
        self._counter = 0

    async def select_node(
        self, nodes: List[NodeInfo], constraints: SpawnConstraints
    ) -> Optional[NodeInfo]:
        if not nodes:
            return None

        # Filter nodes by constraints
        eligible_nodes = self._filter_nodes_by_constraints(nodes, constraints)
        if not eligible_nodes:
            return None

        # Round-robin selection
        selected = eligible_nodes[self._counter % len(eligible_nodes)]
        self._counter += 1
        return selected

    def _filter_nodes_by_constraints(
        self, nodes: List[NodeInfo], constraints: SpawnConstraints
    ) -> List[NodeInfo]:
        """Filter nodes based on constraints."""
        eligible = []

        for node in nodes:
            if not node.is_healthy():
                continue

            # Check preferred node
            if (
                constraints.preferred_node
                and node.node_id != constraints.preferred_node
            ):
                continue

            # Check resource requirements
            node_cpus = node.capabilities.get("cpu", 0)
            node_gpus = node.capabilities.get("gpu", 0)
            node_memory = node.capabilities.get("memory", 0)

            if (
                node_cpus >= constraints.cpu_requirements
                and node_gpus >= constraints.gpu_requirements
                and node_memory >= constraints.memory_requirements
            ):
                eligible.append(node)

        return eligible


class LeastLoadedStrategy(LoadBalancingStrategy):
    """Select the node with the lowest load."""

    async def select_node(
        self, nodes: List[NodeInfo], constraints: SpawnConstraints
    ) -> Optional[NodeInfo]:
        eligible_nodes = self._filter_nodes_by_constraints(nodes, constraints)
        if not eligible_nodes:
            return None

        # Select node with lowest load
        return min(eligible_nodes, key=lambda n: n.load)

    def _filter_nodes_by_constraints(
        self, nodes: List[NodeInfo], constraints: SpawnConstraints
    ) -> List[NodeInfo]:
        """Filter nodes based on constraints."""
        eligible = []

        for node in nodes:
            if not node.is_healthy():
                continue

            # Check preferred node
            if (
                constraints.preferred_node
                and node.node_id != constraints.preferred_node
            ):
                continue

            # Check resource requirements
            node_cpus = node.capabilities.get("cpu", 0)
            node_gpus = node.capabilities.get("gpu", 0)
            node_memory = node.capabilities.get("memory", 0)

            if (
                node_cpus >= constraints.cpu_requirements
                and node_gpus >= constraints.gpu_requirements
                and node_memory >= constraints.memory_requirements
            ):
                eligible.append(node)

        return eligible


class ResourceAwareStrategy(LoadBalancingStrategy):
    """Resource-aware load balancing considering CPU, memory, and GPU."""

    async def select_node(
        self, nodes: List[NodeInfo], constraints: SpawnConstraints
    ) -> Optional[NodeInfo]:
        eligible_nodes = self._filter_nodes_by_constraints(nodes, constraints)
        if not eligible_nodes:
            return None

        # Calculate resource score for each node
        best_node = None
        best_score = float("inf")

        for node in eligible_nodes:
            score = self._calculate_resource_score(node, constraints)
            if score < best_score:
                best_score = score
                best_node = node

        return best_node

    def _calculate_resource_score(
        self, node: NodeInfo, constraints: SpawnConstraints
    ) -> float:
        """Calculate a resource utilization score (lower is better)."""
        cpu_ratio = constraints.cpu_requirements / max(
            node.capabilities.get("cpu", 1), 1
        )
        memory_ratio = constraints.memory_requirements / max(
            node.capabilities.get("memory", 1), 1
        )
        gpu_ratio = (
            constraints.gpu_requirements / max(node.capabilities.get("gpu", 1), 1)
            if constraints.gpu_requirements > 0
            else 0
        )

        # Combine load and resource requirements
        return node.load + cpu_ratio + memory_ratio + gpu_ratio

    def _filter_nodes_by_constraints(
        self, nodes: List[NodeInfo], constraints: SpawnConstraints
    ) -> List[NodeInfo]:
        """Filter nodes based on constraints."""
        eligible = []

        for node in nodes:
            if not node.is_healthy():
                continue

            # Check preferred node
            if (
                constraints.preferred_node
                and node.node_id != constraints.preferred_node
            ):
                continue

            # Check resource requirements
            node_cpus = node.capabilities.get("cpu", 0)
            node_gpus = node.capabilities.get("gpu", 0)
            node_memory = node.capabilities.get("memory", 0)

            if (
                node_cpus >= constraints.cpu_requirements
                and node_gpus >= constraints.gpu_requirements
                and node_memory >= constraints.memory_requirements
            ):
                eligible.append(node)

        return eligible


# ============================================================================
# Abstract Cluster Manager
# ============================================================================


class ClusterManager(ABC):
    """Abstract base class for cluster managers."""

    def __init__(self, load_balancing_strategy: LoadBalancingStrategy = None):
        self.load_balancing_strategy = load_balancing_strategy or LeastLoadedStrategy()
        self.nodes: Dict[str, NodeInfo] = {}
        self.actor_locations: Dict[str, str] = {}  # actor_path -> node_id
        self._shutdown = False
        self._heartbeat_task: Optional[asyncio.Task] = None

    @abstractmethod
    async def start(self) -> None:
        """Start the cluster manager."""
        pass

    @abstractmethod
    async def shutdown(self) -> None:
        """Shutdown the cluster manager."""
        pass

    @abstractmethod
    async def discover_nodes(self) -> List[NodeInfo]:
        """Discover available nodes in the cluster."""
        pass

    async def register_node(self, node_info: NodeInfo) -> None:
        """Register a node with the cluster."""
        logger.info(f"Registering node: {node_info.node_id}")
        self.nodes[node_info.node_id] = node_info

    async def unregister_node(self, node_id: str) -> None:
        """Unregister a node from the cluster."""
        logger.info(f"Unregistering node: {node_id}")
        if node_id in self.nodes:
            del self.nodes[node_id]

        # Remove actor locations for this node
        actors_to_remove = [
            actor_path
            for actor_path, actor_node_id in self.actor_locations.items()
            if actor_node_id == node_id
        ]
        for actor_path in actors_to_remove:
            del self.actor_locations[actor_path]

    async def get_healthy_nodes(self) -> List[NodeInfo]:
        """Get a list of healthy nodes."""
        return [node for node in self.nodes.values() if node.is_healthy()]

    async def select_node_for_spawn(
        self, actor_class: Type, constraints: SpawnConstraints
    ) -> Optional[NodeInfo]:
        """Select the best node for spawning an actor."""
        healthy_nodes = await self.get_healthy_nodes()
        if not healthy_nodes:
            raise RuntimeError("No healthy nodes available for spawning")

        return await self.load_balancing_strategy.select_node(
            healthy_nodes, constraints
        )

    async def track_actor_location(self, actor_path: str, node_id: str) -> None:
        """Track the location of an actor."""
        self.actor_locations[actor_path] = node_id

        # Update node actor count
        if node_id in self.nodes:
            self.nodes[node_id].actor_count += 1

    async def find_actor_location(self, actor_path: str) -> Optional[NodeInfo]:
        """Find the node where an actor is located."""
        node_id = self.actor_locations.get(actor_path)
        if node_id and node_id in self.nodes:
            return self.nodes[node_id]
        return None

    async def update_node_stats(self, node_id: str, stats: NodeStats) -> None:
        """Update node statistics."""
        if node_id in self.nodes:
            node = self.nodes[node_id]
            node.load = max(stats.cpu_usage, stats.memory_usage)
            node.actor_count = stats.active_actors
            node.last_heartbeat = time.time()
            node.status = NodeStatus.HEALTHY

    async def _heartbeat_loop(self) -> None:
        """Background task to check node health."""
        while not self._shutdown:
            try:
                current_time = time.time()
                for node in self.nodes.values():
                    if current_time - node.last_heartbeat > 30.0:  # 30 second timeout
                        if node.status == NodeStatus.HEALTHY:
                            logger.warning(f"Node {node.node_id} appears unhealthy")
                            node.status = NodeStatus.UNHEALTHY

                await asyncio.sleep(10)  # Check every 10 seconds
            except Exception as e:
                logger.error(f"Error in heartbeat loop: {e}")


# ============================================================================
# Fixed Cluster Manager
# ============================================================================


@dataclass
class FixedClusterConfig:
    """Configuration for fixed cluster deployment."""

    nodes: List[NodeInfo]
    load_balancing_strategy: LoadBalancingStrategy = field(
        default_factory=LeastLoadedStrategy
    )
    heartbeat_interval: float = 10.0
    heartbeat_timeout: float = 30.0


class FixedClusterManager(ClusterManager):
    """Cluster manager for fixed/predefined cluster nodes."""

    def __init__(self, config: FixedClusterConfig):
        super().__init__(config.load_balancing_strategy)
        self.config = config
        self.known_nodes = {node.node_id: node for node in config.nodes}

    async def start(self) -> None:
        """Start the fixed cluster manager."""
        logger.info("Starting FixedClusterManager")

        # Initialize with known nodes
        for node in self.config.nodes:
            node.status = NodeStatus.STARTING
            await self.register_node(node)

        # Start heartbeat monitoring
        self._heartbeat_task = asyncio.create_task(self._heartbeat_loop())

        # Try to ping all nodes to check their health
        await self._initial_health_check()

    async def shutdown(self) -> None:
        """Shutdown the fixed cluster manager."""
        logger.info("Shutting down FixedClusterManager")
        self._shutdown = True

        if self._heartbeat_task:
            self._heartbeat_task.cancel()
            try:
                await self._heartbeat_task
            except asyncio.CancelledError:
                pass

    async def discover_nodes(self) -> List[NodeInfo]:
        """Return the predefined nodes."""
        return list(self.known_nodes.values())

    async def scale_out(self, target_nodes: int) -> None:
        """Scale out is not supported in fixed cluster mode."""
        current_nodes = len(self.known_nodes)
        if target_nodes > current_nodes:
            logger.warning(
                f"Cannot scale out from {current_nodes} to {target_nodes} nodes "
                "in fixed cluster mode. Add more nodes to configuration."
            )

    async def scale_in(self, target_nodes: int) -> None:
        """Scale in by marking nodes as unavailable."""
        current_healthy = len(await self.get_healthy_nodes())
        if target_nodes < current_healthy:
            nodes_to_remove = current_healthy - target_nodes
            healthy_nodes = await self.get_healthy_nodes()

            # Mark the least loaded nodes as stopping
            nodes_by_load = sorted(healthy_nodes, key=lambda n: n.load)
            for i in range(min(nodes_to_remove, len(nodes_by_load))):
                node = nodes_by_load[i]
                node.status = NodeStatus.STOPPING
                logger.info(f"Marking node {node.node_id} as stopping")

    async def _initial_health_check(self) -> None:
        """Perform initial health check on all known nodes."""
        # In a real implementation, you would ping each node's ActorFactory
        # For now, we'll assume all nodes are healthy
        for node in self.known_nodes.values():
            node.status = NodeStatus.HEALTHY
            node.last_heartbeat = time.time()
            logger.info(f"Node {node.node_id} marked as healthy")


# ============================================================================
# Ray Cluster Manager
# ============================================================================


@dataclass
class RayClusterConfig:
    """Configuration for Ray cluster deployment."""

    ray_address: Optional[str] = None  # None for local cluster
    load_balancing_strategy: LoadBalancingStrategy = field(
        default_factory=ResourceAwareStrategy
    )
    heartbeat_interval: float = 10.0
    auto_scaling: bool = True
    min_nodes: int = 1
    max_nodes: int = 10


class RayClusterManager(ClusterManager):
    """Cluster manager for Ray-based deployment."""

    def __init__(self, config: RayClusterConfig):
        if not HAS_RAY:
            raise RuntimeError(
                "Ray is not installed. Please install ray: pip install ray"
            )

        super().__init__(config.load_balancing_strategy)
        self.config = config
        self._ray_initialized = False
        self._discovery_task: Optional[asyncio.Task] = None

    async def start(self) -> None:
        """Start the Ray cluster manager."""
        logger.info("Starting RayClusterManager")

        # Initialize Ray if not already initialized
        if not ray.is_initialized():
            if self.config.ray_address:
                ray.init(address=self.config.ray_address)
            else:
                ray.init()
            self._ray_initialized = True
            logger.info(f"Connected to Ray cluster: {ray.get_runtime_context()}")

        # Start node discovery and heartbeat monitoring
        self._discovery_task = asyncio.create_task(self._discovery_loop())
        self._heartbeat_task = asyncio.create_task(self._heartbeat_loop())

        # Initial node discovery
        await self._discover_ray_nodes()

    async def shutdown(self) -> None:
        """Shutdown the Ray cluster manager."""
        logger.info("Shutting down RayClusterManager")
        self._shutdown = True

        # Cancel background tasks
        for task in [self._discovery_task, self._heartbeat_task]:
            if task:
                task.cancel()
                try:
                    await task
                except asyncio.CancelledError:
                    pass

        # Shutdown Ray if we initialized it
        if self._ray_initialized and ray.is_initialized():
            ray.shutdown()

    async def discover_nodes(self) -> List[NodeInfo]:
        """Discover Ray cluster nodes."""
        await self._discover_ray_nodes()
        return list(self.nodes.values())

    async def scale_out(self, target_nodes: int) -> None:
        """Scale out using Ray's autoscaling capabilities."""
        if not self.config.auto_scaling:
            logger.warning("Auto-scaling is disabled in configuration")
            return

        current_nodes = len(await self.get_healthy_nodes())
        if target_nodes > current_nodes:
            # Ray handles autoscaling automatically based on resource demands
            # We can request resources to trigger scaling
            nodes_needed = min(
                target_nodes - current_nodes, self.config.max_nodes - current_nodes
            )

            if nodes_needed > 0:
                logger.info(f"Requesting {nodes_needed} additional nodes from Ray")
                # This would trigger Ray's autoscaling
                # In practice, you might use ray.autoscaler.commands or resource requests

    async def scale_in(self, target_nodes: int) -> None:
        """Scale in by reducing resource demands."""
        current_nodes = len(await self.get_healthy_nodes())
        if target_nodes < current_nodes and target_nodes >= self.config.min_nodes:
            nodes_to_remove = current_nodes - target_nodes
            logger.info(f"Reducing cluster size by {nodes_to_remove} nodes")
            # Ray will automatically scale down idle nodes

    async def _discover_ray_nodes(self) -> None:
        """Discover nodes in the Ray cluster."""
        try:
            # Get node information from Ray
            ray_nodes = list_nodes()

            for ray_node in ray_nodes:
                if ray_node["state"] != "ALIVE":
                    continue

                node_id = ray_node["node_id"]

                # Extract node information
                resources = ray_node.get("resources", {})
                node_info = NodeInfo(
                    node_id=node_id,
                    address=f"{ray_node.get('node_manager_address', 'unknown')}:50051",
                    capabilities={
                        "cpu": int(resources.get("CPU", 0)),
                        "gpu": int(resources.get("GPU", 0)),
                        "memory": int(
                            resources.get("memory", 0) / (1024 * 1024)
                        ),  # Convert to MB
                        "object_store_memory": int(
                            resources.get("object_store_memory", 0) / (1024 * 1024)
                        ),
                    },
                    status=NodeStatus.HEALTHY,
                    last_heartbeat=time.time(),
                    metadata={"ray_node_id": node_id, "ray_state": ray_node["state"]},
                )

                # Update or add node
                if node_id in self.nodes:
                    # Update existing node
                    existing_node = self.nodes[node_id]
                    existing_node.capabilities = node_info.capabilities
                    existing_node.last_heartbeat = time.time()
                    existing_node.status = NodeStatus.HEALTHY
                else:
                    # Add new node
                    await self.register_node(node_info)

            # Remove nodes that are no longer in Ray cluster
            ray_node_ids = {
                node["node_id"] for node in ray_nodes if node["state"] == "ALIVE"
            }
            nodes_to_remove = []
            for node_id in self.nodes:
                if self.nodes[node_id].metadata.get("ray_node_id") not in ray_node_ids:
                    nodes_to_remove.append(node_id)

            for node_id in nodes_to_remove:
                await self.unregister_node(node_id)

        except Exception as e:
            logger.error(f"Error discovering Ray nodes: {e}")

    async def _discovery_loop(self) -> None:
        """Background task to periodically discover Ray nodes."""
        while not self._shutdown:
            try:
                await self._discover_ray_nodes()
                await asyncio.sleep(self.config.heartbeat_interval)
            except Exception as e:
                logger.error(f"Error in Ray node discovery loop: {e}")
                await asyncio.sleep(5)  # Shorter retry interval on error


def create_cluster_manager(
    mode: ClusterMode, config: Union[FixedClusterConfig, RayClusterConfig]
) -> ClusterManager:
    """Factory function to create cluster managers."""
    if mode == ClusterMode.FIXED_CLUSTER:
        if not isinstance(config, FixedClusterConfig):
            raise ValueError("FixedClusterConfig required for FIXED_CLUSTER mode")
        return FixedClusterManager(config)

    elif mode == ClusterMode.RAY:
        if not isinstance(config, RayClusterConfig):
            raise ValueError("RayClusterConfig required for RAY mode")
        return RayClusterManager(config)

    else:
        raise ValueError(f"Unsupported cluster mode: {mode}")
