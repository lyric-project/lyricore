use crate::rpc::actor_service::{AskRequest, MessageEnvelope, StreamMessage, TellRequest};
use dashmap::DashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

#[derive(Clone, Debug)]
pub struct StreamConnectionManager {
    connections: Arc<DashMap<String, StreamConnection>>,
    config: StreamConfig,
}

#[derive(Clone, Debug)]
pub struct StreamConfig {
    pub buffer_size: usize,
    pub keep_alive_interval: std::time::Duration,
    pub max_batch_size: usize,
    pub batch_timeout: std::time::Duration,
    ask_timeout: std::time::Duration,
}

impl Default for StreamConfig {
    fn default() -> Self {
        Self {
            buffer_size: 10000,
            keep_alive_interval: std::time::Duration::from_secs(30),
            max_batch_size: 1000,
            batch_timeout: std::time::Duration::from_millis(1),
            ask_timeout: std::time::Duration::from_secs(30),
        }
    }
}

#[derive(Debug)]
struct StreamConnection {
    node_id: String,
    sender: mpsc::UnboundedSender<StreamMessage>,
    pending_asks: Arc<
        DashMap<
            String,
            oneshot::Sender<crate::error::Result<crate::serialization::MessageEnvelope>>,
        >,
    >,
    stats: Arc<StreamStats>,
}

#[derive(Default, Debug)]
struct StreamStats {
    messages_sent: std::sync::atomic::AtomicU64,
    messages_received: std::sync::atomic::AtomicU64,
    batch_count: std::sync::atomic::AtomicU64,
    avg_batch_size: std::sync::atomic::AtomicU64,
}

impl StreamConnectionManager {
    pub fn new(config: StreamConfig) -> Self {
        Self {
            connections: Arc::new(DashMap::new()),
            config,
        }
    }

    // Connects to a remote node using its ID and address.
    pub async fn connect_to_node(
        &self,
        node_id: String,
        address: String,
    ) -> crate::error::Result<()> {
        let mut client =
            crate::rpc::actor_service::actor_service_client::ActorServiceClient::connect(format!(
                "http://{}",
                address
            ))
            .await?;

        let (tx, rx) = mpsc::unbounded_channel();
        let pending_asks = Arc::new(DashMap::new());
        let stats = Arc::new(StreamStats::default());

        // Start a bidirectional stream with the remote node
        let stream = client
            .bidirectional_stream(tokio_stream::wrappers::UnboundedReceiverStream::new(rx))
            .await?;
        let mut response_stream = stream.into_inner();

        // Handle incoming messages from the stream
        let pending_asks_clone = Arc::clone(&pending_asks);
        let stats_clone = Arc::clone(&stats);
        tokio::spawn(async move {
            while let Ok(Some(response)) = response_stream.message().await {
                Self::handle_stream_response(response, &pending_asks_clone, &stats_clone).await;
            }
        });

        // Start the batch sender to handle outgoing messages
        let batch_sender = self.create_batch_sender(tx.clone()).await;

        let connection = StreamConnection {
            node_id: node_id.clone(),
            sender: batch_sender,
            pending_asks,
            stats,
        };
        let node_key = format!("{}@{}", node_id, address);
        self.connections.insert(node_key, connection);
        Ok(())
    }

    // Handles batching of messages to optimize network usage
    async fn create_batch_sender(
        &self,
        stream_tx: mpsc::UnboundedSender<StreamMessage>,
    ) -> mpsc::UnboundedSender<StreamMessage> {
        let (batch_tx, mut batch_rx) = mpsc::unbounded_channel::<StreamMessage>();
        let config = self.config.clone();

        tokio::spawn(async move {
            let mut batch = Vec::with_capacity(config.max_batch_size);
            let mut timer = tokio::time::interval(config.batch_timeout);
            timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

            loop {
                tokio::select! {
                    // Connect to the batch receiver
                    msg = batch_rx.recv() => {
                        match msg {
                            Some(message) => {
                                batch.push(message);

                                // If the batch is full, send it immediately
                                if batch.len() >= config.max_batch_size {
                                    Self::send_batch(&stream_tx, &mut batch).await;
                                }
                            }
                            None => break,
                        }
                    }

                    // Schedule a tick to send the batch
                    _ = timer.tick() => {
                        if !batch.is_empty() {
                            Self::send_batch(&stream_tx, &mut batch).await;
                        }
                    }
                }
            }
        });

        batch_tx
    }

    async fn send_batch(
        stream_tx: &mpsc::UnboundedSender<StreamMessage>,
        batch: &mut Vec<StreamMessage>,
    ) {
        for message in batch.drain(..) {
            if stream_tx.send(message).is_err() {
                tracing::error!("Failed to send message to stream");
                break;
            }
        }
    }

    // Sends a TellRequest message to the specified node.
    pub async fn stream_tell(
        &self,
        node_id: &str,
        request: TellRequest,
    ) -> crate::error::Result<()> {
        let connection = self.connections.get(node_id).ok_or_else(|| {
            crate::error::LyricoreActorError::Actor(crate::error::ActorError::RpcError(
                "Node not connected".to_string(),
            ))
        })?;

        let stream_message = StreamMessage {
            message_type: Some(
                crate::rpc::actor_service::stream_message::MessageType::TellRequest(request),
            ),
            stream_id: uuid::Uuid::new_v4().to_string(),
        };

        connection.sender.send(stream_message).map_err(|_| {
            crate::error::LyricoreActorError::Actor(crate::error::ActorError::RpcError(
                "Failed to send to stream".to_string(),
            ))
        })?;

        connection
            .stats
            .messages_sent
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }

    // Sends an AskRequest message to the specified node and waits for a response.
    pub async fn stream_ask(
        &self,
        node_id: &str,
        request: AskRequest,
    ) -> crate::error::Result<crate::serialization::MessageEnvelope> {
        let connection = self.connections.get(node_id).ok_or_else(|| {
            crate::error::LyricoreActorError::Actor(crate::error::ActorError::RpcError(
                "Node not connected".to_string(),
            ))
        })?;

        let correlation_id = uuid::Uuid::new_v4().to_string();
        let (response_tx, response_rx) = oneshot::channel();

        // Register the response channel in the connection's pending asks
        connection
            .pending_asks
            .insert(correlation_id.clone(), response_tx);

        let mut ask_request = request;
        ask_request.correlation_id = correlation_id.clone();

        let stream_message = StreamMessage {
            message_type: Some(
                crate::rpc::actor_service::stream_message::MessageType::AskRequest(ask_request),
            ),
            stream_id: correlation_id.clone(),
        };

        connection.sender.send(stream_message).map_err(|_| {
            connection.pending_asks.remove(&correlation_id);
            crate::error::LyricoreActorError::Actor(crate::error::ActorError::RpcError(
                "Failed to send to stream".to_string(),
            ))
        })?;

        connection
            .stats
            .messages_sent
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        // Wait for the response with a timeout
        match tokio::time::timeout(self.config.ask_timeout, response_rx).await {
            Ok(Ok(response)) => match response {
                Ok(envelope) => Ok(envelope),
                Err(e) => Err(e),
            },
            Ok(Err(_)) => {
                connection.pending_asks.remove(&correlation_id);
                Err(crate::error::LyricoreActorError::Actor(
                    crate::error::ActorError::RpcError("Response channel closed".to_string()),
                ))
            }
            Err(_) => {
                connection.pending_asks.remove(&correlation_id);
                Err(crate::error::LyricoreActorError::Actor(
                    crate::error::ActorError::RpcError("Request timeout".to_string()),
                ))
            }
        }
    }

    async fn handle_stream_response(
        response: StreamMessage,
        pending_asks: &DashMap<
            String,
            oneshot::Sender<crate::error::Result<crate::serialization::MessageEnvelope>>,
        >,
        stats: &StreamStats,
    ) {
        use crate::rpc::actor_service::stream_message::MessageType;

        match response.message_type {
            Some(MessageType::AskResponse(ask_response)) => {
                if let Some((_, sender)) = pending_asks.remove(&ask_response.correlation_id) {
                    let result = if ask_response.success {
                        if let Some(envelope) = ask_response.response_envelope {
                            match envelope.try_into() {
                                Ok(env) => Ok(env),
                                Err(e) => Err(e),
                            }
                        } else {
                            Err(crate::error::LyricoreActorError::Actor(
                                crate::error::ActorError::RpcError(
                                    "Missing response envelope".to_string(),
                                ),
                            ))
                        }
                    } else {
                        Err(crate::error::LyricoreActorError::Actor(
                            crate::error::ActorError::RpcError(
                                ask_response
                                    .error
                                    .unwrap_or_else(|| "Unknown error".to_string()),
                            ),
                        ))
                    };

                    let _ = sender.send(result);
                }
                stats
                    .messages_received
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            Some(MessageType::TellResponse(_)) => {
                stats
                    .messages_received
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            _ => {
                tracing::warn!("Received unknown message type in stream response");
            }
        }
    }

    // Gets statistics for a specific node connection.
    pub fn get_stats(&self, node_id: &str) -> Option<(u64, u64)> {
        self.connections.get(node_id).map(|conn| {
            let sent = conn
                .stats
                .messages_sent
                .load(std::sync::atomic::Ordering::Relaxed);
            let received = conn
                .stats
                .messages_received
                .load(std::sync::atomic::Ordering::Relaxed);
            (sent, received)
        })
    }
}
