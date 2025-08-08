use crate::error::LyricoreActorError;
use crate::message::InboxMessage;
use crate::rpc::actor_service;
use crate::rpc::actor_service::actor_service_server::ActorService;
use crate::rpc::actor_service::{
    AskRequest, AskResponse, StreamMessage, TellRequest, TellResponse,
};
use crate::scheduler::{SchedulerJobRequest, WorkItem};
use crate::serialization::{MessageEnvelope, MessageRegistry, SerFormat};
use crate::{ActorAddress, ActorId, ActorPath};
use futures::stream::FuturesUnordered;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot, Semaphore};
use tokio_stream::wrappers::UnboundedReceiverStream;
use tokio_stream::StreamExt;
use tonic::{Request, Response, Status, Streaming};

#[derive(Debug)]
enum MessageType {
    Tell,
    Ask,
}

pub struct ActorServiceImpl {
    job_sender: mpsc::UnboundedSender<SchedulerJobRequest>,
    message_registry: Arc<MessageRegistry>,
    node_capabilities: actor_service::NodeCapabilities,
    concurrency_limit: usize,
    ask_timeout: Duration,
}

impl ActorServiceImpl {
    pub fn new(
        job_sender: mpsc::UnboundedSender<SchedulerJobRequest>,
        message_registry: Arc<MessageRegistry>,
        concurrency_limit: usize,
        ask_timeout: Duration,
    ) -> Self {
        // Create default node capabilities
        let node_capabilities = actor_service::NodeCapabilities {
            supported_formats: vec![SerFormat::Json as i32, SerFormat::Messagepack as i32],
            supports_streaming: false,
            supports_compression: false,
            compression_algorithms: vec![],
        };

        Self {
            job_sender,
            message_registry,
            node_capabilities,
            concurrency_limit,
            ask_timeout,
        }
    }

    async fn schedule_remote_envelope_rpc_message(
        &self,
        actor_id: ActorId,
        envelope: MessageEnvelope,
        addr: ActorAddress,
    ) -> crate::Result<MessageEnvelope> {
        let (response_tx, response_rx) = oneshot::channel();

        let message = Box::new(InboxMessage::rpc_envelope_message(
            addr,
            envelope,
            response_tx,
        ));

        let work_item = WorkItem::new(actor_id, message);
        let _ = self.job_sender.send(SchedulerJobRequest::SubmitWork {
            work_item,
            remote: true,
        });

        match tokio::time::timeout(self.ask_timeout, response_rx).await {
            Ok(result) => match result {
                Ok(response) => response,
                Err(_) => Err(LyricoreActorError::Actor(
                    crate::error::ActorError::RpcError("Response channel error".to_string()),
                )),
            },
            Err(_) => Err(LyricoreActorError::Actor(
                crate::error::ActorError::RpcError("Request timeout".to_string()),
            )),
        }
    }
    async fn process_tell_request(
        req: TellRequest,
        job_sender: &mpsc::UnboundedSender<SchedulerJobRequest>,
    ) -> crate::error::Result<()> {
        if let Some(proto_envelope) = req.envelope {
            let envelope: MessageEnvelope = proto_envelope.try_into()?;
            let actor_path = ActorPath::parse(&req.actor_id)?;
            let actor_id = ActorId::new("".to_string(), actor_path);

            // 使用默认地址，因为这是内部处理
            let addr = ActorAddress::new("127.0.0.1".to_string(), 0);
            let message = Box::new(InboxMessage::envelope_message(addr, envelope));
            let work_item = WorkItem::new(actor_id, message);

            job_sender
                .send(SchedulerJobRequest::SubmitWork {
                    work_item,
                    remote: true,
                })
                .map_err(|_| {
                    crate::error::LyricoreActorError::Actor(crate::error::ActorError::RpcError(
                        "Failed to submit work".to_string(),
                    ))
                })?;

            Ok(())
        } else {
            Err(crate::error::LyricoreActorError::Actor(
                crate::error::ActorError::RpcError("Missing message envelope".to_string()),
            ))
        }
    }
    async fn process_ask_request(
        req: AskRequest,
        job_sender: &mpsc::UnboundedSender<SchedulerJobRequest>,
        message_registry: &Arc<MessageRegistry>,
    ) -> crate::error::Result<MessageEnvelope> {
        if let Some(proto_envelope) = req.envelope {
            let envelope: MessageEnvelope = proto_envelope.try_into()?;
            let actor_path = ActorPath::parse(&req.actor_id)?;
            let actor_id = ActorId::new("".to_string(), actor_path);

            // 使用默认地址
            let addr = ActorAddress::new("127.0.0.1".to_string(), 0);

            // 创建响应通道
            let (response_tx, response_rx) = tokio::sync::oneshot::channel();
            let message = Box::new(InboxMessage::rpc_envelope_message(
                addr,
                envelope,
                response_tx,
            ));

            let work_item = WorkItem::new(actor_id, message);
            job_sender
                .send(SchedulerJobRequest::SubmitWork {
                    work_item,
                    remote: true,
                })
                .map_err(|_| {
                    LyricoreActorError::Actor(crate::error::ActorError::RpcError(
                        "Failed to submit work".to_string(),
                    ))
                })?;

            // 等待响应
            match tokio::time::timeout(tokio::time::Duration::from_secs(30), response_rx).await {
                Ok(result) => match result {
                    Ok(response) => response,
                    Err(_) => Err(LyricoreActorError::Actor(
                        crate::error::ActorError::RpcError("Response channel error".to_string()),
                    )),
                },
                Err(_) => Err(LyricoreActorError::Actor(
                    crate::error::ActorError::RpcError("Request timeout".to_string()),
                )),
            }
        } else {
            Err(LyricoreActorError::Actor(
                crate::error::ActorError::RpcError("Missing message envelope".to_string()),
            ))
        }
    }

    // 修改后的处理函数，支持并行处理
    async fn process_stream_message_parallel(
        msg: StreamMessage,
        job_sender: &mpsc::UnboundedSender<SchedulerJobRequest>,
        message_registry: &Arc<MessageRegistry>,
    ) -> (MessageType, Option<StreamMessage>) {
        use crate::rpc::actor_service::stream_message::MessageType as ProtoMessageType;

        match msg.message_type {
            Some(ProtoMessageType::TellRequest(tell_req)) => {
                // Tell消息：快速处理，不等待响应
                let result = Self::process_tell_request(tell_req.clone(), job_sender).await;

                let response = TellResponse {
                    success: result.is_ok(),
                    error: result.err().map(|e| e.to_string()),
                    trace_id: tell_req.trace_id,
                };

                let response_msg = StreamMessage {
                    message_type: Some(ProtoMessageType::TellResponse(response)),
                    stream_id: msg.stream_id,
                };

                (MessageType::Tell, Some(response_msg))
            }

            Some(ProtoMessageType::AskRequest(ask_req)) => {
                // Ask消息：异步处理，等待Actor响应
                let result =
                    Self::process_ask_request(ask_req.clone(), job_sender, message_registry).await;

                let response = match result {
                    Ok(response_envelope) => AskResponse {
                        correlation_id: ask_req.correlation_id.clone(),
                        success: true,
                        response_envelope: Some(response_envelope.into()),
                        error: None,
                        trace_id: ask_req.trace_id,
                    },
                    Err(e) => AskResponse {
                        correlation_id: ask_req.correlation_id.clone(),
                        success: false,
                        response_envelope: None,
                        error: Some(e.to_string()),
                        trace_id: ask_req.trace_id,
                    },
                };

                let response_msg = StreamMessage {
                    message_type: Some(ProtoMessageType::AskResponse(response)),
                    stream_id: msg.stream_id,
                };

                (MessageType::Ask, Some(response_msg))
            }

            Some(ProtoMessageType::KeepAlive(_)) => {
                // Heartbeat message
                let response_msg = StreamMessage {
                    message_type: Some(ProtoMessageType::KeepAlive(
                        crate::rpc::actor_service::KeepAlive {
                            timestamp: std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap()
                                .as_secs() as i64,
                        },
                    )),
                    stream_id: msg.stream_id,
                };

                (MessageType::Tell, Some(response_msg)) // Heartbeat is treated as a Tell
            }

            _ => {
                tracing::warn!("Received unknown stream message type");
                (MessageType::Tell, None)
            }
        }
    }
}

#[tonic::async_trait]
impl ActorService for ActorServiceImpl {
    type StreamTellStream =
        UnboundedReceiverStream<std::result::Result<TellResponse, tonic::Status>>;
    type StreamAskStream = UnboundedReceiverStream<std::result::Result<AskResponse, tonic::Status>>;
    type BidirectionalStreamStream =
        UnboundedReceiverStream<std::result::Result<StreamMessage, tonic::Status>>;

    async fn tell(
        &self,
        request: Request<TellRequest>,
    ) -> std::result::Result<Response<actor_service::TellResponse>, Status> {
        let addr = request
            .remote_addr()
            .ok_or_else(|| Status::internal("Missing remote address"))?;
        let req = request.into_inner();

        if let Some(proto_envelope) = req.envelope {
            let envelope: MessageEnvelope = proto_envelope
                .try_into()
                .map_err(|e| Status::invalid_argument(format!("Invalid envelope: {}", e)))?;
            let addr = ActorAddress::new(addr.ip().to_string(), addr.port());
            let actor_path = ActorPath::parse(req.actor_id.as_str())
                .map_err(|e| Status::invalid_argument(format!("Invalid actor path: {}", e)))?;
            let actor_id = ActorId::new("".to_string(), actor_path);
            let message = Box::new(InboxMessage::envelope_message(addr, envelope));
            let work_item = WorkItem::new(actor_id, message);
            let _ = self.job_sender.send(SchedulerJobRequest::SubmitWork {
                work_item,
                remote: true,
            });

            Ok(Response::new(actor_service::TellResponse {
                success: true,
                error: None,
                trace_id: req.trace_id,
            }))
        } else {
            Err(Status::invalid_argument("Missing message envelope"))
        }
    }

    async fn ask(
        &self,
        request: Request<actor_service::AskRequest>,
    ) -> std::result::Result<Response<actor_service::AskResponse>, Status> {
        let addr = request
            .remote_addr()
            .ok_or_else(|| Status::internal("Missing remote address"))?;
        let req = request.into_inner();

        if let Some(proto_envelope) = req.envelope {
            let envelope: MessageEnvelope = proto_envelope
                .try_into()
                .map_err(|e| Status::invalid_argument(format!("Invalid envelope: {}", e)))?;

            let addr = ActorAddress::new(addr.ip().to_string(), addr.port());
            let actor_path = ActorPath::parse(req.actor_id.as_str())
                .map_err(|e| Status::invalid_argument(format!("Invalid actor path: {}", e)))?;
            tracing::debug!(
                "Scheduling ask for actor: {}, actor_path: {:?}",
                req.actor_id,
                actor_path.full_path()
            );
            let actor_id = ActorId::new("".to_string(), actor_path);
            let response_result = self
                .schedule_remote_envelope_rpc_message(actor_id, envelope, addr)
                .await;
            match response_result {
                Ok(response_envelope) => Ok(Response::new(actor_service::AskResponse {
                    correlation_id: req.correlation_id,
                    success: true,
                    response_envelope: Some(response_envelope.into()),
                    error: None,
                    trace_id: req.trace_id,
                })),
                Err(e) => Ok(Response::new(actor_service::AskResponse {
                    correlation_id: req.correlation_id,
                    success: false,
                    response_envelope: None,
                    error: Some(e.to_string()),
                    trace_id: req.trace_id,
                })),
            }
        } else {
            Err(Status::invalid_argument("Missing message envelope"))
        }
    }

    async fn ping(
        &self,
        request: Request<actor_service::PingRequest>,
    ) -> std::result::Result<Response<actor_service::PingResponse>, Status> {
        let _req = request.into_inner();

        Ok(Response::new(actor_service::PingResponse {
            node_id: "current_node".to_string(),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64,
            capabilities: Some(self.node_capabilities.clone()),
        }))
    }

    async fn get_node_info(
        &self,
        request: Request<actor_service::NodeInfoRequest>,
    ) -> std::result::Result<Response<actor_service::NodeInfoResponse>, Status> {
        let _req = request.into_inner();

        Ok(Response::new(actor_service::NodeInfoResponse {
            node_id: "current_node".to_string(),
            version: "1.0.0".to_string(),
            capabilities: Some(self.node_capabilities.clone()),
            supported_message_types: vec![],
        }))
    }

    async fn stream_tell(
        &self,
        request: Request<Streaming<TellRequest>>,
    ) -> std::result::Result<Response<Self::StreamTellStream>, Status> {
        let mut stream = request.into_inner();
        let (response_tx, response_rx) = mpsc::unbounded_channel();
        let job_sender = self.job_sender.clone();

        // Handle stream
        tokio::spawn(async move {
            let mut batch_count = 0u64;
            let mut success_count = 0u64;

            while let Some(tell_request) = stream.next().await {
                match tell_request {
                    Ok(req) => {
                        batch_count += 1;

                        // Handle single tell request
                        let result = Self::process_tell_request(req.clone(), &job_sender).await;

                        let response = TellResponse {
                            success: result.is_ok(),
                            error: result.err().map(|e| e.to_string()),
                            trace_id: req.trace_id,
                        };

                        if response.success {
                            success_count += 1;
                        }

                        if response_tx.send(Ok(response)).is_err() {
                            tracing::debug!("Stream tell response channel closed");
                            break;
                        }
                    }
                    Err(status) => {
                        tracing::error!("Stream tell request error: {}", status);
                        let error_response = TellResponse {
                            success: false,
                            error: Some(status.message().to_string()),
                            trace_id: None,
                        };
                        if response_tx.send(Ok(error_response)).is_err() {
                            break;
                        }
                    }
                }
            }

            tracing::info!(
                "Stream tell completed: {} requests processed, {} successful",
                batch_count,
                success_count
            );
        });

        Ok(Response::new(UnboundedReceiverStream::new(response_rx)))
    }

    async fn stream_ask(
        &self,
        request: Request<Streaming<AskRequest>>,
    ) -> std::result::Result<Response<Self::StreamAskStream>, Status> {
        let mut stream = request.into_inner();
        let (response_tx, response_rx) = mpsc::unbounded_channel();
        let job_sender = self.job_sender.clone();
        let message_registry = Arc::clone(&self.message_registry);

        // Handle stream
        tokio::spawn(async move {
            let mut batch_count = 0u64;
            let mut success_count = 0u64;

            while let Some(ask_request) = stream.next().await {
                match ask_request {
                    Ok(req) => {
                        batch_count += 1;

                        // Handle single ask request
                        let result =
                            Self::process_ask_request(req.clone(), &job_sender, &message_registry)
                                .await;

                        let response = match result {
                            Ok(response_envelope) => AskResponse {
                                correlation_id: req.correlation_id.clone(),
                                success: true,
                                response_envelope: Some(response_envelope.into()),
                                error: None,
                                trace_id: req.trace_id,
                            },
                            Err(e) => AskResponse {
                                correlation_id: req.correlation_id.clone(),
                                success: false,
                                response_envelope: None,
                                error: Some(e.to_string()),
                                trace_id: req.trace_id,
                            },
                        };

                        if response.success {
                            success_count += 1;
                        }

                        if response_tx.send(Ok(response)).is_err() {
                            tracing::debug!("Stream ask response channel closed");
                            break;
                        }
                    }
                    Err(status) => {
                        tracing::error!("Stream ask request error: {}", status);
                        let error_response = AskResponse {
                            correlation_id: "unknown".to_string(),
                            success: false,
                            response_envelope: None,
                            error: Some(status.message().to_string()),
                            trace_id: None,
                        };
                        if response_tx.send(Ok(error_response)).is_err() {
                            break;
                        }
                    }
                }
            }

            tracing::info!(
                "Stream ask completed: {} requests processed, {} successful",
                batch_count,
                success_count
            );
        });

        Ok(Response::new(UnboundedReceiverStream::new(response_rx)))
    }

    async fn bidirectional_stream(
        &self,
        request: Request<Streaming<StreamMessage>>,
    ) -> std::result::Result<Response<Self::BidirectionalStreamStream>, Status> {
        let mut stream = request.into_inner();
        let (response_tx, response_rx) = mpsc::unbounded_channel();
        let job_sender = self.job_sender.clone();
        let message_registry = Arc::clone(&self.message_registry);

        // Currently set to 1000 concurrent tasks
        let concurrency_limit = Arc::new(Semaphore::new(self.concurrency_limit));

        // Handle the bidirectional stream
        tokio::spawn(async move {
            let mut total_messages = 0u64;
            let mut tell_count = 0u64;
            let mut ask_count = 0u64;

            // Using FuturesUnordered allows us to handle multiple futures concurrently
            let mut processing_futures = FuturesUnordered::new();

            loop {
                tokio::select! {
                    // Receive a new message from the stream
                    stream_result = stream.next() => {
                        match stream_result {
                            Some(Ok(msg)) => {
                                total_messages += 1;

                                // Request a permit from the semaphore
                                let permit = concurrency_limit.clone().acquire_owned().await.unwrap();
                                let job_sender = job_sender.clone();
                                let message_registry = Arc::clone(&message_registry);
                                let response_tx = response_tx.clone();

                                // Handle the message in a new async task
                                let future = async move {
                                    let _permit = permit; // Keep the permit alive for the duration of this task

                                    let (msg_type, response) = Self::process_stream_message_parallel(
                                        msg,
                                        &job_sender,
                                        &message_registry,
                                    ).await;

                                    if let Some(resp) = response {
                                        let _ = response_tx.send(Ok(resp));
                                    }

                                    msg_type // Return the message type for counting
                                };

                                processing_futures.push(future);
                            }
                            Some(Err(status)) => {
                                tracing::error!("Bidirectional stream message error: {}", status);
                                continue;
                            }
                            None => {
                                // Stream has ended, wait for all processing to complete
                                break;
                            }
                        }
                    }
                    // Handle completed futures
                    result = processing_futures.next(), if !processing_futures.is_empty() => {
                        if let Some(msg_type) = result {
                            match msg_type {
                                MessageType::Tell => tell_count += 1,
                                MessageType::Ask => ask_count += 1,
                            }
                        }
                    }
                }
            }

            // Wait for all remaining futures to complete
            while let Some(msg_type) = processing_futures.next().await {
                match msg_type {
                    MessageType::Tell => tell_count += 1,
                    MessageType::Ask => ask_count += 1,
                }
            }

            tracing::info!(
                "Bidirectional stream completed: {} total messages, {} tells, {} asks",
                total_messages,
                tell_count,
                ask_count
            );
        });

        Ok(Response::new(UnboundedReceiverStream::new(response_rx)))
    }
}
