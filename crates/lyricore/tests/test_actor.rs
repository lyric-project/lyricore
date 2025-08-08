use futures::future::join_all;
use lyricore::{ActorContext, ActorSystem, Message, SchedulerConfig};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Test Actor specifically designed to detect concurrency within a single Actor
struct ConcurrencyDetectionActor {
    actor_id: String,
    // Used for concurrency detection: if there's concurrency, this value will not be 0
    concurrent_counter: Arc<Mutex<i32>>,
    // Record all detailed processing times
    processing_log: Arc<Mutex<Vec<ProcessingEntry>>>,
    message_count: usize,
}

#[derive(Clone, Debug)]
struct ProcessingEntry {
    message_id: String,
    start_time: Instant,
    end_time: Option<Instant>,
    concurrent_count_at_start: i32,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
struct ConcurrencyTestMessage {
    message_id: String,
    processing_duration_ms: u64,
}

// Message trait is automatically implemented, we just need to ensure the Response type is correct

#[async_trait::async_trait]
impl lyricore::Actor for ConcurrencyDetectionActor {
    type Message = ConcurrencyTestMessage;
    type State = ();

    async fn on_start(&mut self, _ctx: &mut ActorContext) -> lyricore::error::Result<()> {
        println!("🔍 ConcurrencyDetectionActor {} started", self.actor_id);
        Ok(())
    }

    async fn on_message(
        &mut self,
        msg: Self::Message,
        _ctx: &mut ActorContext,
    ) -> lyricore::error::Result<()> {
        let start_time = Instant::now();

        // Increment concurrency counter
        let concurrent_count = {
            let mut counter = self.concurrent_counter.lock().unwrap();
            *counter += 1;
            *counter
        };

        println!(
            "🔥 Actor {} starting message {}, concurrent count: {}",
            self.actor_id, msg.message_id, concurrent_count
        );

        // If concurrent count is greater than 1, it means there's concurrent processing!
        if concurrent_count > 1 {
            println!(
                "❌ CONCURRENT PROCESSING DETECTED in Actor {}! Count: {}",
                self.actor_id, concurrent_count
            );
        }

        // Record processing start
        {
            let mut log = self.processing_log.lock().unwrap();
            log.push(ProcessingEntry {
                message_id: msg.message_id.clone(),
                start_time,
                end_time: None,
                concurrent_count_at_start: concurrent_count,
            });
        }

        // Simulate processing time
        tokio::time::sleep(Duration::from_millis(msg.processing_duration_ms)).await;

        self.message_count += 1;
        let end_time = Instant::now();

        // Decrement concurrency counter
        {
            let mut counter = self.concurrent_counter.lock().unwrap();
            *counter -= 1;
            println!(
                "✅ Actor {} finished message {}, concurrent count now: {}",
                self.actor_id, msg.message_id, *counter
            );
        }

        // Update processing record
        {
            let mut log = self.processing_log.lock().unwrap();
            if let Some(entry) = log
                .iter_mut()
                .find(|e| e.message_id == msg.message_id && e.end_time.is_none())
            {
                entry.end_time = Some(end_time);
            }
        }

        Ok(())
    }

    async fn handle_message(
        &mut self,
        msg: Self::Message,
        ctx: &mut ActorContext,
    ) -> lyricore::error::Result<<Self::Message as Message>::Response> {
        self.on_message(msg.clone(), ctx).await?;
        // Return () because this is the Response type of the automatically implemented Message trait
        Ok(())
    }

    async fn on_stop(&mut self, _ctx: &mut ActorContext) -> lyricore::error::Result<()> {
        println!(
            "🛑 Actor {} stopped. Processed {} messages",
            self.actor_id, self.message_count
        );

        // Analyze processing log
        let log = self.processing_log.lock().unwrap();
        let mut max_concurrent = 0;
        let mut concurrent_violations = 0;

        for entry in log.iter() {
            if entry.concurrent_count_at_start > max_concurrent {
                max_concurrent = entry.concurrent_count_at_start;
            }
            if entry.concurrent_count_at_start > 1 {
                concurrent_violations += 1;
            }
        }

        println!(
            "📊 Actor {} Analysis: Max concurrent: {}, Violations: {}",
            self.actor_id, max_concurrent, concurrent_violations
        );

        if concurrent_violations > 0 {
            println!(
                "❌ Actor {} had {} concurrent processing violations!",
                self.actor_id, concurrent_violations
            );
        } else {
            println!(
                "✅ Actor {} maintained sequential processing",
                self.actor_id
            );
        }

        Ok(())
    }
}

#[cfg(test)]
mod precise_tests {
    use super::*;

    /// Precisely test the sequential ordering of a single Actor
    #[tokio::test]
    async fn test_single_actor_strict_ordering() {
        println!("🧪 === Testing Single Actor Strict Sequential Processing ===");

        let mut system = ActorSystem::new(
            "precise_test_node".to_string(),
            "127.0.0.1:50070".to_string(),
            SchedulerConfig::default(),
            None,
        )
        .unwrap();

        system.start_server().await.unwrap();

        let concurrent_counter = Arc::new(Mutex::new(0));
        let processing_log = Arc::new(Mutex::new(Vec::new()));

        let actor_ref = system.spawn_local(
            "strict_actor".to_string(),
            ConcurrencyDetectionActor {
                actor_id: "strict_actor".to_string(),
                concurrent_counter: concurrent_counter.clone(),
                processing_log: processing_log.clone(),
                message_count: 0,
            },
        );

        // Send multiple messages, each with a longer processing time
        let num_messages = 10;
        let mut tasks = Vec::new();

        println!("📤 Sending {} messages to single actor...", num_messages);

        for i in 0..num_messages {
            let actor_ref_clone = actor_ref.clone();
            let task = async move {
                actor_ref_clone
                    .ask(ConcurrencyTestMessage {
                        message_id: format!("msg_{}", i),
                        processing_duration_ms: 200, // 200ms processing time
                    })
                    .await
            };
            tasks.push(task);
        }

        // Send all messages in parallel
        let start_time = Instant::now();
        let results = join_all(tasks).await;
        let total_time = start_time.elapsed();

        // Verify results
        let mut successful_count = 0;
        for (i, result) in results.iter().enumerate() {
            match result {
                Ok(response) => {
                    successful_count += 1;
                    println!("✅ Message {} processed successfully", i);
                }
                Err(e) => {
                    println!("❌ Message {} failed: {:?}", i, e);
                }
            }
        }

        // Analyze processing log
        let final_counter = *concurrent_counter.lock().unwrap();
        let log = processing_log.lock().unwrap();

        println!("\n📊 === Analysis Results ===");
        println!("Total messages sent: {}", num_messages);
        println!("Successful messages: {}", successful_count);
        println!("Total processing time: {:.2}s", total_time.as_secs_f64());
        println!("Final concurrent counter: {}", final_counter);

        // Check if there was concurrent processing
        let mut had_concurrency = false;
        let mut max_concurrent = 0;

        for entry in log.iter() {
            if entry.concurrent_count_at_start > 1 {
                had_concurrency = true;
                println!(
                    "❌ VIOLATION: Message {} started with concurrent count {}",
                    entry.message_id, entry.concurrent_count_at_start
                );
            }
            if entry.concurrent_count_at_start > max_concurrent {
                max_concurrent = entry.concurrent_count_at_start;
            }
        }

        println!("Max concurrent processing detected: {}", max_concurrent);

        // Assert: A single Actor should never have concurrent processing
        assert_eq!(
            final_counter, 0,
            "Concurrent counter should be 0 at the end, but was {}",
            final_counter
        );

        if had_concurrency {
            println!("❌ TEST FAILED: Single actor had concurrent message processing!");
            println!("🔍 This indicates a serious bug in the actor framework!");

            // Print detailed timeline
            println!("\n📋 Detailed Timeline:");
            for entry in log.iter() {
                println!(
                    "  {} - Start: {:?}, End: {:?}, Concurrent: {}",
                    entry.message_id,
                    entry.start_time,
                    entry.end_time,
                    entry.concurrent_count_at_start
                );
            }

            panic!("Single actor concurrent processing detected!");
        } else {
            println!("✅ TEST PASSED: Single actor maintained sequential processing");
        }

        // Verify processing time: if strictly sequential, total time should be close to num_messages * 200ms
        let expected_min_time = Duration::from_millis(num_messages as u64 * 200);

        if total_time < expected_min_time {
            println!(
                "⚠️  WARNING: Total time {:.2}s is less than expected {:.2}s",
                total_time.as_secs_f64(),
                expected_min_time.as_secs_f64()
            );
            println!("This might indicate concurrent processing!");
        } else {
            println!(
                "✅ Timing check passed: {:.2}s >= {:.2}s",
                total_time.as_secs_f64(),
                expected_min_time.as_secs_f64()
            );
        }

        assert_eq!(successful_count, num_messages);
        assert!(
            !had_concurrency,
            "Actor framework violated sequential processing guarantee!"
        );
    }

    /// Test concurrent independence of multiple Actors
    #[tokio::test]
    async fn test_multiple_actors_concurrent_independence() {
        println!("🧪 === Testing Multiple Actors Concurrent Independence ===");

        let mut system = ActorSystem::new(
            "multi_test_node".to_string(),
            "127.0.0.1:50071".to_string(),
            SchedulerConfig::default(),
            None,
        )
        .unwrap();

        system.start_server().await.unwrap();

        let num_actors = 3;
        let messages_per_actor = 5;
        let mut actors = Vec::new();
        let mut concurrent_counters = Vec::new();

        // Create multiple Actors
        for i in 0..num_actors {
            let concurrent_counter = Arc::new(Mutex::new(0));
            concurrent_counters.push(concurrent_counter.clone());

            let actor_ref = system.spawn_local(
                format!("multi_actor_{}", i),
                ConcurrencyDetectionActor {
                    actor_id: format!("multi_actor_{}", i),
                    concurrent_counter,
                    processing_log: Arc::new(Mutex::new(Vec::new())),
                    message_count: 0,
                },
            );
            actors.push(actor_ref);
        }

        // Send messages to each Actor
        let mut all_tasks = Vec::new();

        for (actor_idx, actor_ref) in actors.iter().enumerate() {
            for msg_idx in 0..messages_per_actor {
                let actor_ref_clone = actor_ref.clone();
                let task = async move {
                    actor_ref_clone
                        .ask(ConcurrencyTestMessage {
                            message_id: format!("actor_{}_msg_{}", actor_idx, msg_idx),
                            processing_duration_ms: 300, // 300ms processing time
                        })
                        .await
                };
                all_tasks.push(task);
            }
        }

        // Process all messages in parallel
        let start_time = Instant::now();
        let results = join_all(all_tasks).await;
        let total_time = start_time.elapsed();

        // Verify results
        let total_messages = num_actors * messages_per_actor;
        let mut successful_count = 0;

        for result in results {
            if result.is_ok() {
                successful_count += 1;
            }
        }

        println!("\n📊 === Multi-Actor Analysis ===");
        println!("Total actors: {}", num_actors);
        println!("Messages per actor: {}", messages_per_actor);
        println!("Total messages: {}", total_messages);
        println!("Successful messages: {}", successful_count);
        println!("Total processing time: {:.2}s", total_time.as_secs_f64());

        // Check concurrency situation for each Actor
        let mut any_actor_had_concurrency = false;
        for (i, counter) in concurrent_counters.iter().enumerate() {
            let final_count = *counter.lock().unwrap();
            if final_count != 0 {
                println!("❌ Actor {} has non-zero final counter: {}", i, final_count);
                any_actor_had_concurrency = true;
            } else {
                println!("✅ Actor {} ended with counter 0", i);
            }
        }

        // Verify timing: multiple Actors should be able to process concurrently,
        // total time should be less than serial processing time of all messages
        let serial_time = Duration::from_millis(total_messages as u64 * 300);
        let parallel_time_per_actor = Duration::from_millis(messages_per_actor as u64 * 300);

        println!(
            "Expected serial time: {:.2}s, Expected parallel time: {:.2}s, Actual: {:.2}s",
            serial_time.as_secs_f64(),
            parallel_time_per_actor.as_secs_f64(),
            total_time.as_secs_f64()
        );

        assert_eq!(successful_count, total_messages);
        assert!(
            !any_actor_had_concurrency,
            "Some actors had internal concurrency violations!"
        );

        // Verify true concurrency: total time should be close to single Actor's processing time,
        // not the serial time of all messages
        if total_time > serial_time * 80 / 100 {
            println!(
                "⚠️  WARNING: Processing time suggests lack of true concurrency between actors"
            );
        } else {
            println!("✅ Multi-actor concurrency confirmed by timing");
        }

        println!("✅ Multi-actor independence test completed");
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🔍 Running Precise Actor Ordering Tests");
    println!("These tests will detect if actors are violating sequential processing guarantees");
    println!("\nTo run tests: cargo test precise_tests");
    Ok(())
}
