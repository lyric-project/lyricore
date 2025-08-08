use crate::actor_ref::{ActorRef, LocalActorRef, RemoteActorRef, StreamingRemoteActorRef};
use crate::error::LyricoreActorError;
use crate::error::Result;
use crate::message::InboxMessage;
use crate::path::{ActorAddress, ActorPath};
use crate::rpc::actor_service;
use crate::rpc::actor_service::actor_service_client::ActorServiceClient;
use crate::rpc::actor_service::actor_service_server::ActorServiceServer;
use crate::rpc_actor_service::ActorServiceImpl;
use crate::scheduler::{
    SchedulerCommand, SchedulerConfig, SchedulerJobRequest, WorkItem, WorkScheduler,
};
use crate::serialization::{MessageRegistry, SerFormat, SerializationStrategy};
use crate::stream::{StreamConfig, StreamConnectionManager};
use crate::{Actor, ActorId, Message};
use dashmap::DashMap;
use std::str::FromStr;
use std::sync::Arc;
use tokio::net::lookup_host;
use tokio::sync::{mpsc, oneshot};
use tonic::transport::Server;

/// NodeRegistry manages the registration and information of remote nodes in the actor system.
#[derive(Clone, Debug)]
pub struct NodeRegistry {
    nodes: Arc<DashMap<String, NodeInfo>>,
}

/// NodeInfo holds information about a registered node.
#[derive(Clone, Debug)]
pub struct NodeInfo {
    pub node_id: String,
    pub address: String,
    pub client: ActorServiceClient<tonic::transport::Channel>,
    pub last_seen: std::time::SystemTime,
    pub capabilities: Option<actor_service::NodeCapabilities>,
}

impl NodeRegistry {
    pub fn new() -> Self {
        Self {
            nodes: Arc::new(DashMap::new()),
        }
    }

    pub async fn register_node(&self, node_id: String, address: String) -> Result<()> {
        let client = ActorServiceClient::connect(format!("http://{}", address))
            .await
            .map_err(|e| {
                LyricoreActorError::Actor(crate::error::ActorError::RpcError(e.to_string()))
            })?;

        let node_key = format!("{}@{}", node_id, address);
        self.nodes.insert(
            node_key,
            NodeInfo {
                node_id,
                address,
                client,
                last_seen: std::time::SystemTime::now(),
                capabilities: None, // Use ping to fetch capabilities later if needed
            },
        );

        Ok(())
    }

    pub fn get_node(&self, node_id: &str) -> Option<NodeInfo> {
        self.nodes.get(node_id).map(|entry| entry.clone())
    }

    pub fn remove_node(&self, node_id: &str) {
        self.nodes.remove(node_id);
    }
}

#[derive(Debug)]
pub(crate) struct ActorSystemInner {
    pub(crate) message_registry: Arc<MessageRegistry>,
    node_registry: Arc<NodeRegistry>,
    /// System name, e.g., "my_actor_system"
    pub system_name: String,
    pub system_address: ActorAddress,

    pub(crate) job_sender: mpsc::UnboundedSender<SchedulerJobRequest>,
    scheduler_cmd_sender: mpsc::UnboundedSender<SchedulerCommand>,

    pub(crate) local_actors: Arc<DashMap<ActorId, LocalActorRef>>,
    pub(crate) path_to_runtime: Arc<DashMap<String, ActorId>>, // full_path -> runtime_id
    pub(crate) remote_actors: Arc<DashMap<String, StreamingRemoteActorRef>>, // full_path -> remote_ref
    stream_manager: Arc<StreamConnectionManager>,
}

pub struct ActorSystem {
    server_handle: Option<tokio::task::JoinHandle<()>>,
    shutdown: Option<oneshot::Sender<()>>,
    inner: Arc<ActorSystemInner>,
    scheduler: Arc<WorkScheduler>,
}

impl ActorSystemInner {
    pub(crate) async fn actor_of_path(&self, path: &ActorPath) -> Result<ActorRef> {
        if path.is_local(&self.system_name, &self.system_address) {
            // Local actor
            let full_path = path.full_path();
            if let Some(runtime_id) = self.path_to_runtime.get(&full_path) {
                if let Some(actor_ref) = self.local_actors.get(&*runtime_id) {
                    return Ok(ActorRef::Local(actor_ref.clone()));
                }
            }
            Err(LyricoreActorError::Actor(
                crate::error::ActorError::ActorNotFound(full_path),
            ))
        } else {
            // Remote actor
            let full_path = path.full_path();
            if let Some(remote_ref) = self.remote_actors.get(&full_path) {
                Ok(ActorRef::Remote(remote_ref.clone()))
            } else {
                // Create new remote actor reference
                let actor_id = ActorId::generate(path.clone());
                let remote_ref = RemoteActorRef::new(
                    actor_id.clone(),
                    Arc::clone(&self.node_registry),
                    Arc::clone(&self.message_registry),
                );
                let stream_remote_ref = StreamingRemoteActorRef::new(
                    actor_id,
                    Arc::clone(&self.stream_manager),
                    Arc::clone(&self.message_registry),
                    Some(remote_ref),
                );

                self.remote_actors
                    .insert(full_path, stream_remote_ref.clone());
                Ok(ActorRef::Remote(stream_remote_ref))
            }
        }
    }
    // Create a local actor at the specified path
    pub(crate) fn spawn_at_path<A: Actor + 'static>(
        &self,
        path: ActorPath,
        actor: A,
    ) -> Result<ActorRef> {
        // Check if the actor already exists at the specified path
        let full_path = path.full_path();
        if self.path_to_runtime.contains_key(&full_path) {
            return Err(LyricoreActorError::Actor(
                crate::error::ActorError::ActorCreationFailed(format!(
                    "Actor already exists at path: {}",
                    full_path
                )),
            ));
        }

        // Create a new ActorId for the actor
        let actor_id = ActorId::generate(path);

        let local_ref = LocalActorRef::new(
            actor_id.clone(),
            actor,
            self.job_sender.clone(),
            Arc::clone(&self.message_registry),
        );

        // Register the actor with the scheduler
        let _ = self.job_sender.send(SchedulerJobRequest::RegisterActor {
            actor_id: actor_id.clone(),
            actor_ref: local_ref.clone(),
        });

        // Send an initial message to start the actor
        let work_item = WorkItem::new(actor_id.clone(), Box::new(InboxMessage::OnStart));
        let _ = self.job_sender.send(SchedulerJobRequest::SubmitWork {
            work_item,
            remote: false,
        });

        // Register the actor in local actors and path to runtime
        self.local_actors
            .insert(actor_id.clone(), local_ref.clone());
        self.path_to_runtime.insert(full_path, actor_id);

        Ok(ActorRef::Local(local_ref))
    }

    pub(crate) fn spawn_local<A: Actor + 'static>(&self, actor_id: String, actor: A) -> ActorRef {
        let path = ActorPath::new(
            self.system_name.clone(),
            self.system_address.clone(),
            format!("/user/{}", actor_id),
        );
        self.spawn_at_path(path, actor).unwrap()
    }

    pub(crate) async fn actor_of(&self, actor_id: ActorId) -> Result<ActorRef> {
        self.actor_of_path(&actor_id.path).await
    }
    pub async fn actor_of_str(&self, path_str: &str) -> Result<ActorRef> {
        let path = if path_str.starts_with("lyricore://") {
            ActorPath::parse_with_default(path_str, &self.system_address)?
        } else if path_str.starts_with('/') {
            // Absolute path, e.g., "/user/my_actor"
            ActorPath::new(
                self.system_name.clone(),
                self.system_address.clone(),
                path_str.to_string(),
            )
        } else {
            // Relative paths are resolved based on the system's user path
            ActorPath::new(
                self.system_name.clone(),
                self.system_address.clone(),
                format!("/user/{}", path_str),
            )
        };
        self.actor_of_path(&path).await
    }

    // Stops the actor at the specified path.
    pub fn stop_actor_at_path(&self, path: &ActorPath) -> Result<()> {
        let full_path = path.full_path();

        if let Some((_, runtime_id)) = self.path_to_runtime.remove(&full_path) {
            if let Some((_, actor_ref)) = self.local_actors.remove(&runtime_id) {
                actor_ref.stop();
                let _ = self.job_sender.send(SchedulerJobRequest::UnregisterActor {
                    actor_id: runtime_id,
                });
                Ok(())
            } else {
                Err(LyricoreActorError::Actor(
                    crate::error::ActorError::ActorNotFound("Runtime actor not found".to_string()),
                ))
            }
        } else {
            Err(LyricoreActorError::Actor(
                crate::error::ActorError::ActorNotFound(full_path),
            ))
        }
    }
}

impl ActorSystem {
    pub fn new_optimized(
        node_id: String,
        listen_address: String,
        config: SchedulerConfig,
        preferred_format: SerFormat,
    ) -> Result<Self> {
        Self::new_optimized_with_name(node_id, listen_address, config, preferred_format)
    }

    pub fn new_with_name(
        system_name: String,
        listen_address: String,
        config: SchedulerConfig,
        serialization_strategy: Option<SerializationStrategy>,
    ) -> Result<Self> {
        let address = ActorAddress::from_str(&listen_address)?;
        // TODO: Pass stream configuration if needed
        Self::new_internal(system_name, address, config, None, serialization_strategy)
    }

    pub fn new_optimized_with_name(
        system_name: String,
        listen_address: String,
        config: SchedulerConfig,
        preferred_format: SerFormat,
    ) -> Result<Self> {
        let strategy = match preferred_format {
            SerFormat::Json => SerializationStrategy::fast_json(),
            SerFormat::Messagepack => SerializationStrategy::messagepack(),
            _ => SerializationStrategy::default(),
        };
        Self::new_with_name(system_name, listen_address, config, Some(strategy))
    }
    pub fn new(
        node_id: String,
        listen_address: String,
        config: SchedulerConfig,
        serialization_strategy: Option<SerializationStrategy>,
    ) -> Result<Self> {
        Self::new_with_name(node_id, listen_address, config, serialization_strategy)
    }

    fn new_internal(
        system_name: String,
        system_address: ActorAddress,
        config: SchedulerConfig,
        stream_config: Option<StreamConfig>,
        serialization_strategy: Option<SerializationStrategy>,
    ) -> Result<Self> {
        let (scheduler_cmd_tx, scheduler_cmd_rx) = mpsc::unbounded_channel();
        let (job_request_tx, job_request_rx) = mpsc::unbounded_channel();
        let node_registry = Arc::new(NodeRegistry::new());
        let strategy = serialization_strategy.unwrap_or_default();
        let message_registry = Arc::new(MessageRegistry::new(strategy));
        let stream_manager = Arc::new(StreamConnectionManager::new(
            stream_config.unwrap_or_default(),
        ));

        let inner = Arc::new(ActorSystemInner {
            system_name,
            system_address,
            job_sender: job_request_tx.clone(),
            scheduler_cmd_sender: scheduler_cmd_tx.clone(),
            node_registry,
            message_registry,
            local_actors: Arc::new(DashMap::new()),
            path_to_runtime: Arc::new(DashMap::new()),
            remote_actors: Arc::new(DashMap::new()),
            stream_manager,
        });

        let scheduler = WorkScheduler::new(
            Arc::clone(&inner),
            config,
            scheduler_cmd_tx,
            scheduler_cmd_rx,
            job_request_tx,
            job_request_rx,
        );

        let system = Self {
            server_handle: None,
            shutdown: None,
            inner,
            scheduler,
        };

        Ok(system)
    }

    /// Registers a message type with the actor system.
    pub fn register_message_type<T: Message>(&mut self, _preferred_format: Option<SerFormat>) {
        todo!()
    }

    pub async fn start_server(&mut self) -> Result<()> {
        let addr_str = format!(
            "{}:{}",
            self.inner.system_address.host, self.inner.system_address.port
        );
        let mut addrs = lookup_host(&addr_str).await.map_err(|e| {
            LyricoreActorError::Actor(crate::error::ActorError::RpcError(format!(
                "DNS resolution failed: {}",
                e
            )))
        })?;

        let addr = addrs.next().ok_or_else(|| {
            LyricoreActorError::Actor(crate::error::ActorError::RpcError(
                "No valid address found".to_string(),
            ))
        })?;

        let concurrency_limit = 2000;
        let ask_timeout = std::time::Duration::from_secs(30);
        let service = ActorServiceImpl::new(
            self.inner.job_sender.clone(),
            Arc::clone(&self.inner.message_registry),
            concurrency_limit,
            ask_timeout,
        );
        let (tx_rpc_ready, _) = oneshot::channel();
        let (tx, rx) = oneshot::channel();
        let server_handle = tokio::spawn(async move {
            let res = Server::builder()
                .add_service(ActorServiceServer::new(service))
                .serve_with_shutdown(addr, async {
                    let _ = tx_rpc_ready.send(());
                    rx.await.ok();
                })
                .await;
            match res {
                Ok(_) => tracing::debug!("Actor service server stopped gracefully."),
                Err(e) => tracing::error!("Actor service server error: {}", e),
            }
        });
        self.shutdown = Some(tx);
        self.server_handle = Some(server_handle);
        Ok(())
    }

    pub fn spawn_at_path<A: Actor + 'static>(&self, path: ActorPath, actor: A) -> Result<ActorRef> {
        self.inner.spawn_at_path(path, actor)
    }
    pub fn spawn_at<A: Actor + 'static>(&self, path_str: &str, actor: A) -> Result<ActorRef> {
        let path = if path_str.starts_with('/') {
            ActorPath::new(
                self.inner.system_name.clone(),
                self.inner.system_address.clone(),
                path_str.to_string(),
            )
        } else {
            ActorPath::new(
                self.inner.system_name.clone(),
                self.inner.system_address.clone(),
                format!("/user/{}", path_str),
            )
        };
        self.spawn_at_path(path, actor)
    }
    pub async fn actor_of_path(&self, path: &ActorPath) -> Result<ActorRef> {
        self.inner.actor_of_path(path).await
    }
    pub async fn actor_of_str(&self, path_str: &str) -> Result<ActorRef> {
        self.inner.actor_of_str(path_str).await
    }
    pub fn spawn_local<A: Actor + 'static>(&self, actor_id: String, actor: A) -> ActorRef {
        self.inner.spawn_local(actor_id, actor)
    }
    pub async fn actor_of(&self, actor_id: ActorId) -> Result<ActorRef> {
        self.inner.actor_of(actor_id).await
    }

    // Connects to a remote node by its ID and address.
    pub async fn connect_to_node(&self, node_id: String, address: String) -> Result<()> {
        let _ = self
            .inner
            .node_registry
            .register_node(node_id.clone(), address.clone())
            .await;
        self.inner
            .stream_manager
            .connect_to_node(node_id.clone(), address.clone())
            .await
    }

    // Disconnects from a remote node by its ID.
    pub fn disconnect_from_node(&self, node_id: &str) {
        self.inner.node_registry.remove_node(node_id);
    }

    pub fn stop_local_actor(&self, actor_id: &ActorId) -> Result<()> {
        if let Some((_, actor_ref)) = self.inner.local_actors.remove(actor_id) {
            let path_to_remove = actor_ref.actor_path().full_path();
            self.inner.path_to_runtime.remove(&path_to_remove);

            actor_ref.stop();
            let _ = self
                .inner
                .job_sender
                .send(SchedulerJobRequest::UnregisterActor {
                    actor_id: actor_id.clone(),
                });
            Ok(())
        } else {
            // Try to find the actor by its path
            let full_path = actor_id.path.full_path();
            let path = if full_path.starts_with("lyricore://") {
                ActorPath::parse_with_default(full_path.as_str(), &self.inner.system_address)?
            } else if full_path.starts_with('/') {
                ActorPath::new(
                    self.inner.system_name.clone(),
                    self.inner.system_address.clone(),
                    actor_id.to_string(),
                )
            } else {
                ActorPath::new(
                    self.inner.system_name.clone(),
                    self.inner.system_address.clone(),
                    format!("/user/{}", actor_id),
                )
            };
            self.inner.stop_actor_at_path(&path)
        }
    }
    pub fn stop_actor_at_path(&self, path: &ActorPath) -> Result<()> {
        self.inner.stop_actor_at_path(path)
    }
    // Stops all local actors in the system.
    pub async fn stop_all_actors(&self) {
        let actor_ids: Vec<ActorId> = self
            .inner
            .local_actors
            .iter()
            .map(|entry| entry.key().clone())
            .collect();

        for actor_id in actor_ids {
            self.stop_local_actor(&actor_id).ok();
        }

        // Wait for all actors to finish processing their messages
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }
    /// Shuts down the actor system gracefully.
    pub async fn shutdown(&mut self) -> Result<()> {
        // Stop all local actors
        self.stop_all_actors().await;

        // Stop the scheduler
        let _ = self
            .inner
            .scheduler_cmd_sender
            .send(SchedulerCommand::Shutdown);

        // Close the gRPC server
        if let Some(handle) = self.server_handle.take() {
            handle.abort();
        }
        // Close the shutdown channel if it exists
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }

        Ok(())
    }
}

// Example usage of the ActorSystem
#[cfg(test)]
mod tests {
    use super::*;
    use crate::log::init_tracing_subscriber;
    use crate::scheduler::SchedulerConfig;
    use crate::ActorContext;
    use serde::{Deserialize, Serialize};

    #[derive(Clone, Debug, Serialize, Deserialize)]
    struct TestMessage {
        content: String,
    }

    impl Message for TestMessage {
        type Response = String;
    }

    struct TestActor {
        count: usize,
    }

    #[async_trait::async_trait]
    impl Actor for TestActor {
        type Message = TestMessage;
        type State = ();

        async fn on_start(&mut self, ctx: &mut ActorContext) -> Result<()> {
            println!("TestActor started: {}", ctx.actor_id);
            Ok(())
        }

        async fn on_message(&mut self, msg: Self::Message, ctx: &mut ActorContext) -> Result<()> {
            self.count += 1;
            println!("Received: {} (count: {})", msg.content, self.count);
            Ok(())
        }

        async fn handle_message(
            &mut self,
            msg: Self::Message,
            ctx: &mut ActorContext,
        ) -> Result<String> {
            self.on_message(msg.clone(), ctx).await?;
            Ok(format!("Response to: {}", msg.content))
        }

        async fn on_stop(&mut self, ctx: &mut ActorContext) -> Result<()> {
            println!(
                "TestActor stopped: {} (processed {} messages)",
                ctx.actor_id, self.count
            );
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_local_actor() {
        let strategy = SerializationStrategy::fast_json();

        let mut system = ActorSystem::new(
            "test_node".to_string(),
            "127.0.0.1:50051".to_string(),
            SchedulerConfig::default(),
            Some(strategy),
        )
        .unwrap();

        system.start_server().await.unwrap();
        let actor_ref = system.spawn_local("test_actor".to_string(), TestActor { count: 0 });
        let res = actor_ref
            .ask(TestMessage {
                content: "Hello, Actor!".to_string(),
            })
            .await
            .unwrap();
        assert_eq!(res, "Response to: Hello, Actor!");

        system
            .shutdown()
            .await
            .expect("Unable to shutdown actor system");
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
    }

    #[tokio::test]
    async fn test_local_remote_actor() {
        init_tracing_subscriber("lyricore_test", "DEBUG");
        // Create two actor systems representing different nodes
        let strategy1 = SerializationStrategy::messagepack();
        let mut system1 = ActorSystem::new(
            "node1".to_string(),
            "127.0.0.1:50051".to_string(),
            SchedulerConfig::default(),
            Some(strategy1),
        )
        .unwrap();

        let strategy2 = SerializationStrategy::fast_json();
        let mut system2 = ActorSystem::new(
            "node2".to_string(),
            "127.0.0.1:50052".to_string(),
            SchedulerConfig::default(),
            Some(strategy2),
        )
        .unwrap();

        system1.start_server().await.unwrap();
        system2.start_server().await.unwrap();

        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

        system1
            .connect_to_node("node2".to_string(), "127.0.0.1:50052".to_string())
            .await
            .unwrap();
        system2
            .connect_to_node("node1".to_string(), "127.0.0.1:50051".to_string())
            .await
            .unwrap();

        let _local_actor = system1.spawn_local("test_actor".to_string(), TestActor { count: 0 });

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let remote_path = ActorPath::new(
            "node1".to_string(),
            ActorAddress::from_str("127.0.0.1:50051").unwrap(),
            "/user/test_actor".to_string(),
        );
        let remote_actor = system2.actor_of_path(&remote_path).await.unwrap();

        let res = remote_actor
            .ask(TestMessage {
                content: "Hello from remote!".to_string(),
            })
            .await
            .unwrap();

        assert_eq!(res, "Response to: Hello from remote!");
        println!("Test passed! Response: {}", res);

        system1
            .shutdown()
            .await
            .expect("Unable to shutdown actor system 1");
        system2
            .shutdown()
            .await
            .expect("Unable to shutdown actor system 2");
    }

    #[tokio::test]
    async fn test_actor_path_creation_and_lookup() {
        let strategy = SerializationStrategy::fast_json();

        let mut system = ActorSystem::new_with_name(
            "test_system".to_string(),
            "127.0.0.1:50053".to_string(),
            SchedulerConfig::default(),
            Some(strategy),
        )
        .unwrap();

        system.start_server().await.unwrap();

        let actor_ref = system
            .spawn_at("/user/path_test_actor", TestActor { count: 0 })
            .unwrap();

        assert_eq!(actor_ref.actor_path().path, "/user/path_test_actor");
        assert_eq!(actor_ref.actor_path().system, "test_system");
        assert_eq!(actor_ref.actor_path().name(), "path_test_actor");

        let actor_ref2 = system.actor_of_str("/user/path_test_actor").await.unwrap();

        let res1 = actor_ref
            .ask(TestMessage {
                content: "First message".to_string(),
            })
            .await
            .unwrap();

        let res2 = actor_ref2
            .ask(TestMessage {
                content: "Second message".to_string(),
            })
            .await
            .unwrap();

        assert_eq!(res1, "Response to: First message");
        assert_eq!(res2, "Response to: Second message");

        system
            .shutdown()
            .await
            .expect("Unable to shutdown actor system");
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }

    #[tokio::test]
    async fn test_hierarchical_actor_paths() {
        let strategy = SerializationStrategy::fast_json();

        let mut system = ActorSystem::new_with_name(
            "hierarchy_test".to_string(),
            "127.0.0.1:50054".to_string(),
            SchedulerConfig::default(),
            Some(strategy),
        )
        .unwrap();

        system.start_server().await.unwrap();

        // Create parent Actor
        let _parent_ref = system
            .spawn_at("/user/parent", TestActor { count: 0 })
            .unwrap();

        // Create child and grandchild Actors
        let _child_ref = system
            .spawn_at("/user/parent/child", TestActor { count: 0 })
            .unwrap();
        let _grandchild_ref = system
            .spawn_at("/user/parent/child/grandchild", TestActor { count: 0 })
            .unwrap();

        // Verify hierarchical paths
        let parent_path = ActorPath::new(
            "hierarchy_test".to_string(),
            ActorAddress::from_str("127.0.0.1:50054").unwrap(),
            "/user/parent".to_string(),
        );

        let child_path = parent_path.child("child");
        assert_eq!(child_path.path, "/user/parent/child");

        let grandchild_path = child_path.child("grandchild");
        assert_eq!(grandchild_path.path, "/user/parent/child/grandchild");

        // Verify parent-child relationship
        let parent_of_child = child_path.parent().unwrap();
        assert_eq!(parent_of_child.path, "/user/parent");

        // Send message to parent Actor
        let grandchild_actor = system
            .actor_of_str("/user/parent/child/grandchild")
            .await
            .unwrap();
        let res = grandchild_actor
            .ask(TestMessage {
                content: "Hello grandchild!".to_string(),
            })
            .await
            .unwrap();
        assert_eq!(res, "Response to: Hello grandchild!");

        system
            .shutdown()
            .await
            .expect("Unable to shutdown actor system");
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }

    #[tokio::test]
    async fn test_path_parsing_and_validation() {
        // parse and validate ActorPath
        let path_str = "lyricore://test_system@localhost:8888/user/my_actor";
        let path = ActorPath::parse(path_str).unwrap();

        assert_eq!(path.protocol, "lyricore");
        assert_eq!(path.system, "test_system");
        assert_eq!(path.address.host, "localhost");
        assert_eq!(path.address.port, 8888);
        assert_eq!(path.path, "/user/my_actor");
        assert_eq!(path.name(), "my_actor");

        // Test full path generation
        assert_eq!(path.full_path(), path_str);

        // Test local path generation
        assert_eq!(path.local_path(), "lyricore://test_system/user/my_actor");

        // Test child path creation
        let child = path.child("child_actor");
        assert_eq!(child.path, "/user/my_actor/child_actor");
        assert_eq!(child.name(), "child_actor");

        // Test parent path retrieval
        let parent = child.parent().unwrap();
        assert_eq!(parent.path, "/user/my_actor");

        // Test root path parent
        let root_path = ActorPath::new(
            "test".to_string(),
            ActorAddress::local(8080),
            "/".to_string(),
        );
        assert!(root_path.parent().is_none());
    }
}
