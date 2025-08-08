use crate::actor::{Actor, ActorContext};
use crate::actor_system::NodeRegistry;
use crate::error::{LyricoreActorError, Result};
use crate::message::InboxMessage;
use crate::path::{ActorAddress, ActorId, ActorPath};
use crate::rpc::actor_service;
use crate::rpc::actor_service::{AskRequest, TellRequest};
use crate::scheduler::{SchedulerJobRequest, WorkItem};
use crate::serialization::{MessageEnvelope, MessageRegistry, SerFormat};
use crate::stream::StreamConnectionManager;
use crate::Message;
use serde::{Deserialize, Serialize};
use std::cell::UnsafeCell;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};
use tonic::Request;

/// No-lock LocalActorRef, based on UnsafeCell
/// Safety guarantees: The scheduler ensures that messages for the same actor are routed to the same shard
/// and processed in a single-threaded manner, so there won't be concurrent access to the same actor.
#[derive(Debug)]
struct LocalActorRefInner {
    actor_id: ActorId,
    // Use unsafe_cell to allow interior mutability
    actor: Arc<UnsafeCell<Box<dyn ActorProcessor>>>,
    job_sender: mpsc::UnboundedSender<SchedulerJobRequest>,
    message_registry: Arc<MessageRegistry>,
}

// Make sure UnsafeCell is Send and Sync(But not concurrently)
unsafe impl Send for LocalActorRefInner {}
unsafe impl Sync for LocalActorRefInner {}

#[derive(Clone, Debug)]
pub struct LocalActorRef {
    inner: Arc<LocalActorRefInner>,
}

#[derive(Clone, Debug)]
struct RemoteActorRefInner {
    actor_id: ActorId,
    node_registry: Arc<NodeRegistry>,
    message_registry: Arc<MessageRegistry>,
}

/// Remote actor reference, used for remote communication
#[derive(Clone, Debug)]
pub struct RemoteActorRef {
    inner: Arc<RemoteActorRefInner>,
}

/// The `ActorRef` enum represents a reference to an actor, which can be either local or remote.
#[derive(Clone, Debug)]
pub enum ActorRef {
    Local(LocalActorRef),
    Remote(StreamingRemoteActorRef),
}

impl ActorRef {
    pub async fn tell<M: Message + Serialize + 'static>(&self, message: M) -> Result<()> {
        match self {
            ActorRef::Local(local_ref) => local_ref.tell(message).await,
            ActorRef::Remote(remote_ref) => remote_ref.tell(message).await,
        }
    }

    pub async fn ask<M: Message + Serialize + for<'de> Deserialize<'de> + 'static>(
        &self,
        message: M,
    ) -> Result<M::Response>
    where
        M::Response: for<'de> Deserialize<'de> + Serialize,
    {
        match self {
            ActorRef::Local(local_ref) => local_ref.ask(message).await,
            ActorRef::Remote(remote_ref) => remote_ref.ask(message).await,
        }
    }

    pub fn actor_id(&self) -> &ActorId {
        match self {
            ActorRef::Local(local_ref) => local_ref.actor_id(),
            ActorRef::Remote(remote_ref) => remote_ref.actor_id(),
        }
    }

    pub fn actor_path(&self) -> &ActorPath {
        match self {
            ActorRef::Local(local_ref) => local_ref.actor_path(),
            ActorRef::Remote(remote_ref) => remote_ref.actor_path(),
        }
    }

    pub fn address(&self) -> &ActorAddress {
        match self {
            ActorRef::Local(local_ref) => &local_ref.actor_id().path.address,
            ActorRef::Remote(remote_ref) => &remote_ref.actor_id().path.address,
        }
    }

    pub fn stop(&self) {
        match self {
            ActorRef::Local(local_ref) => local_ref.stop(),
            ActorRef::Remote(remote_ref) => {
                // For remote actors, we can send a special stop message
                // or notify the remote node to stop the actor via gRPC call.
                if let Ok(rt) = tokio::runtime::Handle::try_current() {
                    let remote_ref = remote_ref.clone();
                    rt.spawn(async move {
                        // TODO: Implement a proper stop message for remote actors
                        // remote_ref.tell(SystemMessage::Stop).await.ok();
                    });
                }
            }
        }
    }
}

impl LocalActorRefInner {
    async fn process_message(
        &self,
        message: Box<dyn std::any::Any + Send>,
        ctx: &mut ActorContext,
    ) -> Result<()> {
        // SAFETY:
        // 1. The scheduler ensures that messages for the same actor are routed to the same shard by consistent hashing.
        // 2. Each shard processes messages in a single-threaded manner, so there won't be concurrent access to the same actor.
        // 3. So we can safely access the actor through UnsafeCell.
        unsafe {
            let actor_ptr = self.actor.get();
            let actor_ref = &mut *actor_ptr;
            actor_ref.process_any_message(message, ctx).await
        }
    }
}
impl LocalActorRef {
    pub fn new<A: Actor + 'static>(
        actor_id: ActorId,
        actor: A,
        job_sender: mpsc::UnboundedSender<SchedulerJobRequest>,
        message_registry: Arc<MessageRegistry>,
    ) -> Self {
        let inner = LocalActorRefInner {
            actor_id,
            actor: Arc::new(UnsafeCell::new(Box::new(ActorWrapper::new(actor)))),
            job_sender,
            message_registry,
        };
        Self {
            inner: Arc::new(inner),
        }
    }

    pub fn actor_id(&self) -> &ActorId {
        &self.inner.actor_id
    }

    pub fn actor_path(&self) -> &ActorPath {
        &self.inner.actor_id.path
    }

    pub fn runtime_id(&self) -> &str {
        &self.inner.actor_id.runtime_id
    }

    #[inline]
    pub async fn tell<M: Message + Serialize + 'static>(&self, message: M) -> Result<()> {
        let envelope = self
            .inner
            .message_registry
            .create_local_envelope(&message)?;
        let work_item = WorkItem::new(
            self.inner.actor_id.clone(),
            Box::new(InboxMessage::envelope_message(
                self.inner.actor_id.path.address.clone(),
                envelope,
            )),
        );
        let _ = self.inner.job_sender.send(SchedulerJobRequest::SubmitWork {
            work_item,
            remote: false,
        });
        Ok(())
    }

    #[inline]
    pub async fn ask<M: Message + Serialize + for<'de> Deserialize<'de> + 'static>(
        &self,
        message: M,
    ) -> Result<M::Response>
    where
        M::Response: for<'de> Deserialize<'de> + Serialize,
    {
        let (tx, rx) = oneshot::channel();
        let envelope = self
            .inner
            .message_registry
            .create_local_envelope(&message)?;
        let work_item = WorkItem::new(
            self.inner.actor_id.clone(),
            Box::new(InboxMessage::rpc_envelope_message(
                self.inner.actor_id.path.address.clone(),
                envelope,
                tx,
            )),
        );
        let _ = self.inner.job_sender.send(SchedulerJobRequest::SubmitWork {
            work_item,
            remote: false,
        });
        let response_envelope = rx.await.map_err(|_| {
            LyricoreActorError::Actor(crate::error::ActorError::RpcError(
                "Response channel closed".to_string(),
            ))
        })??;
        let response: M::Response = response_envelope.deserialize(&self.inner.message_registry)?;
        Ok(response)
    }

    /// Key method: lock-free process_message
    /// Safety: The scheduler ensures that messages for the same actor are executed on the same
    /// shard, so there won't be concurrent access
    pub(crate) async fn process_message(
        &self,
        message: Box<dyn std::any::Any + Send>,
        ctx: &mut ActorContext,
    ) -> Result<()> {
        self.inner.process_message(message, ctx).await
    }

    pub fn stop(&self) {
        let work_item = WorkItem::new(self.inner.actor_id.clone(), Box::new(InboxMessage::OnStop));
        let _ = self.inner.job_sender.send(SchedulerJobRequest::SubmitWork {
            work_item,
            remote: false,
        });
    }
}

#[async_trait::async_trait]
trait ActorProcessor: Send + Sync {
    async fn process_any_message(
        &mut self,
        message: Box<dyn std::any::Any + Send>,
        ctx: &mut ActorContext,
    ) -> Result<()>;
}

struct ActorWrapper<A: Actor> {
    actor: A,
}

impl<A: Actor> ActorWrapper<A> {
    fn new(actor: A) -> Self {
        Self { actor }
    }
}

#[async_trait::async_trait]
impl<A: Actor> ActorProcessor for ActorWrapper<A> {
    async fn process_any_message(
        &mut self,
        message: Box<dyn std::any::Any + Send>,
        ctx: &mut ActorContext,
    ) -> Result<()> {
        if let Ok(inbox_msg) = message.downcast::<InboxMessage>() {
            match *inbox_msg {
                InboxMessage::EnvelopeMessage { addr, envelope } => {
                    // ctx.sender = Some(addr);
                    if envelope.check_message_type::<A::Message>() {
                        let content: A::Message = match envelope.format {
                            SerFormat::Json => {
                                serde_json::from_slice(&envelope.payload).map_err(|e| {
                                    LyricoreActorError::Actor(crate::error::ActorError::RpcError(
                                        e.to_string(),
                                    ))
                                })?
                            }
                            SerFormat::Messagepack => rmp_serde::from_slice(&envelope.payload)
                                .map_err(|e| {
                                    LyricoreActorError::Actor(crate::error::ActorError::RpcError(
                                        e.to_string(),
                                    ))
                                })?,
                            _ => {
                                return Err(LyricoreActorError::Actor(
                                    crate::error::ActorError::RpcError(
                                        "Unsupported format".to_string(),
                                    ),
                                ));
                            }
                        };
                        self.actor.on_message(content, ctx).await
                    } else {
                        Err(LyricoreActorError::Actor(
                            crate::error::ActorError::RpcError(format!(
                                "Message type mismatch: expected {}, got {}",
                                MessageEnvelope::generate_message_type_id::<A::Message>(),
                                envelope.message_type
                            )),
                        ))
                    }
                }
                InboxMessage::RpcEnvelopeMessage {
                    envelope,
                    response_tx,
                    ..
                } => {
                    if envelope.check_message_type::<A::Message>() {
                        let content: A::Message = match envelope.format {
                            SerFormat::Json => {
                                serde_json::from_slice(&envelope.payload).map_err(|e| {
                                    LyricoreActorError::Actor(crate::error::ActorError::RpcError(
                                        e.to_string(),
                                    ))
                                })?
                            }
                            SerFormat::Messagepack => rmp_serde::from_slice(&envelope.payload)
                                .map_err(|e| {
                                    LyricoreActorError::Actor(crate::error::ActorError::RpcError(
                                        e.to_string(),
                                    ))
                                })?,
                            _ => {
                                let _ = response_tx.send(Err(LyricoreActorError::Actor(
                                    crate::error::ActorError::RpcError(
                                        "Unsupported format".to_string(),
                                    ),
                                )));
                                return Ok(());
                            }
                        };

                        let result = self.actor.handle_message(content, ctx).await;
                        match result {
                            Ok(response) => {
                                let response_payload = match envelope.format {
                                    SerFormat::Json => {
                                        serde_json::to_vec(&response).map_err(|e| {
                                            LyricoreActorError::Actor(
                                                crate::error::ActorError::RpcError(e.to_string()),
                                            )
                                        })?
                                    }
                                    SerFormat::Messagepack => rmp_serde::to_vec(&response)
                                        .map_err(|e| {
                                            LyricoreActorError::Actor(
                                                crate::error::ActorError::RpcError(e.to_string()),
                                            )
                                        })?,
                                    _ => {
                                        let _ = response_tx.send(Err(LyricoreActorError::Actor(
                                            crate::error::ActorError::RpcError(
                                                "Unsupported format".to_string(),
                                            ),
                                        )));
                                        return Ok(());
                                    }
                                };

                                let response_envelope = MessageEnvelope {
                                    message_type: MessageEnvelope::generate_message_type_id::<<A::Message as Message>::Response>(),
                                    format: envelope.format,
                                    schema_version: <<A::Message as Message>::Response as Message>::SCHEMA_VERSION,
                                    payload: response_payload,
                                    metadata: None,
                                    checksum: None,
                                };

                                let _ = response_tx.send(Ok(response_envelope));
                            }
                            Err(e) => {
                                let _ = response_tx.send(Err(e));
                            }
                        }
                        Ok(())
                    } else {
                        let _ = response_tx.send(Err(LyricoreActorError::Actor(
                            crate::error::ActorError::RpcError(format!(
                                "Message type mismatch: expected {}, got {}",
                                MessageEnvelope::generate_message_type_id::<A::Message>(),
                                envelope.message_type
                            )),
                        )));
                        Ok(())
                    }
                }
                InboxMessage::OnStart => self.actor.on_start(ctx).await,
                InboxMessage::OnStop => self.actor.on_stop(ctx).await,
                _ => Ok(()),
            }
        } else {
            Err(LyricoreActorError::Actor(
                crate::error::ActorError::RpcError("Invalid message type".to_string()),
            ))
        }
    }
}

impl RemoteActorRef {
    pub fn new(
        actor_id: ActorId,
        node_registry: Arc<NodeRegistry>,
        message_registry: Arc<MessageRegistry>,
    ) -> Self {
        let inner = RemoteActorRefInner {
            actor_id,
            node_registry,
            message_registry,
        };
        Self {
            inner: Arc::new(inner),
        }
    }
    pub fn actor_id(&self) -> &ActorId {
        &self.inner.actor_id
    }

    pub fn actor_path(&self) -> &ActorPath {
        &self.inner.actor_id.path
    }

    pub async fn tell<M: Message + Serialize + 'static>(&self, message: M) -> Result<()> {
        let node_info = self
            .inner
            .node_registry
            .get_node(&self.inner.actor_id.path.system_address())
            .ok_or_else(|| {
                LyricoreActorError::Actor(crate::error::ActorError::RpcError(
                    "Node not found".to_string(),
                ))
            })?;

        let mut client = node_info.client;

        let envelope = self.inner.message_registry.create_envelope(
            &message,
            false,
            node_info.capabilities.as_ref(),
        )?;

        let request = Request::new(actor_service::TellRequest {
            actor_id: self.inner.actor_id.path.full_path(), // Use full path
            envelope: Some(envelope.into()),
            trace_id: None,
        });

        client.tell(request).await.map_err(|e| {
            LyricoreActorError::Actor(crate::error::ActorError::RpcError(e.to_string()))
        })?;

        Ok(())
    }

    pub async fn ask<M: Message + Serialize + for<'de> Deserialize<'de> + 'static>(
        &self,
        message: M,
    ) -> Result<M::Response>
    where
        M::Response: for<'de> Deserialize<'de> + Serialize,
    {
        let node_info = self
            .inner
            .node_registry
            .get_node(&self.inner.actor_id.path.system_address())
            .ok_or_else(|| {
                LyricoreActorError::Actor(crate::error::ActorError::RpcError(
                    "Node not found".to_string(),
                ))
            })?;

        let mut client = node_info.client;
        let correlation_id = uuid::Uuid::new_v4().to_string();

        let envelope = self.inner.message_registry.create_envelope(
            &message,
            false,
            node_info.capabilities.as_ref(),
        )?;

        let full_path = self.inner.actor_id.path.full_path();
        tracing::debug!("Asking actor at path: {}", full_path);

        let request = Request::new(actor_service::AskRequest {
            actor_id: full_path,
            envelope: Some(envelope.into()),
            correlation_id: correlation_id.clone(),
            timeout_ms: Some(30000),
            trace_id: None,
        });

        let response = client.ask(request).await.map_err(|e| {
            LyricoreActorError::Actor(crate::error::ActorError::RpcError(e.to_string()))
        })?;

        let ask_response = response.into_inner();
        if ask_response.success {
            if let Some(response_envelope) = ask_response.response_envelope {
                let envelope: MessageEnvelope = response_envelope.try_into()?;
                let response: M::Response = envelope.deserialize(&self.inner.message_registry)?;
                Ok(response)
            } else {
                Err(LyricoreActorError::Actor(
                    crate::error::ActorError::RpcError("No response envelope".to_string()),
                ))
            }
        } else {
            Err(LyricoreActorError::Actor(
                crate::error::ActorError::RpcError(
                    ask_response
                        .error
                        .unwrap_or_else(|| "Unknown error".to_string()),
                ),
            ))
        }
    }
}

#[derive(Clone, Debug)]
pub struct StreamingRemoteActorRef {
    actor_id: ActorId,
    stream_manager: Arc<StreamConnectionManager>,
    message_registry: Arc<MessageRegistry>,
    fallback_ref: Option<RemoteActorRef>, // Backup reference for non-streaming calls
}

impl StreamingRemoteActorRef {
    pub fn new(
        actor_id: ActorId,
        stream_manager: Arc<StreamConnectionManager>,
        message_registry: Arc<MessageRegistry>,
        fallback_ref: Option<RemoteActorRef>,
    ) -> Self {
        Self {
            actor_id,
            stream_manager,
            message_registry,
            fallback_ref,
        }
    }

    pub fn actor_path(&self) -> &ActorPath {
        &self.actor_id.path
    }

    pub async fn tell<M: Message + Serialize + 'static>(
        &self,
        message: M,
    ) -> crate::error::Result<()> {
        let node_id = self.actor_id.path.system_address();

        // Try to use streaming connection
        if let Ok(envelope) = self.message_registry.create_local_envelope(&message) {
            let request = TellRequest {
                actor_id: self.actor_id.path.full_path(),
                envelope: Some(envelope.into()),
                trace_id: None,
            };

            // First try to send via streaming
            // If it fails, fall back to regular gRPC
            match self.stream_manager.stream_tell(&node_id, request).await {
                Ok(()) => return Ok(()),
                Err(e) => {
                    tracing::warn!("Stream tell failed, falling back to regular gRPC: {}", e);

                    // Back to regular gRPC
                    if let Some(ref fallback) = self.fallback_ref {
                        return fallback.tell(message).await;
                    } else {
                        return Err(e);
                    }
                }
            }
        }

        Err(LyricoreActorError::Actor(
            crate::error::ActorError::RpcError("Failed to create envelope".to_string()),
        ))
    }

    pub async fn ask<M: Message + Serialize + for<'de> serde::Deserialize<'de> + 'static>(
        &self,
        message: M,
    ) -> Result<M::Response>
    where
        M::Response: for<'de> serde::Deserialize<'de> + Serialize,
    {
        let node_id = self.actor_id.path.system_address();

        if let Ok(envelope) = self.message_registry.create_local_envelope(&message) {
            let request = AskRequest {
                actor_id: self.actor_id.path.full_path(),
                envelope: Some(envelope.into()),
                correlation_id: String::new(), // It will be set by stream_manager
                timeout_ms: Some(30000),
                trace_id: None,
            };

            // Try to use streaming connection first
            match self.stream_manager.stream_ask(&node_id, request).await {
                Ok(response_envelope) => {
                    let response: M::Response =
                        response_envelope.deserialize(&self.message_registry)?;
                    return Ok(response);
                }
                Err(e) => {
                    tracing::warn!("Stream ask failed, falling back to regular gRPC: {}", e);

                    // Back to regular gRPC
                    if let Some(ref fallback) = self.fallback_ref {
                        return fallback.ask(message).await;
                    } else {
                        return Err(e);
                    }
                }
            }
        }

        Err(LyricoreActorError::Actor(
            crate::error::ActorError::RpcError("Failed to create envelope".to_string()),
        ))
    }

    pub fn actor_id(&self) -> &ActorId {
        &self.actor_id
    }

    pub fn get_stats(&self) -> Option<(u64, u64)> {
        let node_id = self.actor_id.path.system_address();
        self.stream_manager.get_stats(&node_id)
    }
}
