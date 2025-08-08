use crate::actor_ref::LocalActorRef;
use crate::actor_system::ActorSystemInner;
// use crate::cooperative_worker::{CooperativeConfig, CooperativeWorkerShard};
use crate::error::LyricoreActorError;
use crate::message::InboxMessage;
use crate::path::ActorAddress;
use crate::serialization::MessageEnvelope;
use crate::{ActorContext, ActorId};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::sync::oneshot;

pub type ActorNumericId = u64;

/// Actor ID to numeric ID conversion.
pub fn string_to_numeric_id(actor_id: &ActorId) -> ActorNumericId {
    actor_id.runtime_hash()
}

pub enum ShardCommand {
    RegisterActor {
        actor_id: ActorId,
        numeric_id: ActorNumericId,
        actor_ref: LocalActorRef,
    },
    UnregisterActor {
        actor_id: ActorId,
        numeric_id: ActorNumericId,
    },
    ProcessMessage {
        actor_id: ActorId,
        numeric_id: ActorNumericId,
        message: Box<dyn std::any::Any + Send>,
    },
    GetShardStats {
        response: oneshot::Sender<ShardStats>,
    },
    Shutdown,
}

pub enum SchedulerJobRequest {
    SubmitWork {
        work_item: WorkItem,
        remote: bool, // Whether this is a remote message
    },
    RegisterActor {
        actor_id: ActorId,
        actor_ref: LocalActorRef,
    },
    UnregisterActor {
        actor_id: ActorId,
    },
}
pub enum SchedulerCommand {
    GetStats {
        response: oneshot::Sender<SchedulerStats>,
    },
    Shutdown,
}

pub struct WorkItem {
    pub actor_id: ActorId,
    pub numeric_id: ActorNumericId,
    pub priority: u8,
    pub message: Box<dyn std::any::Any + Send>,
    pub created_at: u64,
}

impl WorkItem {
    pub fn new(actor_id: ActorId, message: Box<dyn std::any::Any + Send>) -> Self {
        let numeric_id = string_to_numeric_id(&actor_id);
        Self {
            actor_id,
            numeric_id,
            priority: 128,
            message,
            created_at: crate::utils::get_timestamp(),
        }
    }

    pub fn with_priority(mut self, priority: u8) -> Self {
        self.priority = priority;
        self
    }
}

/// The worker shard that handles a subset of actors and their messages.
struct WorkerShard {
    worker_id: usize,
    // Use the numeric ID as the key for faster access
    actors: HashMap<ActorId, LocalActorRef>,
    // Pre-allocated vector to reduce reallocations
    pending_messages: Vec<(ActorId, Box<dyn std::any::Any + Send>)>,
    command_rx: mpsc::UnboundedReceiver<ShardCommand>,
    stats: ShardStats,
    config: SchedulerConfig,
    // Cache the last accessed actor to avoid repeated lookups
    actor_cache: Option<(ActorId, LocalActorRef)>,
    as_inner: Arc<ActorSystemInner>,
}

#[derive(Default, Clone, Debug)]
pub struct ShardStats {
    pub processed_messages: u64,
    pub queue_length: usize,
    pub actor_count: usize,
    pub cache_hits: u64,
    pub cache_misses: u64,
}

#[derive(Clone)]
pub struct SchedulerConfig {
    pub worker_threads: usize,
    pub max_mailbox_size: usize,
    pub batch_size: usize,
    pub max_batch_wait_ms: u64,
    pub enable_actor_cache: bool,
    pub batch_send_threshold: usize,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            worker_threads: num_cpus::get(),
            max_mailbox_size: 10000,
            batch_size: 128,
            max_batch_wait_ms: 1,
            enable_actor_cache: true,
            batch_send_threshold: 64,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SchedulerStats {
    pub total_messages: u64,
    pub processed_messages: u64,
    pub active_workers: usize,
    pub shard_stats: Vec<ShardStats>,
}

impl WorkerShard {
    fn new(
        worker_id: usize,
        command_rx: mpsc::UnboundedReceiver<ShardCommand>,
        config: SchedulerConfig,
        as_inner: Arc<ActorSystemInner>,
    ) -> Self {
        Self {
            worker_id,
            actors: HashMap::with_capacity(1024), // Preallocate for better performance
            pending_messages: Vec::with_capacity(config.batch_size * 2),
            command_rx,
            stats: ShardStats::default(),
            config,
            actor_cache: None,
            as_inner,
        }
    }

    async fn run(&mut self) {
        let mut batch_timer = tokio::time::interval(tokio::time::Duration::from_millis(
            self.config.max_batch_wait_ms,
        ));
        batch_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                Some(cmd) = self.command_rx.recv() => {
                    match cmd {
                        ShardCommand::RegisterActor { actor_id, numeric_id, actor_ref } => {
                            tracing::debug!("Registering actor {:?} with numeric ID {} on worker {}",
                                actor_id, numeric_id, self.worker_id);
                            self.actors.insert(actor_id, actor_ref);
                            self.stats.actor_count = self.actors.len();
                        }
                        ShardCommand::UnregisterActor { actor_id, numeric_id: _ } => {
                            // 1. First, handle all pending messages for this actor (including possible OnStop)
                            self.flush_pending_messages_for_actor(actor_id.clone()).await;
                            self.actors.remove(&actor_id);
                            // Clear the cache if it matches the unregistered actor
                            if let Some((cached_id, _)) = &self.actor_cache {
                                if *cached_id == actor_id {
                                    self.actor_cache = None;
                                }
                            }
                            self.stats.actor_count = self.actors.len();
                        }
                        ShardCommand::ProcessMessage {actor_id,  numeric_id: _, message } => {
                            self.pending_messages.push((actor_id, message));
                            self.stats.queue_length = self.pending_messages.len();

                            if self.pending_messages.len() >= self.config.batch_size {
                                self.process_message_batch().await;
                            }
                        }
                        ShardCommand::GetShardStats { response } => {
                            let _ = response.send(self.stats.clone());
                        }
                        ShardCommand::Shutdown => {
                            if !self.pending_messages.is_empty() {
                                self.process_message_batch().await;
                            }
                            break;
                        }
                    }
                }

                _ = batch_timer.tick() => {
                    if !self.pending_messages.is_empty() {
                        self.process_message_batch().await;
                    }
                }
            }
        }
    }

    // Batch processing of messages for all actors
    async fn process_message_batch(&mut self) {
        if self.pending_messages.is_empty() {
            return;
        }

        let mut actor_messages: HashMap<ActorId, Vec<Box<dyn std::any::Any + Send>>> =
            HashMap::new();

        for (actor_id, message) in self.pending_messages.drain(..) {
            actor_messages
                .entry(actor_id)
                .or_insert_with(Vec::new)
                .push(message);
        }

        self.stats.queue_length = 0;

        // Batch processing of messages for each actor
        for (actor_id, messages) in actor_messages {
            let actor_ref = self.get_actor_fast(actor_id.clone());
            if let Some(actor_ref) = actor_ref {
                self.process_actor_messages(&actor_ref, messages).await;
            } else {
                tracing::warn!(
                    "Worker {}: Actor not found for ID: {:?}",
                    self.worker_id,
                    actor_id
                );
            }
        }
    }
    // Handle flushing of pending messages for a specific actor
    async fn flush_pending_messages_for_actor(&mut self, target_numeric_id: ActorId) {
        let mut remaining_messages = Vec::new();
        let mut target_messages = Vec::new();

        for (numeric_id, message) in self.pending_messages.drain(..) {
            if numeric_id == target_numeric_id {
                target_messages.push(message);
            } else {
                remaining_messages.push((numeric_id, message));
            }
        }

        // Restore remaining messages to the pending queue
        self.pending_messages = remaining_messages;
        self.stats.queue_length = self.pending_messages.len();

        // Handle all messages for the target actor immediately
        if !target_messages.is_empty() {
            if let Some(actor_ref) = self.actors.get(&target_numeric_id) {
                tracing::debug!(
                    "Worker {}: Flushing {} pending messages for actor {}",
                    self.worker_id,
                    target_messages.len(),
                    target_numeric_id
                );
                // Add some timeout to avoid blocking indefinitely
                self.process_actor_messages(&actor_ref.clone(), target_messages)
                    .await;
            }
        }
    }

    /// Uses a cache to quickly retrieve actors by their ID.
    fn get_actor_fast(&mut self, actor_id: ActorId) -> Option<LocalActorRef> {
        if self.config.enable_actor_cache {
            // Check if the actor is in the cache
            if let Some((cached_id, cached_ref)) = &self.actor_cache {
                if *cached_id == actor_id {
                    self.stats.cache_hits += 1;
                    return Some(cached_ref.clone());
                }
            }
        }

        // Not in cache, look it up in the HashMap
        if let Some(actor_ref) = self.actors.get(&actor_id) {
            let actor_ref = actor_ref.clone();

            if self.config.enable_actor_cache {
                self.actor_cache = Some((actor_id, actor_ref.clone()));
                self.stats.cache_misses += 1;
            }

            Some(actor_ref)
        } else {
            None
        }
    }

    async fn process_actor_messages(
        &mut self,
        actor_ref: &LocalActorRef,
        messages: Vec<Box<dyn std::any::Any + Send>>,
    ) {
        let message_count = messages.len();

        // Run messages batch processing in the actor's context
        let as_inner = Arc::clone(&self.as_inner);
        let actor_id = actor_ref.actor_id().clone();
        let mut ctx = ActorContext::new(actor_id, actor_ref.clone(), as_inner, None);
        for message in messages {
            if let Err(e) = actor_ref.process_message(message, &mut ctx).await {
                tracing::warn!("Worker {}: Error processing message: {}", self.worker_id, e);
            }
        }
        self.stats.processed_messages += message_count as u64;
    }
}

pub struct WorkScheduler {
    pub(crate) command_tx: mpsc::UnboundedSender<SchedulerCommand>,
    scheduler_job_request_tx: mpsc::UnboundedSender<SchedulerJobRequest>,
    shard_senders: Vec<mpsc::UnboundedSender<ShardCommand>>,
    total_messages: Arc<AtomicU64>,
    worker_count: usize,
    config: SchedulerConfig,
    as_inner: Arc<ActorSystemInner>,
}

impl WorkScheduler {
    pub fn new(
        as_inner: Arc<ActorSystemInner>,
        config: SchedulerConfig,
        scheduler_cmd_tx: mpsc::UnboundedSender<SchedulerCommand>,
        scheduler_cmd_rx: mpsc::UnboundedReceiver<SchedulerCommand>,
        job_request_tx: mpsc::UnboundedSender<SchedulerJobRequest>,
        job_request_rx: mpsc::UnboundedReceiver<SchedulerJobRequest>,
    ) -> Arc<Self> {
        let worker_count = config.worker_threads;

        let mut shard_senders = Vec::with_capacity(worker_count);

        for worker_id in 0..worker_count {
            let (shard_tx, shard_rx) = mpsc::unbounded_channel();
            shard_senders.push(shard_tx);

            let config_clone = config.clone();

            let as_inner = Arc::clone(&as_inner);
            tokio::spawn(async move {
                let mut shard = WorkerShard::new(worker_id, shard_rx, config_clone, as_inner);
                // let cooperative = CooperativeConfig::default();
                // let mut shard = CooperativeWorkerShard::new(
                //     worker_id,
                //     shard_rx,
                //     cooperative,
                //     as_inner,
                // );
                shard.run().await;
            });
        }

        let scheduler = Arc::new(Self {
            command_tx: scheduler_cmd_tx,
            scheduler_job_request_tx: job_request_tx,
            shard_senders,
            total_messages: Arc::new(AtomicU64::new(0)),
            worker_count,
            config,
            as_inner,
        });

        let scheduler_clone = Arc::clone(&scheduler);
        let shard_senders_clone = scheduler.shard_senders.clone();
        tokio::spawn(async move {
            scheduler_clone
                .control_loop(scheduler_cmd_rx, job_request_rx, shard_senders_clone)
                .await;
        });

        scheduler
    }

    pub(crate) fn job_sender(&self) -> mpsc::UnboundedSender<SchedulerJobRequest> {
        self.scheduler_job_request_tx.clone()
    }

    async fn control_loop(
        &self,
        mut command_rx: mpsc::UnboundedReceiver<SchedulerCommand>,
        mut job_request_rx: mpsc::UnboundedReceiver<SchedulerJobRequest>,
        shard_senders: Vec<mpsc::UnboundedSender<ShardCommand>>,
    ) {
        loop {
            tokio::select! {
                cmd = command_rx.recv() => {
                    match cmd {
                        Some(command) => {
                            match self.handle_internal_scheduler_command(command, &shard_senders).await {
                                Ok(true) => {
                                    tracing::info!("Scheduler shutdown command received, exiting control loop.");
                                    break;
                                }
                                Err(e) => {
                                    tracing::error!("Scheduler command handling error: {}", e);
                                }
                                _ => {
                                }
                            }
                        }
                        None => {
                            tracing::info!("Scheduler command channel closed, exiting control loop.");
                            break;
                        }
                    }
                }
                job_request = job_request_rx.recv() => {
                    match job_request {
                        Some(SchedulerJobRequest::SubmitWork { mut work_item, remote }) => {
                            if remote {
                                if let Some(local_actor_ref) =  self.as_inner.local_actors.get(&work_item.actor_id){
                                    // If it's a remote message, ensure the actor_id's runtime_id is correct
                                    work_item.actor_id.runtime_id = local_actor_ref.runtime_id().to_string();
                                    work_item.numeric_id = string_to_numeric_id(&work_item.actor_id);
                                }
                            }
                            self.schedule_work(work_item);
                        }
                        Some(SchedulerJobRequest::RegisterActor { actor_id, actor_ref }) => {
                            self.register_actor(actor_id, actor_ref);
                        }
                        Some(SchedulerJobRequest::UnregisterActor { actor_id }) => {
                            self.unregister_actor(actor_id);
                        }
                        _ => {
                            tracing::info!("Scheduler job request channel closed, exiting control loop.");
                        }
                    }
                }
            }
        }
    }

    async fn handle_internal_scheduler_command(
        &self,
        command: SchedulerCommand,
        shard_senders: &Vec<mpsc::UnboundedSender<ShardCommand>>,
    ) -> Result<bool, LyricoreActorError> {
        match command {
            SchedulerCommand::GetStats { response } => {
                let mut shard_stats = Vec::with_capacity(self.worker_count);
                let mut handles = Vec::new();
                for shard_sender in shard_senders {
                    let (tx, rx) = oneshot::channel();
                    if shard_sender
                        .send(ShardCommand::GetShardStats { response: tx })
                        .is_ok()
                    {
                        handles.push(rx);
                    }
                }

                for handle in handles {
                    if let Ok(stats) = handle.await {
                        shard_stats.push(stats);
                    }
                }

                let total_processed = shard_stats.iter().map(|s| s.processed_messages).sum();

                let stats = SchedulerStats {
                    total_messages: self.total_messages.load(Ordering::Relaxed),
                    processed_messages: total_processed,
                    active_workers: self.worker_count,
                    shard_stats,
                };

                let _ = response.send(stats);
                Ok(false)
            }
            SchedulerCommand::Shutdown => {
                for sender in shard_senders {
                    let _ = sender.send(ShardCommand::Shutdown);
                }
                tracing::info!("Scheduler shutdown initiated.");
                Ok(true)
            }
        }
    }

    // Register an actor with the scheduler, ensuring consistent hashing
    pub(crate) fn register_actor(&self, actor_id: ActorId, actor_ref: LocalActorRef) {
        let numeric_id = string_to_numeric_id(&actor_id);
        let shard_id = (numeric_id as usize) % self.worker_count;

        let _ = self.shard_senders[shard_id].send(ShardCommand::RegisterActor {
            actor_id,
            numeric_id,
            actor_ref,
        });
    }

    fn unregister_actor(&self, actor_id: ActorId) {
        let numeric_id = string_to_numeric_id(&actor_id);
        let shard_id = (numeric_id as usize) % self.worker_count;

        let _ = self.shard_senders[shard_id].send(ShardCommand::UnregisterActor {
            actor_id,
            numeric_id,
        });
    }

    // Scheduler will handle the routing based on numeric_id
    // This allows for consistent routing of messages to the correct worker shard
    pub fn schedule_work(&self, work_item: WorkItem) {
        self.total_messages.fetch_add(1, Ordering::Relaxed);
        let shard_id = (work_item.numeric_id as usize) % self.worker_count;

        let _ = self.shard_senders[shard_id].send(ShardCommand::ProcessMessage {
            actor_id: work_item.actor_id,
            numeric_id: work_item.numeric_id,
            message: work_item.message,
        });
    }

    pub fn schedule_remote_envelope_message(
        &self,
        actor_id: ActorId,
        envelope: MessageEnvelope,
        addr: ActorAddress,
    ) {
        let message = Box::new(InboxMessage::envelope_message(addr, envelope));
        let work_item = WorkItem::new(actor_id, message);
        self.schedule_work(work_item);
    }

    pub async fn schedule_remote_envelope_rpc_message(
        &self,
        actor_id: ActorId,
        envelope: MessageEnvelope,
        addr: ActorAddress,
    ) -> crate::error::Result<MessageEnvelope> {
        let (response_tx, response_rx) = oneshot::channel();

        let message = Box::new(InboxMessage::rpc_envelope_message(
            addr,
            envelope,
            response_tx,
        ));

        let work_item = WorkItem::new(actor_id, message);
        self.schedule_work(work_item);

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
    }

    pub async fn get_stats(&self) -> SchedulerStats {
        let (tx, rx) = oneshot::channel();
        let _ = self
            .command_tx
            .send(SchedulerCommand::GetStats { response: tx });

        match rx.await {
            Ok(stats) => stats,
            Err(_) => SchedulerStats {
                total_messages: self.total_messages.load(Ordering::Relaxed),
                processed_messages: 0,
                active_workers: self.worker_count,
                shard_stats: vec![],
            },
        }
    }
}
