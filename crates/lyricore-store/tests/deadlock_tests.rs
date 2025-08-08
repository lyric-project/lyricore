#![feature(test)]
extern crate test;

use bytes::Bytes;
use lyricore_store::{ObjectBuilder, ObjectId, ObjectStore, StoreConfig};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;

#[cfg(test)]
mod deadlock_tests {
    use super::*;
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    use tokio::task::JoinSet;
    use tokio::time::timeout;

    // Deadlock risk points analysis:
    // 1. cleanup_lru_objects()  Get write lock in cleanup_lru_objects() for a long time
    // 2. Memory pressure check may contend with normal operations
    // 3. Parallel put operations all need to check memory pressure, which may form lock contention

    /// Test concurrent read and write operations for deadlock risks
    #[tokio::test]
    async fn test_concurrent_read_write_no_deadlock() {
        let store = Arc::new(ObjectStore::default());
        let timeout_duration = Duration::from_secs(10);

        let test_result = timeout(timeout_duration, async {
            let mut tasks = JoinSet::new();

            // Start multiple write tasks
            for i in 0..50 {
                let store_clone = Arc::clone(&store);
                tasks.spawn(async move {
                    for j in 0..10 {
                        let data = vec![i as u8; 1024 + j * 100];
                        let _ = store_clone.put(data).await;
                    }
                });
            }

            // Start multiple read tasks
            for _ in 0..50 {
                let store_clone = Arc::clone(&store);
                tasks.spawn(async move {
                    // Write some data first for reading
                    let mut ids = Vec::new();
                    for k in 0..5 {
                        let data = vec![k as u8; 1024];
                        if let Ok(id) = store_clone.put(data).await {
                            ids.push(id);
                        }
                    }

                    // Read the data
                    for id in ids {
                        let _ = store_clone.get(id).await;
                    }
                });
            }

            // Wait for all tasks to complete
            while let Some(result) = tasks.join_next().await {
                result.expect("Task should not panic");
            }
        })
        .await;

        assert!(
            test_result.is_ok(),
            "Timeout occurred, possible deadlock detected"
        );
    }

    /// Test deadlock risk during memory pressure handling
    #[tokio::test]
    async fn test_memory_pressure_no_deadlock() {
        // Create a store with small memory limits to quickly trigger memory pressure
        let config = StoreConfig {
            max_memory: 50 * 1024,      // 50KB
            max_object_size: 10 * 1024, // 10KB
            memory_pressure_threshold: 0.5,
            track_access_time: true,
        };
        let store = Arc::new(ObjectStore::new(config));
        let timeout_duration = Duration::from_secs(15);

        let test_result = timeout(timeout_duration, async {
            let mut tasks = JoinSet::new();

            // Start multiple tasks to trigger memory pressure simultaneously
            for i in 0..20 {
                let store_clone = Arc::clone(&store);
                tasks.spawn(async move {
                    for j in 0..10 {
                        let data = vec![i as u8; 8 * 1024]; // 8KB
                                                            // It will trigger memory pressure handling
                        let _ = store_clone.put(data).await;

                        if j % 3 == 0 {
                            let stats = store_clone.stats();
                            if stats.total_objects > 0 {
                                for _ in 0..3 {
                                    let test_id =
                                        ObjectId::from_str("1").unwrap_or(ObjectId::new());
                                    let _ = store_clone.get(test_id).await;
                                }
                            }
                        }
                    }
                });
            }

            // Wait for all tasks to complete
            while let Some(result) = tasks.join_next().await {
                result.expect("Task should not panic");
            }
        })
        .await;

        assert!(
            test_result.is_ok(),
            "Timeout occurred, possible deadlock detected"
        );
    }

    /// Test deadlock risks of manual cleanup operations
    #[tokio::test]
    async fn test_cleanup_operations_no_deadlock() {
        let store = Arc::new(ObjectStore::default());
        let timeout_duration = Duration::from_secs(10);

        let test_result = timeout(timeout_duration, async {
            let mut tasks = JoinSet::new();

            // Fill some data first
            for i in 0..100 {
                let data = vec![i as u8; 1024];
                let _ = store.put(data).await;
            }

            // Execute cleanup and normal operations concurrently
            for i in 0..10 {
                let store_clone = Arc::clone(&store);
                if i % 3 == 0 {
                    // Cleanup tasks
                    tasks.spawn(async move {
                        for _ in 0..5 {
                            let _ = store_clone.cleanup().await;
                            tokio::time::sleep(Duration::from_millis(10)).await;
                        }
                    });
                } else if i % 3 == 1 {
                    // Write tasks
                    tasks.spawn(async move {
                        for j in 0..20 {
                            let data = vec![j as u8; 1024];
                            let _ = store_clone.put(data).await;
                        }
                    });
                } else {
                    // Read tasks
                    tasks.spawn(async move {
                        for _ in 0..30 {
                            let test_id = ObjectId::new();
                            let _ = store_clone.get(test_id).await;
                        }
                    });
                }
            }

            // Wait for all tasks to complete
            while let Some(result) = tasks.join_next().await {
                result.expect("Task should not panic");
            }
        })
        .await;

        assert!(
            test_result.is_ok(),
            "Cleanup operation test timeout, possible deadlock"
        );
    }

    /// Test deadlock risks of batch operations
    #[tokio::test]
    async fn test_batch_operations_no_deadlock() {
        let store = Arc::new(ObjectStore::default());
        let timeout_duration = Duration::from_secs(10);

        let test_result = timeout(timeout_duration, async {
            let mut tasks = JoinSet::new();

            // Execute batch operations concurrently
            for i in 0..20 {
                let store_clone = Arc::clone(&store);
                tasks.spawn(async move {
                    // Batch insert
                    let mut ids = Vec::new();
                    for j in 0..10 {
                        let data = vec![i as u8; 1024 + j * 100];
                        if let Ok(id) = store_clone.put(data).await {
                            ids.push(id);
                        }
                    }

                    // Batch read
                    if !ids.is_empty() {
                        let _ = store_clone.get_batch(&ids).await;
                    }

                    // Batch delete half
                    for (idx, id) in ids.into_iter().enumerate() {
                        if idx % 2 == 0 {
                            let _ = store_clone.delete(id).await;
                        }
                    }
                });
            }

            // Wait for all tasks to complete
            while let Some(result) = tasks.join_next().await {
                result.expect("Task should not panic");
            }
        })
        .await;

        assert!(
            test_result.is_ok(),
            "Batch operation test timeout, possible deadlock"
        );
    }

    /// Stress test: Deadlock detection under high concurrency
    #[tokio::test]
    async fn test_high_concurrency_stress() {
        let store = Arc::new(ObjectStore::default());
        let timeout_duration = Duration::from_secs(30);

        let test_result = timeout(timeout_duration, async {
            let mut tasks = JoinSet::new();

            // High concurrency tasks
            for i in 0..100 {
                let store_clone = Arc::clone(&store);
                tasks.spawn(async move {
                    match i % 4 {
                        0 => {
                            // Pure write
                            for j in 0..20 {
                                let data = vec![(i + j) as u8; 1024];
                                let _ = store_clone.put(data).await;
                            }
                        }
                        1 => {
                            // Write then immediately read
                            for j in 0..15 {
                                let data = vec![(i + j) as u8; 1024];
                                if let Ok(id) = store_clone.put(data).await {
                                    let _ = store_clone.get(id).await;
                                }
                            }
                        }
                        2 => {
                            // Mixed write, read, delete
                            for j in 0..10 {
                                let data = vec![(i + j) as u8; 1024];
                                if let Ok(id) = store_clone.put(data).await {
                                    let _ = store_clone.get(id).await;
                                    if j % 2 == 0 {
                                        let _ = store_clone.delete(id).await;
                                    }
                                }
                            }
                        }
                        3 => {
                            // Statistics and check operations
                            for _ in 0..50 {
                                let _ = store_clone.stats();
                                let test_id = ObjectId::new();
                                let _ = store_clone.contains(test_id).await;
                            }
                        }
                        _ => unreachable!(),
                    }
                });
            }

            // Wait for all tasks to complete
            while let Some(result) = tasks.join_next().await {
                result.expect("Task should not panic");
            }
        })
        .await;

        assert!(
            test_result.is_ok(),
            "High concurrency stress test timeout, possible deadlock"
        );
    }

    /// Test lock contention detection
    #[tokio::test]
    async fn test_lock_contention_measurement() {
        let store = Arc::new(ObjectStore::default());
        let contention_measurements = Arc::new(std::sync::Mutex::new(Vec::new()));

        let mut tasks = JoinSet::new();

        // Start multiple tasks competing for write locks
        for i in 0..10 {
            let store_clone = Arc::clone(&store);
            let measurements = Arc::clone(&contention_measurements);

            tasks.spawn(async move {
                for j in 0..20 {
                    let start = Instant::now();
                    let data = vec![(i * 20 + j) as u8; 1024];
                    let _ = store_clone.put(data).await;
                    let duration = start.elapsed();

                    // Record execution time
                    measurements.lock().unwrap().push(duration);
                }
            });
        }

        // Wait for all tasks to complete
        while let Some(result) = tasks.join_next().await {
            result.expect("Task should not panic");
        }

        // Analyze lock contention
        let durations = contention_measurements.lock().unwrap();
        let avg_duration: Duration = durations.iter().sum::<Duration>() / durations.len() as u32;
        let max_duration = durations.iter().max().unwrap();

        println!("Lock contention analysis:");
        println!("  Average execution time: {:?}", avg_duration);
        println!("  Maximum execution time: {:?}", max_duration);
        println!("  Total operations: {}", durations.len());

        // If maximum execution time exceeds 100ms, there may be severe lock contention
        assert!(
            max_duration.as_millis() < 100,
            "Severe lock contention detected, maximum execution time: {:?}",
            max_duration
        );
    }

    /// Helper function: Run test with deadlock detection timeout
    async fn run_with_deadlock_detection<F, Fut>(test_fn: F, timeout_secs: u64) -> bool
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = ()>,
    {
        let timeout_duration = Duration::from_secs(timeout_secs);
        timeout(timeout_duration, test_fn()).await.is_ok()
    }
}

// Convenient macro for running deadlock detection
#[macro_export]
macro_rules! assert_no_deadlock {
    ($test_expr:expr, $timeout_secs:expr) => {
        let timeout_duration = std::time::Duration::from_secs($timeout_secs);
        let result = tokio::time::timeout(timeout_duration, $test_expr).await;
        assert!(result.is_ok(), "Operation timeout, possible deadlock");
    };
}

// Deadlock detection versions in benchmarks
#[cfg(test)]
mod deadlock_aware_benches {
    use super::*;
    use test::Bencher;
    use tokio::task::JoinSet;

    #[bench]
    fn bench_deadlock_detection_concurrent_ops(b: &mut Bencher) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let store = Arc::new(ObjectStore::default());

        b.iter(|| {
            rt.block_on(async {
                // Use timeout to ensure no deadlock
                let timeout_duration = Duration::from_millis(1000);
                let result = tokio::time::timeout(timeout_duration, async {
                    let mut tasks = JoinSet::new();

                    // Concurrent writes
                    for i in 0..5 {
                        let store_clone = Arc::clone(&store);
                        tasks.spawn(async move {
                            let data = vec![i as u8; 1024];
                            store_clone.put(data).await
                        });
                    }

                    // Concurrent reads
                    for _ in 0..5 {
                        let store_clone = Arc::clone(&store);
                        // tasks.spawn(async move {
                        //     let test_id = ObjectId::new();
                        //     store_clone.get(test_id).await
                        // });
                        tasks.spawn(async move {
                            let test_id = ObjectId::new();
                            let result = store_clone.get(test_id).await;
                            // Handle the result if needed, or just let it drop
                            result.map(|r| r.id())
                        });
                    }

                    while let Some(_) = tasks.join_next().await {}
                })
                .await;

                assert!(result.is_ok(), "Deadlock detected in benchmark test");
            });
        });
    }
}
