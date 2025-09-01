use crate::actor_ref::LocalActorRef;
use crate::actor_system::ActorSystemInner;
use crate::error::LyricoreActorError;
use crate::message::InboxMessage;
use crate::path::ActorAddress;
use crate::serialization::MessageEnvelope;
use crate::{ActorContext, ActorId};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::time::{Duration, Instant};

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
    /// Internal command to notify when an actor finishes processing
    ActorProcessingComplete {
        actor_id: ActorId,
        processed_count: usize,
        processing_time_ms: u64,
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

/// Actor processing state to track concurrent message processing
#[derive(Debug)]
struct ActorProcessingState {
    /// Whether the actor is currently processing messages
    is_processing: bool,
    /// Queue of pending messages for this actor
    message_queue: VecDeque<Box<dyn std::any::Any + Send>>,
    /// Last time this actor was processed
    last_processed: Instant,
    /// Number of messages processed by this actor
    processed_count: u64,
}

impl Default for ActorProcessingState {
    fn default() -> Self {
        Self {
            is_processing: false,
            message_queue: VecDeque::new(),
            last_processed: Instant::now(),
            processed_count: 0,
        }
    }
}

/// The worker shard that handles a subset of actors and their messages with concurrent processing.
struct WorkerShard {
    worker_id: usize,
    /// Map of actor IDs to their references
    actors: HashMap<ActorId, LocalActorRef>,
    /// Map of actor IDs to their processing states
    actor_states: HashMap<ActorId, ActorProcessingState>,
    /// Channel to receive commands from the scheduler
    command_rx: mpsc::UnboundedReceiver<ShardCommand>,
    /// Channel to send commands back to self (for async notifications)
    self_tx: mpsc::UnboundedSender<ShardCommand>,
    /// Shard statistics
    stats: ShardStats,
    /// Configuration
    config: SchedulerConfig,
    /// Cache the last accessed actor to avoid repeated lookups
    actor_cache: Option<(ActorId, LocalActorRef)>,
    /// Reference to the actor system
    as_inner: Arc<ActorSystemInner>,
    /// Semaphore to limit concurrent actor processing
    processing_semaphore: Arc<tokio::sync::Semaphore>,
}

#[derive(Default, Clone, Debug)]
pub struct ShardStats {
    pub processed_messages: u64,
    pub queue_length: usize,
    pub actor_count: usize,
    pub cache_hits: u64,
    pub cache_misses: u64,
    /// Number of actors currently processing messages
    pub concurrent_processing: usize,
    /// Average processing time per message (in milliseconds)
    pub avg_processing_time_ms: f64,
}

#[derive(Clone)]
pub struct SchedulerConfig {
    pub worker_threads: usize,
    pub max_mailbox_size: usize,
    pub batch_size: usize,
    pub max_batch_wait_ms: u64,
    pub enable_actor_cache: bool,
    pub batch_send_threshold: usize,
    /// Maximum number of actors that can process messages concurrently per shard
    pub max_concurrent_actors_per_shard: usize,
    /// Interval to check for idle actors (in milliseconds)
    pub idle_actor_check_interval_ms: u64,
    /// Maximum number of actors to schedule in one batch
    pub max_actors_per_schedule_batch: usize,
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
            max_concurrent_actors_per_shard: 64,
            idle_actor_check_interval_ms: 5,
            max_actors_per_schedule_batch: 16,
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
        let (self_tx, _) = mpsc::unbounded_channel();
        let processing_semaphore = Arc::new(tokio::sync::Semaphore::new(
            config.max_concurrent_actors_per_shard,
        ));

        Self {
            worker_id,
            actors: HashMap::with_capacity(1024),
            actor_states: HashMap::with_capacity(1024),
            command_rx,
            self_tx,
            stats: ShardStats::default(),
            config,
            actor_cache: None,
            as_inner,
            processing_semaphore,
        }
    }

    async fn run(&mut self) {
        let mut idle_check_timer = tokio::time::interval(Duration::from_millis(
            self.config.idle_actor_check_interval_ms,
        ));
        idle_check_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        // Create a new receiver that combines external commands with internal notifications
        let (internal_tx, mut internal_rx) = mpsc::unbounded_channel();
        self.self_tx = internal_tx;

        loop {
            tokio::select! {
                Some(cmd) = self.command_rx.recv() => {
                    match self.handle_command(cmd).await {
                        Ok(should_shutdown) => {
                            if should_shutdown {
                                break;
                            }
                        }
                        Err(e) => {
                            tracing::error!("Worker {}: Error handling command: {}", self.worker_id, e);
                        }
                    }
                }

                Some(internal_cmd) = internal_rx.recv() => {
                    match self.handle_command(internal_cmd).await {
                        Ok(should_shutdown) => {
                            if should_shutdown {
                                break;
                            }
                        }
                        Err(e) => {
                            tracing::error!("Worker {}: Error handling internal command: {}", self.worker_id, e);
                        }
                    }
                }

                _ = idle_check_timer.tick() => {
                    // Periodically check for idle actors with pending messages
                    self.schedule_idle_actors().await;
                }
            }
        }

        // Shutdown: wait for all processing to complete
        self.shutdown_all_actors().await;
    }

    /// Handle both external and internal commands
    async fn handle_command(&mut self, cmd: ShardCommand) -> Result<bool, LyricoreActorError> {
        match cmd {
            ShardCommand::RegisterActor {
                actor_id,
                numeric_id,
                actor_ref,
            } => {
                tracing::debug!(
                    "Registering actor {:?} with numeric ID {} on worker {}",
                    actor_id,
                    numeric_id,
                    self.worker_id
                );
                self.actors.insert(actor_id.clone(), actor_ref);
                self.actor_states
                    .insert(actor_id, ActorProcessingState::default());
                self.stats.actor_count = self.actors.len();
                Ok(false)
            }

            ShardCommand::UnregisterActor { actor_id, .. } => {
                self.unregister_actor(actor_id).await;
                Ok(false)
            }

            ShardCommand::ProcessMessage {
                actor_id,
                message,
                ..
            } => {
                self.enqueue_message(actor_id, message).await;
                Ok(false)
            }

            ShardCommand::ActorProcessingComplete {
                actor_id,
                processed_count,
                processing_time_ms,
            } => {
                self.handle_processing_complete(actor_id, processed_count, processing_time_ms)
                    .await;
                Ok(false)
            }

            ShardCommand::GetShardStats { response } => {
                self.update_stats();
                let _ = response.send(self.stats.clone());
                Ok(false)
            }

            ShardCommand::Shutdown => {
                tracing::info!("Worker {} received shutdown command", self.worker_id);
                Ok(true)
            }
        }
    }

    /// Enqueue a message for an actor and potentially schedule it for processing
    async fn enqueue_message(
        &mut self,
        actor_id: ActorId,
        message: Box<dyn std::any::Any + Send>,
    ) {
        if let Some(actor_state) = self.actor_states.get_mut(&actor_id) {
            actor_state.message_queue.push_back(message);

            // If the actor is not currently processing and has messages, try to schedule it
            if !actor_state.is_processing && !actor_state.message_queue.is_empty() {
                self.try_schedule_actor(actor_id).await;
            }
        } else {
            tracing::warn!(
                "Worker {}: Message for unknown actor {:?}",
                self.worker_id,
                actor_id
            );
        }

        self.update_queue_stats();
    }

    /// Try to schedule an actor for processing if resources are available
    async fn try_schedule_actor(&mut self, actor_id: ActorId) {
        // Try to acquire a permit for concurrent processing
        if let Ok(permit) = self.processing_semaphore.clone().try_acquire_owned() {
            // Check actor state first
            let should_schedule = if let Some(actor_state) = self.actor_states.get(&actor_id) {
                !actor_state.is_processing && !actor_state.message_queue.is_empty()
            } else {
                false
            };

            if should_schedule {
                // Get actor reference before mutable borrow
                let actor_ref = self.get_actor_fast(actor_id.clone());

                if let Some(actor_ref) = actor_ref {
                    // Now get mutable reference to state and extract messages
                    if let Some(actor_state) = self.actor_states.get_mut(&actor_id) {
                        // Mark actor as processing
                        actor_state.is_processing = true;
                        actor_state.last_processed = Instant::now();

                        // Extract all pending messages (ensures order preservation)
                        let messages: Vec<_> = actor_state.message_queue.drain(..).collect();
                        let message_count = messages.len();

                        tracing::debug!(
                            "Worker {}: Scheduling actor {:?} with {} messages",
                            self.worker_id,
                            actor_id,
                            message_count
                        );

                        // Spawn async task to process messages
                        let as_inner = Arc::clone(&self.as_inner);
                        let actor_id_clone = actor_id.clone();
                        let self_tx = self.self_tx.clone();

                        tokio::spawn(async move {
                            let start_time = Instant::now();

                            let result = Self::process_actor_messages_concurrent(
                                actor_ref,
                                messages,
                                as_inner,
                            )
                                .await;

                            let processing_time = start_time.elapsed();

                            // Notify the shard that processing is complete
                            let _ = self_tx.send(ShardCommand::ActorProcessingComplete {
                                actor_id: actor_id_clone,
                                processed_count: message_count,
                                processing_time_ms: processing_time.as_millis() as u64,
                            });

                            // Release the semaphore permit
                            drop(permit);

                            if let Err(e) = result {
                                tracing::error!("Error in concurrent actor processing: {}", e);
                            }
                        });

                        self.stats.concurrent_processing += 1;
                    } else {
                        // Actor state not found, release permit
                        drop(permit);
                    }
                } else {
                    // Actor reference not found, reset processing flag and release permit
                    if let Some(actor_state) = self.actor_states.get_mut(&actor_id) {
                        actor_state.is_processing = false;
                    }
                    drop(permit);
                    tracing::warn!(
                        "Worker {}: Actor reference not found for {:?}",
                        self.worker_id,
                        actor_id
                    );
                }
            } else {
                // Don't need to schedule, release permit
                drop(permit);
            }
        }
        // If we can't acquire a permit, the actor will be scheduled later
    }

    /// Process messages for an actor concurrently
    async fn process_actor_messages_concurrent(
        actor_ref: LocalActorRef,
        messages: Vec<Box<dyn std::any::Any + Send>>,
        as_inner: Arc<ActorSystemInner>,
    ) -> Result<(), LyricoreActorError> {
        let actor_id = actor_ref.actor_id().clone();
        let mut ctx = ActorContext::new(actor_id, actor_ref.clone(), as_inner, None);

        for message in messages {
            if let Err(e) = actor_ref.process_message(message, &mut ctx).await {
                tracing::warn!(
                    "Error processing message for actor {:?}: {}",
                    actor_ref.actor_id(),
                    e
                );
                // Continue processing other messages even if one fails
            }
        }

        Ok(())
    }

    /// Handle notification that an actor has completed processing
    async fn handle_processing_complete(
        &mut self,
        actor_id: ActorId,
        processed_count: usize,
        processing_time_ms: u64,
    ) {
        if let Some(actor_state) = self.actor_states.get_mut(&actor_id) {
            actor_state.is_processing = false;
            actor_state.processed_count += processed_count as u64;
            self.stats.processed_messages += processed_count as u64;

            // Update average processing time
            let total_processed = self.stats.processed_messages;
            if total_processed > 0 {
                self.stats.avg_processing_time_ms = (self.stats.avg_processing_time_ms
                    * (total_processed - processed_count as u64) as f64
                    + processing_time_ms as f64 * processed_count as f64)
                    / total_processed as f64;
            }

            self.stats.concurrent_processing = self.stats.concurrent_processing.saturating_sub(1);

            // If the actor has more messages pending, try to schedule it again
            if !actor_state.message_queue.is_empty() {
                self.try_schedule_actor(actor_id).await;
            }
        }

        self.update_queue_stats();
    }

    /// Schedule idle actors that have pending messages
    async fn schedule_idle_actors(&mut self) {
        let now = Instant::now();
        let mut scheduled_count = 0;

        // Collect actor IDs that need scheduling (to avoid borrowing issues)
        let mut actors_to_schedule = Vec::new();

        for (actor_id, state) in &self.actor_states {
            if !state.is_processing && !state.message_queue.is_empty() {
                // Check if the actor has been idle for a reasonable time
                let idle_duration = now.duration_since(state.last_processed);
                if idle_duration.as_millis() > self.config.idle_actor_check_interval_ms as u128 {
                    actors_to_schedule.push(actor_id.clone());
                }
            }
        }

        // Schedule actors up to the batch limit
        for actor_id in actors_to_schedule {
            if scheduled_count >= self.config.max_actors_per_schedule_batch {
                break;
            }

            self.try_schedule_actor(actor_id).await;
            scheduled_count += 1;
        }

        if scheduled_count > 0 {
            tracing::trace!(
                "Worker {}: Scheduled {} idle actors",
                self.worker_id,
                scheduled_count
            );
        }
    }

    /// Unregister an actor and handle cleanup
    async fn unregister_actor(&mut self, actor_id: ActorId) {
        if let Some(mut actor_state) = self.actor_states.remove(&actor_id) {
            // Wait for the actor to finish processing if it's currently active
            while actor_state.is_processing {
                tracing::debug!(
                    "Worker {}: Waiting for actor {:?} to finish processing before unregister",
                    self.worker_id,
                    actor_id
                );
                tokio::time::sleep(Duration::from_millis(1)).await;
                // In practice, we might want to use a more sophisticated waiting mechanism
                if let Some(updated_state) = self.actor_states.get(&actor_id) {
                    actor_state.is_processing = updated_state.is_processing;
                } else {
                    break;
                }
            }

            // Process any remaining messages
            if !actor_state.message_queue.is_empty() {
                if let Some(actor_ref) = self.actors.get(&actor_id) {
                    let remaining_messages: Vec<_> = actor_state.message_queue.drain(..).collect();
                    tracing::debug!(
                        "Worker {}: Processing {} remaining messages for unregistering actor {:?}",
                        self.worker_id,
                        remaining_messages.len(),
                        actor_id
                    );

                    let as_inner = Arc::clone(&self.as_inner);
                    let actor_ref = actor_ref.clone();

                    // Process remaining messages synchronously to ensure cleanup
                    if let Err(e) = Self::process_actor_messages_concurrent(
                        actor_ref,
                        remaining_messages,
                        as_inner,
                    )
                        .await
                    {
                        tracing::error!(
                            "Error processing remaining messages during actor unregister: {}",
                            e
                        );
                    }
                }
            }
        }

        // Remove actor reference
        self.actors.remove(&actor_id);

        // Clear cache if it matches the unregistered actor
        if let Some((cached_id, _)) = &self.actor_cache {
            if *cached_id == actor_id {
                self.actor_cache = None;
            }
        }

        self.stats.actor_count = self.actors.len();
        tracing::debug!(
            "Worker {}: Unregistered actor {:?}",
            self.worker_id,
            actor_id
        );
    }

    /// Shutdown all actors and process remaining messages
    async fn shutdown_all_actors(&mut self) {
        tracing::info!("Worker {}: Shutting down all actors", self.worker_id);

        // Wait for all actors to finish processing
        let mut wait_count = 0;
        while self
            .actor_states
            .values()
            .any(|state| state.is_processing)
        {
            wait_count += 1;
            if wait_count % 100 == 0 {
                let processing_count = self
                    .actor_states
                    .values()
                    .filter(|state| state.is_processing)
                    .count();
                tracing::info!(
                    "Worker {}: Waiting for {} actors to finish processing",
                    self.worker_id,
                    processing_count
                );
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        // Process all remaining messages
        for (actor_id, mut state) in self.actor_states.drain() {
            if !state.message_queue.is_empty() {
                if let Some(actor_ref) = self.actors.get(&actor_id) {
                    let remaining_messages: Vec<_> = state.message_queue.drain(..).collect();
                    tracing::debug!(
                        "Worker {}: Processing {} remaining messages for actor {:?} during shutdown",
                        self.worker_id,
                        remaining_messages.len(),
                        actor_id
                    );

                    let as_inner = Arc::clone(&self.as_inner);
                    let actor_ref = actor_ref.clone();

                    if let Err(e) = Self::process_actor_messages_concurrent(
                        actor_ref,
                        remaining_messages,
                        as_inner,
                    )
                        .await
                    {
                        tracing::error!("Error processing messages during shutdown: {}", e);
                    }
                }
            }
        }

        tracing::info!("Worker {}: Shutdown complete", self.worker_id);
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

    /// Update queue-related statistics
    fn update_queue_stats(&mut self) {
        let total_queued: usize = self
            .actor_states
            .values()
            .map(|state| state.message_queue.len())
            .sum();
        self.stats.queue_length = total_queued;
    }

    /// Update all statistics
    fn update_stats(&mut self) {
        self.update_queue_stats();
        self.stats.actor_count = self.actors.len();
        self.stats.concurrent_processing = self
            .actor_states
            .values()
            .filter(|state| state.is_processing)
            .count();
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
                        None => {
                            tracing::info!("Scheduler job request channel closed, exiting control loop.");
                            break;
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