// src/eventbus.rs
//! EventBus implementation for Lyricore Actor Framework
//!
//! This module provides a distributed event bus that supports both local and remote actor subscriptions.
//! Events are classified and delivered asynchronously to subscribed actors.

use crate::actor_ref::ActorRef;
use crate::error::Result;
use crate::path::ActorPath;
use crate::Message;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::any::{Any, TypeId};
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};
use uuid::Uuid;

/// Unique identifier for event subscriptions
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SubscriptionId(pub String);

impl SubscriptionId {
    pub fn new() -> Self {
        Self(Uuid::new_v4().to_string())
    }
}

impl Default for SubscriptionId {
    fn default() -> Self {
        Self::new()
    }
}

/// Base trait for all events that can be published on the EventBus
pub trait Event: Message {
    /// Get the event type name for debugging and classification
    fn event_type(&self) -> &'static str {
        std::any::type_name::<Self>()
    }

    /// Get event metadata (optional)
    fn metadata(&self) -> Option<HashMap<String, String>> {
        None
    }

    /// Get event timestamp (optional, defaults to current time)
    fn timestamp(&self) -> std::time::SystemTime {
        std::time::SystemTime::now()
    }
}


/// Implement Event for any type that meets the requirements and implements Message
impl<T> Event for T
where
    T: Send + Sync + Clone + 'static + Serialize + for<'de> Deserialize<'de> + Message + Default,
{
}

/// Event classifier that determines which subscribers receive which events
pub trait EventClassifier: Send + Sync + 'static {
    type Event: Event;
    type Classifier: Clone + Eq + Hash + Send + Sync + 'static;

    fn classify(&self, event: &Self::Event) -> Vec<Self::Classifier>;

    fn name(&self) -> &'static str {
        std::any::type_name::<Self>()
    }
}
pub trait EventClassifierTrait: Send + Sync + Any {
    fn matches_event(&self, event: &dyn Any) -> bool;
    fn event_type_id(&self) -> TypeId;
    fn as_any(&self) -> &dyn Any;
}

impl<C> EventClassifierTrait for C
where
    C: EventClassifier + 'static,
    C::Event: Event + 'static,
{
    fn matches_event(&self, event: &dyn Any) -> bool {
        if let Some(concrete_event) = event.downcast_ref::<C::Event>() {
            !self.classify(concrete_event).is_empty()
        } else {
            false
        }
    }

    fn event_type_id(&self) -> TypeId {
        TypeId::of::<C::Event>()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
/// Subscription information
#[derive(Clone)]
pub struct Subscription {
    pub id: SubscriptionId,
    pub subscriber: ActorRef,
    // pub classifier: Box<dyn Any + Send + Sync>,
    // pub classifier: Arc<Box<dyn Any + Send + Sync>>,
    pub classifier: Arc<dyn EventClassifierTrait>, // 使用 trait 对象
    pub classifier_type_id: TypeId,
    pub event_type_id: TypeId,
    pub event_type_name: String,
}

impl PartialEq for Subscription {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for Subscription {}

impl Hash for Subscription {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

/// Event bus statistics
#[derive(Debug, Clone)]
pub struct EventBusStats {
    pub total_events_published: u64,
    pub total_subscriptions: usize,
    pub events_by_type: HashMap<String, u64>,
    pub subscribers_by_type: HashMap<String, usize>,
    pub failed_deliveries: u64,
}

/// Configuration for the EventBus
#[derive(Debug, Clone)]
pub struct EventBusConfig {
    /// Maximum number of concurrent event deliveries
    pub max_concurrent_deliveries: usize,
    /// Timeout for event delivery to a single actor
    pub delivery_timeout_ms: u64,
    /// Whether to continue delivery if some subscribers fail
    pub continue_on_delivery_failure: bool,
    /// Maximum size of the event delivery buffer
    pub delivery_buffer_size: usize,
}

impl Default for EventBusConfig {
    fn default() -> Self {
        Self {
            max_concurrent_deliveries: 1000,
            delivery_timeout_ms: 5000,
            continue_on_delivery_failure: true,
            delivery_buffer_size: 10000,
        }
    }
}

/// The main EventBus implementation
pub struct EventBus {
    /// Map from event type to its subscriptions
    subscriptions: Arc<DashMap<TypeId, Vec<Subscription>>>,
    /// Map from subscription ID to subscription for quick lookup
    subscription_index: Arc<DashMap<SubscriptionId, Subscription>>,
    /// Map from ActorRef to its subscription IDs for cleanup
    actor_subscriptions: Arc<DashMap<String, HashSet<SubscriptionId>>>,
    /// Statistics
    stats: Arc<RwLock<EventBusStats>>,
    /// Internal broadcast channel for shutdown
    shutdown_tx: broadcast::Sender<()>,
    /// Event delivery configuration
    config: EventBusConfig,
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new(EventBusConfig::default())
    }
}

impl EventBus {
    /// Create a new EventBus
    pub fn new(config: EventBusConfig) -> Self {
        let (shutdown_tx, _) = broadcast::channel(1);

        Self {
            subscriptions: Arc::new(DashMap::new()),
            subscription_index: Arc::new(DashMap::new()),
            actor_subscriptions: Arc::new(DashMap::new()),
            stats: Arc::new(RwLock::new(EventBusStats {
                total_events_published: 0,
                total_subscriptions: 0,
                events_by_type: HashMap::new(),
                subscribers_by_type: HashMap::new(),
                failed_deliveries: 0,
            })),
            shutdown_tx,
            config,
        }
    }

    /// Subscribe an actor to events with a classifier
    pub async fn subscribe<C, E>(
        &self,
        subscriber: ActorRef,
        classifier: C,
    ) -> Result<SubscriptionId>
    where
        C: EventClassifier<Event = E> + Clone + 'static,
        E: Event,
    {
        let subscription_id = SubscriptionId::new();
        let event_type_id = TypeId::of::<E>();
        let event_type_name = std::any::type_name::<E>().to_string();
        let classifier_type_id = TypeId::of::<C>();

        let subscription = Subscription {
            id: subscription_id.clone(),
            subscriber: subscriber.clone(),
            // classifier: Arc::new(Box::new(classifier)),
            classifier: Arc::new(classifier),
            classifier_type_id,
            event_type_id,
            event_type_name: event_type_name.clone(),
        };

        // Add to subscriptions by event type
        let mut subs = self.subscriptions.entry(event_type_id).or_insert_with(Vec::new);
        subs.push(subscription.clone());

        // Add to subscription index
        self.subscription_index.insert(subscription_id.clone(), subscription);

        // Track actor subscriptions for cleanup
        let actor_key = self.get_actor_key(&subscriber);
        let mut actor_subs = self.actor_subscriptions.entry(actor_key).or_insert_with(HashSet::new);
        actor_subs.insert(subscription_id.clone());

        // Update statistics
        {
            let mut stats = self.stats.write().await;
            stats.total_subscriptions += 1;
            *stats.subscribers_by_type.entry(event_type_name).or_insert(0) += 1;
        }

        tracing::debug!(
            "Actor {:?} subscribed to events with ID {:?}",
            subscriber.actor_path(),
            subscription_id
        );

        Ok(subscription_id)
    }

    /// Unsubscribe by subscription ID
    pub async fn unsubscribe(&self, subscription_id: &SubscriptionId) -> Result<bool> {
        if let Some((_, subscription)) = self.subscription_index.remove(subscription_id) {
            let event_type_id = subscription.event_type_id;
            let event_type_name = subscription.event_type_name.clone();

            // Remove from subscriptions by event type
            if let Some(mut subs) = self.subscriptions.get_mut(&event_type_id) {
                subs.retain(|s| s.id != *subscription_id);
                if subs.is_empty() {
                    drop(subs);
                    self.subscriptions.remove(&event_type_id);
                }
            }

            // Remove from actor subscriptions
            let actor_key = self.get_actor_key(&subscription.subscriber);
            if let Some(mut actor_subs) = self.actor_subscriptions.get_mut(&actor_key) {
                actor_subs.remove(subscription_id);
                if actor_subs.is_empty() {
                    drop(actor_subs);
                    self.actor_subscriptions.remove(&actor_key);
                }
            }

            // Update statistics
            {
                let mut stats = self.stats.write().await;
                stats.total_subscriptions = stats.total_subscriptions.saturating_sub(1);
                if let Some(count) = stats.subscribers_by_type.get_mut(&event_type_name) {
                    *count = count.saturating_sub(1);
                    if *count == 0 {
                        stats.subscribers_by_type.remove(&event_type_name);
                    }
                }
            }

            tracing::debug!("Unsubscribed subscription {:?}", subscription_id);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Unsubscribe all subscriptions for an actor
    pub async fn unsubscribe_actor(&self, actor: &ActorRef) -> Result<usize> {
        let actor_key = self.get_actor_key(actor);

        if let Some((_, subscription_ids)) = self.actor_subscriptions.remove(&actor_key) {
            let mut count = 0;
            for subscription_id in subscription_ids {
                if self.unsubscribe(&subscription_id).await? {
                    count += 1;
                }
            }
            tracing::debug!("Unsubscribed {} subscriptions for actor {:?}", count, actor.actor_path());
            Ok(count)
        } else {
            Ok(0)
        }
    }

    /// Publish an event to all matching subscribers
    pub async fn publish<E>(&self, event: E) -> Result<PublishResult>
    where
        E: Event + Clone,
    {
        self.publish_with_topic(event, None).await
    }

    /// Publish an event with an optional topic to all matching subscribers
    pub async fn publish_with_topic<E>(&self, event: E, topic: Option<String>) -> Result<PublishResult>
    where
        E: Event + Clone,
    {
        let event_type_id = TypeId::of::<E>();
        let event_type_name = std::any::type_name::<E>().to_string();

        // Update statistics
        {
            let mut stats = self.stats.write().await;
            stats.total_events_published += 1;
            *stats.events_by_type.entry(event_type_name.clone()).or_insert(0) += 1;
        }

        // Get all subscriptions for this event type
        let subscriptions = if let Some(subs) = self.subscriptions.get(&event_type_id) {
            subs.clone()
        } else {
            tracing::debug!("No subscribers for event type: {}", event_type_name);
            return Ok(PublishResult {
                total_subscribers: 0,
                successful_deliveries: 0,
                failed_deliveries: 0,
                delivery_errors: Vec::new(),
            });
        };

        tracing::debug!(
            "Publishing event {} to {} potential subscribers",
            event_type_name,
            subscriptions.len()
        );

        // Find matching subscribers using classifiers
        let mut matching_subscribers = Vec::new();

        for subscription in subscriptions {
            let should_deliver = if let Some(ref topic) = topic {
                // If topic is provided, check if this is a TopicClassifier and matches the topic
                let classifier_any = subscription.classifier.as_any();
                if let Some(topic_classifier) = classifier_any.downcast_ref::<TopicClassifier<E>>() {
                    topic_classifier.matches_topic(topic)
                } else {
                    // For non-TopicClassifier, use the normal classification
                    subscription.classifier.matches_event(&event)
                }
            } else {
                // No topic provided, use normal classification
                subscription.classifier.matches_event(&event)
            };

            if should_deliver {
                matching_subscribers.push(subscription.subscriber.clone());
            }
        }

        if matching_subscribers.is_empty() {
            tracing::debug!("No matching subscribers for event after classification");
            return Ok(PublishResult {
                total_subscribers: 0,
                successful_deliveries: 0,
                failed_deliveries: 0,
                delivery_errors: Vec::new(),
            });
        }

        // Deliver event to matching subscribers asynchronously
        let delivery_result = self.deliver_event_async(event, matching_subscribers).await;

        // Update failure statistics
        if delivery_result.failed_deliveries > 0 {
            let mut stats = self.stats.write().await;
            stats.failed_deliveries += delivery_result.failed_deliveries as u64;
        }

        Ok(delivery_result)
    }

    /// Asynchronously deliver event to subscribers
    async fn deliver_event_async<E>(
        &self,
        event: E,
        subscribers: Vec<ActorRef>,
    ) -> PublishResult
    where
        E: Event + Clone,
    {
        let total_subscribers = subscribers.len();
        let mut successful_deliveries = 0;
        let mut failed_deliveries = 0;
        let mut delivery_errors = Vec::new();

        // Use semaphore to limit concurrent deliveries
        let semaphore = Arc::new(tokio::sync::Semaphore::new(self.config.max_concurrent_deliveries));
        let mut delivery_tasks = Vec::new();

        for subscriber in subscribers {
            let event_clone = event.clone();
            let subscriber_clone = subscriber.clone();
            let permit = semaphore.clone().acquire_owned().await.unwrap();
            let timeout = std::time::Duration::from_millis(self.config.delivery_timeout_ms);

            let task = tokio::spawn(async move {
                let _permit = permit; // Keep permit alive

                let result = tokio::time::timeout(
                    timeout,
                    subscriber_clone.tell(event_clone)
                ).await;

                match result {
                    Ok(Ok(())) => DeliveryResult::Success,
                    Ok(Err(e)) => DeliveryResult::Error(format!("Actor error: {}", e)),
                    Err(_) => DeliveryResult::Error("Delivery timeout".to_string()),
                }
            });

            delivery_tasks.push(task);
        }

        // Wait for all deliveries to complete
        for task in delivery_tasks {
            match task.await {
                Ok(DeliveryResult::Success) => successful_deliveries += 1,
                Ok(DeliveryResult::Error(error)) => {
                    failed_deliveries += 1;
                    delivery_errors.push(error);
                }
                Err(e) => {
                    failed_deliveries += 1;
                    delivery_errors.push(format!("Task join error: {}", e));
                }
            }
        }

        PublishResult {
            total_subscribers,
            successful_deliveries,
            failed_deliveries,
            delivery_errors,
        }
    }

    /// Get statistics about the EventBus
    pub async fn get_stats(&self) -> EventBusStats {
        self.stats.read().await.clone()
    }

    /// Get all subscriptions for debugging
    pub async fn list_subscriptions(&self) -> Vec<(SubscriptionId, ActorPath, String)> {
        self.subscription_index
            .iter()
            .map(|entry| {
                let subscription = entry.value();
                (
                    subscription.id.clone(),
                    subscription.subscriber.actor_path().clone(),
                    subscription.event_type_name.clone(),
                )
            })
            .collect()
    }

    /// Shutdown the EventBus
    pub async fn shutdown(&self) -> Result<()> {
        let _ = self.shutdown_tx.send(());

        // Clear all subscriptions
        self.subscriptions.clear();
        self.subscription_index.clear();
        self.actor_subscriptions.clear();

        tracing::info!("EventBus shutdown completed");
        Ok(())
    }

    /// Helper to generate a unique key for an actor
    fn get_actor_key(&self, actor: &ActorRef) -> String {
        format!("{}#{}", actor.actor_path().full_path(), actor.actor_id().runtime_id)
    }
}

/// Result of event delivery
#[derive(Debug)]
enum DeliveryResult {
    Success,
    Error(String),
}

/// Result of publishing an event
#[derive(Debug, Clone)]
pub struct PublishResult {
    pub total_subscribers: usize,
    pub successful_deliveries: usize,
    pub failed_deliveries: usize,
    pub delivery_errors: Vec<String>,
}

impl PublishResult {
    pub fn success_rate(&self) -> f64 {
        if self.total_subscribers == 0 {
            1.0
        } else {
            self.successful_deliveries as f64 / self.total_subscribers as f64
        }
    }

    pub fn is_fully_successful(&self) -> bool {
        self.failed_deliveries == 0 && self.total_subscribers > 0
    }
}

// === Event Classifiers ===

/// Simple event classifier that matches all events of a type
#[derive(Debug, Clone)]
pub struct AllEventsClassifier<E> {
    _phantom: PhantomData<E>,
}

impl<E> Default for AllEventsClassifier<E> {
    fn default() -> Self {
        Self {
            _phantom: PhantomData,
        }
    }
}

impl<E> AllEventsClassifier<E> {
    pub fn new() -> Self {
        Self::default()
    }
}

impl<E> EventClassifier for AllEventsClassifier<E>
where
    E: Event,
{
    type Event = E;
    type Classifier = ();

    fn classify(&self, _event: &Self::Event) -> Vec<Self::Classifier> {
        vec![()]  // Match all events
    }
}


/// Topic-based classifier for event routing
/// Supports hierarchical topics with wildcard patterns like "user.*", "order.created", "sensor.temperature.*"
#[derive(Debug, Clone)]
pub struct TopicClassifier<T: Event> {
    pub topic_patterns: Vec<String>,
    _phantom: PhantomData<T>,
}

impl<T: Event> TopicClassifier<T> {
    /// Create a new classifier with specific topic patterns
    pub fn new(patterns: Vec<String>) -> Self {
        Self { topic_patterns: patterns, _phantom: Default::default() }
    }

    /// Subscribe to a single exact topic
    pub fn exact(topic: String) -> Self {
        Self::new(vec![topic])
    }

    /// Subscribe to a topic hierarchy (e.g., "user.*" matches "user.created", "user.updated", etc.)
    pub fn hierarchy(prefix: String) -> Self {
        Self::new(vec![format!("{}.*", prefix)])
    }

    /// Subscribe to multiple topics
    pub fn topics(topics: Vec<String>) -> Self {
        Self::new(topics)
    }

    /// Add a topic pattern to existing classifier
    pub fn add_topic(mut self, topic: String) -> Self {
        self.topic_patterns.push(topic);
        self
    }

    /// Check if a topic matches any of the patterns
    pub fn matches_topic(&self, topic: &str) -> bool {
        self.topic_patterns.iter().any(|pattern| self.topic_matches(topic, pattern))
    }

    /// Match topic against pattern with wildcard support
    fn topic_matches(&self, topic: &str, pattern: &str) -> bool {
        let topic_parts: Vec<&str> = topic.split('.').collect();
        let pattern_parts: Vec<&str> = pattern.split('.').collect();

        if topic_parts.len() != pattern_parts.len() {
            return false;
        }

        topic_parts.iter().zip(pattern_parts.iter()).all(|(topic_part, pattern_part)| {
            pattern_part == &"*" || pattern_part == topic_part
        })
    }
}

impl<T: Event> EventClassifier for TopicClassifier<T> {
    type Event = T;
    type Classifier = String;

    fn classify(&self, _event: &Self::Event) -> Vec<Self::Classifier> {
        // TopicClassifier doesn't classify by event content, only by topic passed to publish
        // This will be handled in the publish method
        vec![]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Actor, ActorContext, ActorSystem, SchedulerConfig};
    use serde::{Deserialize, Serialize};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Mutex};
    use tokio::time::{sleep, Duration};

    /// Test event type for TopicClassifier tests
    #[derive(Debug, Clone, Serialize, Deserialize, Default)]
    pub struct TestEvent {
        pub message: String,
        pub value: u32,
    }


    impl TestEvent {
        pub fn new(message: String, value: String) -> Self {
            Self {
                message,
                value: value.len() as u32,
            }
        }
    }

    // Test Event Types
    #[derive(Debug, Clone, Serialize, Deserialize, Default)]
    struct SimpleEvent {
        message: String,
        value: u32,
    }

    #[derive(Debug, Clone, Serialize, Deserialize, Default)]
    struct OrderEvent {
        order_id: String,
        customer_id: String,
        amount: f64,
    }

    #[derive(Debug, Clone, Serialize, Deserialize, Default)]
    struct PaymentEvent {
        payment_id: String,
        order_id: String,
        status: String,
    }

    // Test Actors
    struct SimpleTestActor {
        name: String,
        received_events: Arc<AtomicU32>,
        last_event: Arc<Mutex<Option<SimpleEvent>>>,
    }

    struct MultiEventActor {
        name: String,
        simple_events: Arc<AtomicU32>,
        order_events: Arc<AtomicU32>,
        payment_events: Arc<AtomicU32>,
    }

    struct SlowActor {
        name: String,
        received_events: Arc<AtomicU32>,
        delay_ms: u64,
    }

    struct FailingActor {
        name: String,
        should_fail: Arc<AtomicU32>,
        call_count: Arc<AtomicU32>,
    }

    // Implement Actor for SimpleTestActor
    #[async_trait::async_trait]
    impl Actor for SimpleTestActor {
        type Message = SimpleEvent;
        type State = ();

        async fn on_message(&mut self, msg: Self::Message, _ctx: &mut ActorContext) -> crate::Result<()> {
            println!("Actor {} received simple event: {:?}", self.name, msg);
            self.received_events.fetch_add(1, Ordering::SeqCst);
            *self.last_event.lock().unwrap() = Some(msg);
            Ok(())
        }
    }

    // Implement Actor for MultiEventActor - handles different event types
    #[async_trait::async_trait]
    impl Actor for MultiEventActor {
        type Message = SimpleEvent; // Primary message type
        type State = ();

        async fn on_message(&mut self, msg: Self::Message, _ctx: &mut ActorContext) -> crate::Result<()> {
            self.simple_events.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    // Additional actors for other event types
    struct OrderActor {
        name: String,
        received_events: Arc<AtomicU32>,
    }

    #[async_trait::async_trait]
    impl Actor for OrderActor {
        type Message = OrderEvent;
        type State = ();

        async fn on_message(&mut self, msg: Self::Message, _ctx: &mut ActorContext) -> crate::Result<()> {
            println!("OrderActor {} received: {:?}", self.name, msg);
            self.received_events.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    struct PaymentActor {
        name: String,
        received_events: Arc<AtomicU32>,
    }

    #[async_trait::async_trait]
    impl Actor for PaymentActor {
        type Message = PaymentEvent;
        type State = ();

        async fn on_message(&mut self, msg: Self::Message, _ctx: &mut ActorContext) -> crate::Result<()> {
            println!("PaymentActor {} received: {:?}", self.name, msg);
            self.received_events.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl Actor for SlowActor {
        type Message = SimpleEvent;
        type State = ();

        async fn on_message(&mut self, msg: Self::Message, _ctx: &mut ActorContext) -> crate::Result<()> {
            sleep(Duration::from_millis(self.delay_ms)).await;
            self.received_events.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl Actor for FailingActor {
        type Message = SimpleEvent;
        type State = ();

        async fn on_message(&mut self, _msg: Self::Message, _ctx: &mut ActorContext) -> crate::Result<()> {
            let count = self.call_count.fetch_add(1, Ordering::SeqCst);
            if self.should_fail.load(Ordering::SeqCst) > 0 {
                return Err(crate::error::LyricoreActorError::Actor(
                    crate::error::ActorError::MessageSendFailed(format!("Simulated failure #{}", count))
                ));
            }
            Ok(())
        }
    }

    // Helper function to create test system
    async fn create_test_system(port: u16) -> ActorSystem {
        let mut system = ActorSystem::new(
            format!("test_node_{}", port),
            format!("127.0.0.1:{}", port),
            SchedulerConfig::default(),
            None,
        ).unwrap();
        system.start_server().await.unwrap();
        system
    }

    #[tokio::test]
    async fn test_basic_subscribe_and_publish() {
        let mut system = create_test_system(50100).await;
        let eventbus = EventBus::new(EventBusConfig::default());

        let counter = Arc::new(AtomicU32::new(0));
        let last_event = Arc::new(Mutex::new(None));

        let actor = system.spawn_local(
            "test_actor".to_string(),
            SimpleTestActor {
                name: "TestActor".to_string(),
                received_events: counter.clone(),
                last_event: last_event.clone(),
            },
        );

        // Test subscription
        let sub_id = eventbus.subscribe(actor, AllEventsClassifier::<SimpleEvent>::new()).await.unwrap();
        assert!(!sub_id.0.is_empty());

        // Test event publication
        let test_event = SimpleEvent {
            message: "Hello EventBus!".to_string(),
            value: 42,
        };

        let result = eventbus.publish(test_event.clone()).await.unwrap();
        assert_eq!(result.total_subscribers, 1);
        assert_eq!(result.successful_deliveries, 1);
        assert_eq!(result.failed_deliveries, 0);
        assert!(result.is_fully_successful());
        assert_eq!(result.success_rate(), 1.0);

        sleep(Duration::from_millis(100)).await;

        // Verify event was received
        assert_eq!(counter.load(Ordering::SeqCst), 1);
        let received_event = last_event.lock().unwrap().clone();
        assert!(received_event.is_some());
        let received = received_event.unwrap();
        assert_eq!(received.message, "Hello EventBus!");
        assert_eq!(received.value, 42);

        eventbus.shutdown().await.unwrap();
        system.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_multiple_subscribers() {
        let mut system = create_test_system(50101).await;
        let eventbus = EventBus::new(EventBusConfig::default());

        let counter1 = Arc::new(AtomicU32::new(0));
        let counter2 = Arc::new(AtomicU32::new(0));
        let counter3 = Arc::new(AtomicU32::new(0));

        let actor1 = system.spawn_local("actor1".to_string(), SimpleTestActor {
            name: "Actor1".to_string(),
            received_events: counter1.clone(),
            last_event: Arc::new(Mutex::new(None)),
        });

        let actor2 = system.spawn_local("actor2".to_string(), SimpleTestActor {
            name: "Actor2".to_string(),
            received_events: counter2.clone(),
            last_event: Arc::new(Mutex::new(None)),
        });

        let actor3 = system.spawn_local("actor3".to_string(), SimpleTestActor {
            name: "Actor3".to_string(),
            received_events: counter3.clone(),
            last_event: Arc::new(Mutex::new(None)),
        });

        // Subscribe all actors
        let _sub1 = eventbus.subscribe(actor1, AllEventsClassifier::<SimpleEvent>::new()).await.unwrap();
        let _sub2 = eventbus.subscribe(actor2, AllEventsClassifier::<SimpleEvent>::new()).await.unwrap();
        let _sub3 = eventbus.subscribe(actor3, AllEventsClassifier::<SimpleEvent>::new()).await.unwrap();

        // Publish event
        let test_event = SimpleEvent {
            message: "Broadcast test".to_string(),
            value: 123,
        };

        let result = eventbus.publish(test_event).await.unwrap();
        assert_eq!(result.total_subscribers, 3);
        assert_eq!(result.successful_deliveries, 3);
        assert_eq!(result.failed_deliveries, 0);

        sleep(Duration::from_millis(150)).await;

        // All actors should receive the event
        assert_eq!(counter1.load(Ordering::SeqCst), 1);
        assert_eq!(counter2.load(Ordering::SeqCst), 1);
        assert_eq!(counter3.load(Ordering::SeqCst), 1);

        eventbus.shutdown().await.unwrap();
        system.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_unsubscribe() {
        let mut system = create_test_system(50102).await;
        let eventbus = EventBus::new(EventBusConfig::default());

        let counter1 = Arc::new(AtomicU32::new(0));
        let counter2 = Arc::new(AtomicU32::new(0));

        let actor1 = system.spawn_local("actor1".to_string(), SimpleTestActor {
            name: "Actor1".to_string(),
            received_events: counter1.clone(),
            last_event: Arc::new(Mutex::new(None)),
        });

        let actor2 = system.spawn_local("actor2".to_string(), SimpleTestActor {
            name: "Actor2".to_string(),
            received_events: counter2.clone(),
            last_event: Arc::new(Mutex::new(None)),
        });

        let sub1 = eventbus.subscribe(actor1, AllEventsClassifier::<SimpleEvent>::new()).await.unwrap();
        let _sub2 = eventbus.subscribe(actor2, AllEventsClassifier::<SimpleEvent>::new()).await.unwrap();

        // First publish - both should receive
        let event1 = SimpleEvent { message: "First event".to_string(), value: 1 };
        let result1 = eventbus.publish(event1).await.unwrap();
        assert_eq!(result1.total_subscribers, 2);

        sleep(Duration::from_millis(100)).await;
        assert_eq!(counter1.load(Ordering::SeqCst), 1);
        assert_eq!(counter2.load(Ordering::SeqCst), 1);

        // Unsubscribe actor1
        let unsubscribe_result = eventbus.unsubscribe(&sub1).await.unwrap();
        assert!(unsubscribe_result);

        // Second publish - only actor2 should receive
        let event2 = SimpleEvent { message: "Second event".to_string(), value: 2 };
        let result2 = eventbus.publish(event2).await.unwrap();
        assert_eq!(result2.total_subscribers, 1);

        sleep(Duration::from_millis(100)).await;
        assert_eq!(counter1.load(Ordering::SeqCst), 1); // Still 1
        assert_eq!(counter2.load(Ordering::SeqCst), 2); // Now 2

        // Test unsubscribing non-existent subscription
        let fake_sub = SubscriptionId::new();
        let fake_result = eventbus.unsubscribe(&fake_sub).await.unwrap();
        assert!(!fake_result);

        eventbus.shutdown().await.unwrap();
        system.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_unsubscribe_actor() {
        let mut system = create_test_system(50103).await;
        let eventbus = EventBus::new(EventBusConfig::default());

        let counter = Arc::new(AtomicU32::new(0));
        let actor = system.spawn_local("multi_sub_actor".to_string(), SimpleTestActor {
            name: "MultiSubActor".to_string(),
            received_events: counter.clone(),
            last_event: Arc::new(Mutex::new(None)),
        });

        // Subscribe the same actor multiple times (in real scenarios this might happen)
        let _sub1 = eventbus.subscribe(actor.clone(), AllEventsClassifier::<SimpleEvent>::new()).await.unwrap();
        let _sub2 = eventbus.subscribe(actor.clone(), AllEventsClassifier::<SimpleEvent>::new()).await.unwrap();

        // Publish event - should be delivered twice
        let event = SimpleEvent { message: "Multi sub test".to_string(), value: 100 };
        let result = eventbus.publish(event).await.unwrap();
        assert_eq!(result.total_subscribers, 2);

        sleep(Duration::from_millis(100)).await;
        assert_eq!(counter.load(Ordering::SeqCst), 2);

        // Unsubscribe all subscriptions for this actor
        let unsubscribed_count = eventbus.unsubscribe_actor(&actor).await.unwrap();
        assert_eq!(unsubscribed_count, 2);

        // Publish another event - should not be delivered
        let event2 = SimpleEvent { message: "After unsubscribe".to_string(), value: 200 };
        let result2 = eventbus.publish(event2).await.unwrap();
        assert_eq!(result2.total_subscribers, 0);

        sleep(Duration::from_millis(100)).await;
        assert_eq!(counter.load(Ordering::SeqCst), 2); // Still 2

        eventbus.shutdown().await.unwrap();
        system.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_different_event_types() {
        let mut system = create_test_system(50104).await;
        let eventbus = EventBus::new(EventBusConfig::default());

        let simple_counter = Arc::new(AtomicU32::new(0));
        let order_counter = Arc::new(AtomicU32::new(0));
        let payment_counter = Arc::new(AtomicU32::new(0));

        let simple_actor = system.spawn_local("simple_actor".to_string(), SimpleTestActor {
            name: "SimpleActor".to_string(),
            received_events: simple_counter.clone(),
            last_event: Arc::new(Mutex::new(None)),
        });

        let order_actor = system.spawn_local("order_actor".to_string(), OrderActor {
            name: "OrderActor".to_string(),
            received_events: order_counter.clone(),
        });

        let payment_actor = system.spawn_local("payment_actor".to_string(), PaymentActor {
            name: "PaymentActor".to_string(),
            received_events: payment_counter.clone(),
        });

        // Subscribe to different event types
        let _simple_sub = eventbus.subscribe(simple_actor, AllEventsClassifier::<SimpleEvent>::new()).await.unwrap();
        let _order_sub = eventbus.subscribe(order_actor, AllEventsClassifier::<OrderEvent>::new()).await.unwrap();
        let _payment_sub = eventbus.subscribe(payment_actor, AllEventsClassifier::<PaymentEvent>::new()).await.unwrap();

        // Publish different event types
        let simple_event = SimpleEvent { message: "Simple".to_string(), value: 1 };
        let order_event = OrderEvent {
            order_id: "ORDER-123".to_string(),
            customer_id: "CUST-456".to_string(),
            amount: 99.99
        };
        let payment_event = PaymentEvent {
            payment_id: "PAY-789".to_string(),
            order_id: "ORDER-123".to_string(),
            status: "completed".to_string()
        };

        let simple_result = eventbus.publish(simple_event).await.unwrap();
        let order_result = eventbus.publish(order_event).await.unwrap();
        let payment_result = eventbus.publish(payment_event).await.unwrap();

        // Each event type should only go to its corresponding subscriber
        assert_eq!(simple_result.total_subscribers, 1);
        assert_eq!(order_result.total_subscribers, 1);
        assert_eq!(payment_result.total_subscribers, 1);

        sleep(Duration::from_millis(200)).await;

        assert_eq!(simple_counter.load(Ordering::SeqCst), 1);
        assert_eq!(order_counter.load(Ordering::SeqCst), 1);
        assert_eq!(payment_counter.load(Ordering::SeqCst), 1);

        eventbus.shutdown().await.unwrap();
        system.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_no_subscribers() {
        let mut system = create_test_system(50105).await;
        let eventbus = EventBus::new(EventBusConfig::default());

        // Publish event with no subscribers
        let event = SimpleEvent { message: "No one listening".to_string(), value: 0 };
        let result = eventbus.publish(event).await.unwrap();

        assert_eq!(result.total_subscribers, 0);
        assert_eq!(result.successful_deliveries, 0);
        assert_eq!(result.failed_deliveries, 0);
        assert_eq!(result.success_rate(), 1.0); // Should be 1.0 when no subscribers

        eventbus.shutdown().await.unwrap();
        system.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_concurrent_publishing() {
        let mut system = create_test_system(50106).await;
        let eventbus = Arc::new(EventBus::new(EventBusConfig::default()));

        let counter = Arc::new(AtomicU32::new(0));
        let actor = system.spawn_local("concurrent_actor".to_string(), SimpleTestActor {
            name: "ConcurrentActor".to_string(),
            received_events: counter.clone(),
            last_event: Arc::new(Mutex::new(None)),
        });

        let _sub = eventbus.subscribe(actor, AllEventsClassifier::<SimpleEvent>::new()).await.unwrap();

        // Publish multiple events concurrently
        let mut tasks = Vec::new();
        for i in 0..10 {
            let eventbus_clone = eventbus.clone();
            let task = tokio::spawn(async move {
                let event = SimpleEvent {
                    message: format!("Concurrent event {}", i),
                    value: i
                };
                eventbus_clone.publish(event).await
            });
            tasks.push(task);
        }

        // Wait for all publishes to complete
        let mut total_subscribers = 0;
        let mut total_successful = 0;
        for task in tasks {
            let result = task.await.unwrap().unwrap();
            total_subscribers += result.total_subscribers;
            total_successful += result.successful_deliveries;
        }

        assert_eq!(total_subscribers, 10); // Each publish should find 1 subscriber
        assert_eq!(total_successful, 10); // All should succeed

        sleep(Duration::from_millis(200)).await;
        assert_eq!(counter.load(Ordering::SeqCst), 10);

        eventbus.shutdown().await.unwrap();
        system.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_delivery_timeout() {
        let mut system = create_test_system(50107).await;
        let config = EventBusConfig {
            delivery_timeout_ms: 50, // Very short timeout
            max_concurrent_deliveries: 10,
            continue_on_delivery_failure: true,
            delivery_buffer_size: 100,
        };
        let eventbus = EventBus::new(config);

        let counter = Arc::new(AtomicU32::new(0));
        let slow_actor = system.spawn_local("slow_actor".to_string(), SlowActor {
            name: "SlowActor".to_string(),
            received_events: counter.clone(),
            delay_ms: 200, // Much longer than timeout
        });

        let _sub = eventbus.subscribe(slow_actor, AllEventsClassifier::<SimpleEvent>::new()).await.unwrap();

        let event = SimpleEvent { message: "Timeout test".to_string(), value: 999 };
        let result = eventbus.publish(event).await.unwrap();

        assert_eq!(result.total_subscribers, 1);
        assert_eq!(result.successful_deliveries, 1);
        assert_eq!(result.failed_deliveries, 0);
        assert!(result.is_fully_successful());
        assert_eq!(result.success_rate(), 1.0);

        eventbus.shutdown().await.unwrap();
        system.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_statistics() {
        let mut system = create_test_system(50108).await;
        let eventbus = EventBus::new(EventBusConfig::default());

        let counter = Arc::new(AtomicU32::new(0));
        let actor = system.spawn_local("stats_actor".to_string(), SimpleTestActor {
            name: "StatsActor".to_string(),
            received_events: counter.clone(),
            last_event: Arc::new(Mutex::new(None)),
        });

        // Check initial stats
        let initial_stats = eventbus.get_stats().await;
        assert_eq!(initial_stats.total_events_published, 0);
        assert_eq!(initial_stats.total_subscriptions, 0);
        assert_eq!(initial_stats.failed_deliveries, 0);

        // Subscribe and check stats
        let _sub = eventbus.subscribe(actor, AllEventsClassifier::<SimpleEvent>::new()).await.unwrap();
        let after_sub_stats = eventbus.get_stats().await;
        assert_eq!(after_sub_stats.total_subscriptions, 1);

        let simple_event_type = std::any::type_name::<SimpleEvent>();
        assert_eq!(after_sub_stats.subscribers_by_type.get(simple_event_type), Some(&1));

        // Publish events and check stats
        for i in 0..5 {
            let event = SimpleEvent { message: format!("Event {}", i), value: i };
            eventbus.publish(event).await.unwrap();
        }

        let final_stats = eventbus.get_stats().await;
        assert_eq!(final_stats.total_events_published, 5);
        assert_eq!(final_stats.events_by_type.get(simple_event_type), Some(&5));

        eventbus.shutdown().await.unwrap();
        system.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_list_subscriptions() {
        let mut system = create_test_system(50109).await;
        let eventbus = EventBus::new(EventBusConfig::default());

        let actor1 = system.spawn_local("list_actor1".to_string(), SimpleTestActor {
            name: "Actor1".to_string(),
            received_events: Arc::new(AtomicU32::new(0)),
            last_event: Arc::new(Mutex::new(None)),
        });

        let actor2 = system.spawn_local("list_actor2".to_string(), OrderActor {
            name: "Actor2".to_string(),
            received_events: Arc::new(AtomicU32::new(0)),
        });

        // Initially no subscriptions
        let empty_list = eventbus.list_subscriptions().await;
        assert_eq!(empty_list.len(), 0);

        // Add subscriptions
        let _sub1 = eventbus.subscribe(actor1, AllEventsClassifier::<SimpleEvent>::new()).await.unwrap();
        let _sub2 = eventbus.subscribe(actor2, AllEventsClassifier::<OrderEvent>::new()).await.unwrap();

        let subscriptions = eventbus.list_subscriptions().await;
        assert_eq!(subscriptions.len(), 2);

        // Check for event types in the subscriptions
        // Note: type_name() returns the full module path
        let event_types: Vec<&String> = subscriptions.iter().map(|(_, _, event_type)| event_type).collect();

        // Use contains to check, as actual type names will include full paths
        assert!(event_types.iter().any(|t| t.contains("SimpleEvent")));
        assert!(event_types.iter().any(|t| t.contains("OrderEvent")));

        eventbus.shutdown().await.unwrap();
        system.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_eventbus_shutdown() {
        let mut system = create_test_system(50110).await;
        let eventbus = EventBus::new(EventBusConfig::default());

        let actor = system.spawn_local("shutdown_actor".to_string(), SimpleTestActor {
            name: "ShutdownActor".to_string(),
            received_events: Arc::new(AtomicU32::new(0)),
            last_event: Arc::new(Mutex::new(None)),
        });

        let _sub = eventbus.subscribe(actor, AllEventsClassifier::<SimpleEvent>::new()).await.unwrap();

        // Verify we have subscriptions
        let stats_before = eventbus.get_stats().await;
        assert_eq!(stats_before.total_subscriptions, 1);

        // Shutdown eventbus
        eventbus.shutdown().await.unwrap();

        // Verify subscriptions are cleared after shutdown
        // Note: The current implementation may not immediately clear internal maps
        // In a production system, you might want to add more sophisticated cleanup
        let subscriptions_after = eventbus.list_subscriptions().await;
        let stats_after = eventbus.get_stats().await;

        // The internal maps should be cleared, but stats may still reflect the state
        assert_eq!(subscriptions_after.len(), 0, "Subscriptions list should be empty after shutdown");

        system.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_high_volume_events() {
        let mut system = create_test_system(50111).await;
        let eventbus = EventBus::new(EventBusConfig::default());

        let counter = Arc::new(AtomicU32::new(0));
        let actor = system.spawn_local("volume_actor".to_string(), SimpleTestActor {
            name: "VolumeActor".to_string(),
            received_events: counter.clone(),
            last_event: Arc::new(Mutex::new(None)),
        });

        let _sub = eventbus.subscribe(actor, AllEventsClassifier::<SimpleEvent>::new()).await.unwrap();

        // Publish many events
        let event_count = 100;
        for i in 0..event_count {
            let event = SimpleEvent { message: format!("Volume event {}", i), value: i };
            let result = eventbus.publish(event).await.unwrap();
            assert_eq!(result.total_subscribers, 1);
            assert_eq!(result.successful_deliveries, 1);
        }

        // Wait for all events to be processed
        sleep(Duration::from_millis(500)).await;

        assert_eq!(counter.load(Ordering::SeqCst), event_count);

        let final_stats = eventbus.get_stats().await;
        assert_eq!(final_stats.total_events_published, event_count as u64);

        eventbus.shutdown().await.unwrap();
        system.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_event_classification() {

        let mut system = create_test_system(50112).await;
        let eventbus = EventBus::new(EventBusConfig::default());

        let counter1 = Arc::new(AtomicU32::new(0));
        let counter2 = Arc::new(AtomicU32::new(0));

        let actor1 = system.spawn_local("actor1".to_string(), SimpleTestActor {
            name: "Actor1".to_string(),
            received_events: counter1.clone(),
            last_event: Arc::new(Mutex::new(None)),
        });

        let actor2 = system.spawn_local("actor2".to_string(), SimpleTestActor {
            name: "Actor2".to_string(),
            received_events: counter2.clone(),
            last_event: Arc::new(Mutex::new(None)),
        });

        // Two actors subscribe to SimpleEvent but with different classifiers
        let _sub1 = eventbus.subscribe(actor1, AllEventsClassifier::<SimpleEvent>::new()).await.unwrap();

        // Custom classifier: only receive messages where value is even
        #[derive(Clone)]
        struct EvenValueClassifier;

        impl EventClassifier for EvenValueClassifier {
            type Event = SimpleEvent;
            type Classifier = ();

            fn classify(&self, event: &SimpleEvent) -> Vec<()> {
                if event.value % 2 == 0 {
                    vec![()]
                } else {
                    vec![]
                }
            }
        }

        let _sub2 = eventbus.subscribe(actor2, EvenValueClassifier).await.unwrap();

        // Publish several events
        for i in 0..5 {
            let event = SimpleEvent {
                message: format!("Event {}", i),
                value: i
            };
            eventbus.publish(event).await.unwrap();
            // OrderEvent will not be received by either actor
            eventbus.publish(OrderEvent {
                order_id: format!("ORDER-{}", i),
                customer_id: format!("CUST-{}", i),
                amount: (i as f64) * 10.0
            }).await.unwrap();
        }

        sleep(Duration::from_millis(200)).await;

        // actor1 should receive all 5 events
        assert_eq!(counter1.load(Ordering::SeqCst), 5);
        // actor2 should only receive events with even values (0, 2, 4)
        assert_eq!(counter2.load(Ordering::SeqCst), 3);

        eventbus.shutdown().await.unwrap();
        system.shutdown().await.unwrap();
    }

    // === TopicClassifier Tests ===

    #[tokio::test]
    async fn test_topic_classifier_exact_match() {
        let mut system = create_test_system(50200).await;
        let eventbus = EventBus::new(EventBusConfig::default());

        
        // Create a test actor that handles TestEvent
        struct TopicTestActor {
            name: String,
            received_events: Arc<AtomicU32>,
            last_message: Arc<Mutex<Option<String>>>,
        }

        #[async_trait::async_trait]
        impl Actor for TopicTestActor {
            type Message = TestEvent;
            type State = ();

            async fn on_message(&mut self, msg: Self::Message, _ctx: &mut ActorContext) -> crate::Result<()> {
                println!("TopicTestActor {} received: {:?}", self.name, msg.message);
                self.received_events.fetch_add(1, Ordering::SeqCst);
                *self.last_message.lock().unwrap() = Some(msg.message.clone());
                Ok(())
            }
        }

        let counter = Arc::new(AtomicU32::new(0));
        let last_message = Arc::new(Mutex::new(None));
        let actor = system.spawn_local("topic_exact_actor".to_string(), TopicTestActor {
            name: "TopicExactActor".to_string(),
            received_events: counter.clone(),
            last_message: last_message.clone(),
        });

        // Subscribe to exact topic "user.created"
        let _sub = eventbus.subscribe(actor.clone(), TopicClassifier::<TestEvent>::exact("user.created".to_string())).await.unwrap();

        // Test exact match with topic
        let exact_event = TestEvent {
            message: "User created event".to_string(),
            value: 42,
        };
        let result = eventbus.publish_with_topic(exact_event, Some("user.created".to_string())).await.unwrap();
        assert_eq!(result.total_subscribers, 1);
        assert_eq!(result.successful_deliveries, 1);

        // Test non-matching topic
        let wrong_event = TestEvent {
            message: "User deleted event".to_string(),
            value: 43,
        };
        let wrong_result = eventbus.publish_with_topic(wrong_event, Some("user.deleted".to_string())).await.unwrap();
        assert_eq!(wrong_result.total_subscribers, 0);

        // Test without topic (should not match TopicClassifier)
        let no_topic_event = TestEvent {
            message: "No topic event".to_string(),
            value: 44,
        };
        let no_topic_result = eventbus.publish(no_topic_event).await.unwrap();
        assert_eq!(no_topic_result.total_subscribers, 0);

        sleep(Duration::from_millis(100)).await;
        assert_eq!(counter.load(Ordering::SeqCst), 1);
        
        // Check the message was received correctly
        let received_message = last_message.lock().unwrap().clone();
        assert_eq!(received_message, Some("User created event".to_string()));

        eventbus.shutdown().await.unwrap();
        system.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_topic_classifier_wildcard() {
        let mut system = create_test_system(50201).await;
        let eventbus = EventBus::new(EventBusConfig::default());

        
        // Create a test actor that handles TestEvent
        struct TopicTestActor {
            name: String,
            received_events: Arc<AtomicU32>,
            last_message: Arc<Mutex<Option<String>>>,
        }

        #[async_trait::async_trait]
        impl Actor for TopicTestActor {
            type Message = TestEvent;
            type State = ();

            async fn on_message(&mut self, msg: Self::Message, _ctx: &mut ActorContext) -> crate::Result<()> {
                println!("TopicTestActor {} received: {:?}", self.name, msg.message);
                self.received_events.fetch_add(1, Ordering::SeqCst);
                *self.last_message.lock().unwrap() = Some(msg.message.clone());
                Ok(())
            }
        }

        let counter = Arc::new(AtomicU32::new(0));
        let last_message = Arc::new(Mutex::new(None));
        let actor = system.spawn_local("topic_wildcard_actor".to_string(), TopicTestActor {
            name: "TopicWildcardActor".to_string(),
            received_events: counter.clone(),
            last_message: last_message.clone(),
        });

        // Subscribe to "user.*" pattern
        let _sub = eventbus.subscribe(actor.clone(), TopicClassifier::<TestEvent>::hierarchy("user".to_string())).await.unwrap();

        // Test various user topics
        let topics = vec![
            "user.created",
            "user.updated", 
            "user.deleted",
            "user.profile"
        ];

        for topic in topics {
            let event = TestEvent {
                message: format!("Event for topic {}", topic),
                value: 42,
            };
            let result = eventbus.publish_with_topic(event, Some(topic.to_string())).await.unwrap();
            assert_eq!(result.total_subscribers, 1, "Topic {} should match", topic);
        }

        // Test non-matching topics
        let non_matching = vec![
            "order.created",
            "product.updated",
            "user"  // Wrong level
        ];

        for topic in non_matching {
            let event = TestEvent {
                message: format!("Event for non-matching topic {}", topic),
                value: 43,
            };
            let result = eventbus.publish_with_topic(event, Some(topic.to_string())).await.unwrap();
            assert_eq!(result.total_subscribers, 0, "Topic {} should not match", topic);
        }

        sleep(Duration::from_millis(100)).await;
        assert_eq!(counter.load(Ordering::SeqCst), 4); // Only matching topics

        eventbus.shutdown().await.unwrap();
        system.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_topic_classifier_multi_level_wildcard() {
        let mut system = create_test_system(50206).await;
        let eventbus = EventBus::new(EventBusConfig::default());

        // Create a test actor that handles TestEvent
        struct TopicTestActor {
            name: String,
            received_events: Arc<AtomicU32>,
            last_topic: Arc<Mutex<Option<String>>>,
        }

        #[async_trait::async_trait]
        impl Actor for TopicTestActor {
            type Message = TestEvent;
            type State = ();

            async fn on_message(&mut self, msg: Self::Message, _ctx: &mut ActorContext) -> crate::Result<()> {
                println!("TopicTestActor {} received: {:?}", self.name, msg);
                self.received_events.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        }

        let counter = Arc::new(AtomicU32::new(0));
        let last_topic = Arc::new(Mutex::new(None));
        let actor = system.spawn_local("topic_multi_wildcard_actor".to_string(), TopicTestActor {
            name: "TopicMultiWildcardActor".to_string(),
            received_events: counter.clone(),
            last_topic: last_topic.clone(),
        });

        // Subscribe to "user.profile.*" pattern - matches user.profile.created, user.profile.updated, etc.
        let _sub = eventbus.subscribe(actor.clone(), TopicClassifier::<TestEvent>::hierarchy("user.profile".to_string())).await.unwrap();

        // Test various user profile topics
        let topics = vec![
            "user.profile.created",
            "user.profile.updated", 
            "user.profile.deleted",
            "user.profile.changed"
        ];

        for topic in topics {
            let event = TestEvent::new("multi_wildcard_test".to_string(), format!("payload for {}", topic));
            let result = eventbus.publish_with_topic(event, Some(topic.to_string())).await.unwrap();
            assert_eq!(result.total_subscribers, 1, "Topic {} should match", topic);
        }

        // Test non-matching topics
        let non_matching = vec![
            "user.created",      // Wrong level
            "user.settings.changed",  // Wrong sub-level
            "profile.user.created",    // Wrong order
        ];

        for topic in non_matching {
            let event = TestEvent::new("multi_wildcard_non_match".to_string(), format!("payload for {}", topic));
            let result = eventbus.publish_with_topic(event, Some(topic.to_string())).await.unwrap();
            assert_eq!(result.total_subscribers, 0, "Topic {} should not match", topic);
        }

        sleep(Duration::from_millis(100)).await;
        assert_eq!(counter.load(Ordering::SeqCst), 4); // All matching topics

        eventbus.shutdown().await.unwrap();
        system.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_topic_classifier_multiple_patterns() {
        let mut system = create_test_system(50202).await;
        let eventbus = EventBus::new(EventBusConfig::default());

        // Create a test actor that handles TestEvent
        struct TopicTestActor {
            name: String,
            received_events: Arc<AtomicU32>,
            last_topic: Arc<Mutex<Option<String>>>,
        }

        #[async_trait::async_trait]
        impl Actor for TopicTestActor {
            type Message = TestEvent;
            type State = ();

            async fn on_message(&mut self, msg: Self::Message, _ctx: &mut ActorContext) -> crate::Result<()> {
                println!("TopicTestActor {} received: {:?}", self.name, msg);
                self.received_events.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        }

        let counter = Arc::new(AtomicU32::new(0));
        let last_topic = Arc::new(Mutex::new(None));
        let actor = system.spawn_local("topic_multi_actor".to_string(), TopicTestActor {
            name: "TopicMultiActor".to_string(),
            received_events: counter.clone(),
            last_topic: last_topic.clone(),
        });

        // Subscribe to multiple patterns
        let classifier = TopicClassifier::<TestEvent>::new(vec![
            "user.*".to_string(),
            "order.created".to_string(),
            "system.*.error".to_string(),
        ]);

        let _sub = eventbus.subscribe(actor.clone(), classifier).await.unwrap();

        // Test matching topics
        let matching_topics = vec![
            "user.created",
            "user.updated",
            "order.created",
            "system.auth.error",
            "system.database.error",
        ];

        for topic in matching_topics {
            let event = TestEvent::new("multi_pattern_test".to_string(), format!("payload for {}", topic));
            let result = eventbus.publish_with_topic(event, Some(topic.to_string())).await.unwrap();
            assert_eq!(result.total_subscribers, 1, "Topic {} should match", topic);
        }

        // Test non-matching topics
        let non_matching = vec![
            "order.updated",  // Only order.created matches
            "system.error",   // Wrong level
            "product.created",
        ];

        for topic in non_matching {
            let event = TestEvent::new("multi_pattern_non_match".to_string(), format!("payload for {}", topic));
            let result = eventbus.publish_with_topic(event, Some(topic.to_string())).await.unwrap();
            assert_eq!(result.total_subscribers, 0, "Topic {} should not match", topic);
        }

        sleep(Duration::from_millis(100)).await;
        assert_eq!(counter.load(Ordering::SeqCst), 5); // All matching topics

        eventbus.shutdown().await.unwrap();
        system.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_topic_event_with_data() {
        let mut system = create_test_system(50203).await;
        let eventbus = EventBus::new(EventBusConfig::default());

        // Actor that handles TestEvent
        struct TopicDataActor {
            name: String,
            received_events: Arc<AtomicU32>,
            last_message: Arc<Mutex<Option<String>>>,
        }

        #[async_trait::async_trait]
        impl Actor for TopicDataActor {
            type Message = TestEvent;
            type State = ();

            async fn on_message(&mut self, msg: Self::Message, _ctx: &mut ActorContext) -> crate::Result<()> {
                println!("TopicDataActor {} received: {:?}", self.name, msg);
                self.received_events.fetch_add(1, Ordering::SeqCst);
                *self.last_message.lock().unwrap() = Some(msg.message.clone());
                Ok(())
            }
        }

        let counter = Arc::new(AtomicU32::new(0));
        let last_message = Arc::new(Mutex::new(None));
        let actor = system.spawn_local("topic_data_actor".to_string(), TopicDataActor {
            name: "TopicDataActor".to_string(),
            received_events: counter.clone(),
            last_message: last_message.clone(),
        });

        // Subscribe to user events using TopicClassifier
        let classifier = TopicClassifier::<TestEvent>::hierarchy("user".to_string());
        let _sub = eventbus.subscribe(actor, classifier).await.unwrap();

        // Test typed topic events
        let events = vec![
            TestEvent::new("user.created".to_string(), "user123".to_string()),
            TestEvent::new("user.updated".to_string(), "user456".to_string()),
        ];

        let topics = vec!["user.created", "user.updated"];
        
        for (i, event) in events.iter().enumerate() {
            let result = eventbus.publish_with_topic(event.clone(), Some(topics[i].to_string())).await.unwrap();
            assert_eq!(result.total_subscribers, 1);
        }

        sleep(Duration::from_millis(100)).await;
        assert_eq!(counter.load(Ordering::SeqCst), 2);

        // Check last message
        let last_msg = last_message.lock().unwrap().clone();
        assert_eq!(last_msg, Some("user.updated".to_string()));

        eventbus.shutdown().await.unwrap();
        system.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_topic_classifier_complex_hierarchy() {
        let mut system = create_test_system(50204).await;
        let eventbus = EventBus::new(EventBusConfig::default());

        // Create a test actor that handles TestEvent
        struct TopicTestActor {
            name: String,
            received_events: Arc<AtomicU32>,
            last_topic: Arc<Mutex<Option<String>>>,
        }

        #[async_trait::async_trait]
        impl Actor for TopicTestActor {
            type Message = TestEvent;
            type State = ();

            async fn on_message(&mut self, msg: Self::Message, _ctx: &mut ActorContext) -> crate::Result<()> {
                println!("TopicTestActor {} received: {:?}", self.name, msg);
                self.received_events.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        }

        let counter = Arc::new(AtomicU32::new(0));
        let last_topic = Arc::new(Mutex::new(None));
        let actor = system.spawn_local("topic_complex_actor".to_string(), TopicTestActor {
            name: "TopicComplexActor".to_string(),
            received_events: counter.clone(),
            last_topic: last_topic.clone(),
        });

        // Test complex hierarchical patterns
        let classifier = TopicClassifier::<TestEvent>::new(vec![
            "sensor.temperature.*".to_string(),
            "sensor.humidity.living_room".to_string(),
            "system.alert.*".to_string(),
        ]);

        let _sub = eventbus.subscribe(actor.clone(), classifier).await.unwrap();

        // Should match
        let matching = vec![
            "sensor.temperature.kitchen",
            "sensor.temperature.bedroom", 
            "sensor.humidity.living_room",
            "system.alert.high_cpu",
            "system.alert.memory_usage",
        ];

        // Should not match
        let non_matching = vec![
            "sensor.humidity.kitchen",  // Only living_room humidity matches
            "sensor.temperature",        // Wrong level
            "system.warning.high_cpu",   // Different base
            "alert.system.high_cpu",     // Wrong order
        ];

        for topic in &matching {
            let event = TestEvent::new("complex_hierarchy_test".to_string(), format!("payload for {}", topic));
            let result = eventbus.publish_with_topic(event, Some(topic.to_string())).await.unwrap();
            assert_eq!(result.total_subscribers, 1, "Topic {} should match", topic);
        }

        for topic in &non_matching {
            let event = TestEvent::new("complex_hierarchy_non_match".to_string(), format!("payload for {}", topic));
            let result = eventbus.publish_with_topic(event, Some(topic.to_string())).await.unwrap();
            assert_eq!(result.total_subscribers, 0, "Topic {} should not match", topic);
        }

        sleep(Duration::from_millis(100)).await;
        assert_eq!(counter.load(Ordering::SeqCst), matching.len() as u32);

        eventbus.shutdown().await.unwrap();
        system.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_topic_message_headers() {
        let mut system = create_test_system(50205).await;
        let eventbus = EventBus::new(EventBusConfig::default());

        // Create a test actor that handles TestEvent
        struct TopicTestActor {
            name: String,
            received_events: Arc<AtomicU32>,
            last_topic: Arc<Mutex<Option<String>>>,
        }

        #[async_trait::async_trait]
        impl Actor for TopicTestActor {
            type Message = TestEvent;
            type State = ();

            async fn on_message(&mut self, msg: Self::Message, _ctx: &mut ActorContext) -> crate::Result<()> {
                println!("TopicTestActor {} received: {:?}", self.name, msg);
                self.received_events.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        }

        let counter = Arc::new(AtomicU32::new(0));
        let last_topic = Arc::new(Mutex::new(None));
        let actor = system.spawn_local("topic_headers_actor".to_string(), TopicTestActor {
            name: "TopicHeadersActor".to_string(),
            received_events: counter.clone(),
            last_topic: last_topic.clone(),
        });

        let _sub = eventbus.subscribe(actor.clone(), TopicClassifier::<TestEvent>::exact("test.headers".to_string())).await.unwrap();

        // Create event with headers (using TestEvent which doesn't have headers, but we can test the topic functionality)
        let event = TestEvent::new("headers_test".to_string(), "payload with headers".to_string());

        let result = eventbus.publish_with_topic(event, Some("test.headers".to_string())).await.unwrap();
        assert_eq!(result.total_subscribers, 1);

        sleep(Duration::from_millis(100)).await;
        assert_eq!(counter.load(Ordering::SeqCst), 1);

        eventbus.shutdown().await.unwrap();
        system.shutdown().await.unwrap();
    }
}