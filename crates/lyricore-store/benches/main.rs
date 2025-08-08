#![feature(test)]
extern crate test;

use bytes::Bytes;
use std::sync::Arc;
use test::Bencher;

use lyricore_store::{ObjectBuilder, ObjectId, ObjectStore, StoreConfig};

// Test data generators
fn generate_test_data(size: usize) -> Vec<u8> {
    (0..size).map(|i| (i % 256) as u8).collect()
}

fn generate_large_test_data(size: usize) -> Vec<u8> {
    let mut data = Vec::with_capacity(size);
    let pattern = b"Hello, World! This is a test pattern for benchmarking. ";
    for i in 0..size {
        data.push(pattern[i % pattern.len()]);
    }
    data
}

fn create_test_store() -> ObjectStore {
    let config = StoreConfig {
        max_memory: 1024 * 1024 * 1024,    // 1GB
        max_object_size: 64 * 1024 * 1024, // 64MB
        memory_pressure_threshold: 0.9,
        track_access_time: true,
    };
    ObjectStore::new(config)
}

fn create_fast_store() -> ObjectStore {
    let config = StoreConfig {
        max_memory: 1024 * 1024 * 1024,    // 1GB
        max_object_size: 64 * 1024 * 1024, // 64MB
        memory_pressure_threshold: 0.9,
        // Disable access time tracking for performance
        track_access_time: false,
    };
    ObjectStore::new(config)
}

// ============ PUT basic  ===========

#[bench]
fn bench_put_small_objects_1kb(b: &mut Bencher) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let store = create_test_store();
    let data = generate_test_data(1024); // 1KB

    b.iter(|| {
        rt.block_on(async {
            let _ = store.put(data.clone()).await;
        });
    });
}

#[bench]
fn bench_put_medium_objects_100kb(b: &mut Bencher) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let store = create_test_store();
    let data = generate_test_data(100 * 1024); // 100KB

    b.iter(|| {
        rt.block_on(async {
            let _ = store.put(data.clone()).await;
        });
    });
}

#[bench]
fn bench_put_large_objects_1mb(b: &mut Bencher) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let store = create_test_store();
    let data = generate_large_test_data(1024 * 1024); // 1MB

    b.iter(|| {
        rt.block_on(async {
            let _ = store.put(data.clone()).await;
        });
    });
}

#[bench]
fn bench_put_arc_shared_data(b: &mut Bencher) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let store = create_test_store();
    let data: Arc<[u8]> = Arc::from(generate_test_data(64 * 1024).into_boxed_slice()); // 64KB

    b.iter(|| {
        rt.block_on(async {
            let _ = store.put_arc(Arc::clone(&data)).await;
        });
    });
}

#[bench]
fn bench_put_bytes_data(b: &mut Bencher) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let store = create_test_store();
    let data = Bytes::from(generate_test_data(64 * 1024)); // 64KB

    b.iter(|| {
        rt.block_on(async {
            let _ = store.put_bytes(data.clone()).await;
        });
    });
}

#[bench]
fn bench_put_numpy_data(b: &mut Bencher) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let store = create_test_store();
    let data = generate_test_data(1000 * 8); // 1000 float64 elements

    b.iter(|| {
        rt.block_on(async {
            let _ = store
                .put_numpy(data.clone(), "float64".to_string(), vec![100, 10], 'C')
                .await;
        });
    });
}

#[bench]
fn bench_put_arrow_data(b: &mut Bencher) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let store = create_test_store();
    let data: Arc<[u8]> = Arc::from(generate_test_data(64 * 1024).into_boxed_slice());

    b.iter(|| {
        rt.block_on(async {
            let _ = store.put_arrow(Arc::clone(&data), 64).await;
        });
    });
}

// ============ GET basic ============

#[bench]
fn bench_get_small_objects(b: &mut Bencher) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let store = create_test_store();

    // Pre-fill data
    let mut ids = Vec::new();
    rt.block_on(async {
        for _ in 0..1000 {
            let data = generate_test_data(1024);
            let id = store.put(data).await.unwrap();
            ids.push(id);
        }
    });

    let mut index = 0;
    b.iter(|| {
        rt.block_on(async {
            let id = ids[index % ids.len()];
            let _ = store.get(id).await.unwrap();
            index += 1;
        });
    });
}

#[bench]
fn bench_get_large_objects(b: &mut Bencher) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let store = create_test_store();

    // Bigger objects
    let mut ids = Vec::new();
    rt.block_on(async {
        for _ in 0..10 {
            let data = generate_large_test_data(1024 * 1024); // 1MB each
            let id = store.put(data).await.unwrap();
            ids.push(id);
        }
    });

    let mut index = 0;
    b.iter(|| {
        rt.block_on(async {
            let id = ids[index % ids.len()];
            let _ = store.get(id).await.unwrap();
            index += 1;
        });
    });
}

#[bench]
fn bench_get_with_fast_store(b: &mut Bencher) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let store = create_fast_store(); // Disabled access time tracking

    let mut ids = Vec::new();
    rt.block_on(async {
        for _ in 0..1000 {
            let data = generate_test_data(1024);
            let id = store.put(data).await.unwrap();
            ids.push(id);
        }
    });

    let mut index = 0;
    b.iter(|| {
        rt.block_on(async {
            let id = ids[index % ids.len()];
            let _ = store.get(id).await.unwrap();
            index += 1;
        });
    });
}

// ============ Batch operations ============

#[bench]
fn bench_get_batch_10_objects(b: &mut Bencher) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let store = create_test_store();

    let mut ids = Vec::new();
    rt.block_on(async {
        for _ in 0..100 {
            let data = generate_test_data(1024);
            let id = store.put(data).await.unwrap();
            ids.push(id);
        }
    });

    b.iter(|| {
        rt.block_on(async {
            let batch_ids: Vec<ObjectId> = ids.iter().take(10).copied().collect();
            let _ = store.get_batch(&batch_ids).await;
        });
    });
}

#[bench]
fn bench_get_batch_100_objects(b: &mut Bencher) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let store = create_test_store();

    let mut ids = Vec::new();
    rt.block_on(async {
        for _ in 0..1000 {
            let data = generate_test_data(1024);
            let id = store.put(data).await.unwrap();
            ids.push(id);
        }
    });

    b.iter(|| {
        rt.block_on(async {
            let batch_ids: Vec<ObjectId> = ids.iter().take(100).copied().collect();
            let _ = store.get_batch(&batch_ids).await;
        });
    });
}

#[bench]
fn bench_put_shared_10_objects(b: &mut Bencher) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let store = create_test_store();
    let shared_data: Arc<[u8]> = Arc::from(generate_test_data(64 * 1024).into_boxed_slice());

    b.iter(|| {
        rt.block_on(async {
            let _ = store.put_shared(Arc::clone(&shared_data), 10).await;
        });
    });
}

#[bench]
fn bench_put_shared_bytes_10_objects(b: &mut Bencher) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let store = create_test_store();
    let shared_data = Bytes::from(generate_test_data(64 * 1024));

    b.iter(|| {
        rt.block_on(async {
            let _ = store.put_shared_bytes(shared_data.clone(), 10).await;
        });
    });
}

// ============ Data views and access ============

#[bench]
fn bench_get_view_access(b: &mut Bencher) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let store = create_test_store();

    let id = rt.block_on(async {
        let data = generate_test_data(1024 * 1024); // 1MB
        store
            .put_numpy(data, "float64".to_string(), vec![128, 1024], 'C')
            .await
            .unwrap()
    });

    b.iter(|| {
        rt.block_on(async {
            let view = store.get_view(id).await.unwrap();
            let _ = view.as_bytes();
            if let Some(numpy_view) = view.as_numpy_compatible() {
                let _ = numpy_view.for_numpy_capi();
            }
        });
    });
}

#[bench]
fn bench_get_raw_ptr_access(b: &mut Bencher) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let store = create_test_store();

    let mut ids = Vec::new();
    rt.block_on(async {
        for _ in 0..100 {
            let data = generate_test_data(1024);
            let id = store.put(data).await.unwrap();
            ids.push(id);
        }
    });

    let mut index = 0;
    b.iter(|| {
        rt.block_on(async {
            let id = ids[index % ids.len()];
            let _ = store.get_raw_ptr(id).await.unwrap();
            index += 1;
        });
    });
}

// ============ Memory management and pressure handling ============

#[bench]
fn bench_delete_operations(b: &mut Bencher) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let store = create_test_store();

    b.iter(|| {
        rt.block_on(async {
            // Create and put an object
            let data = generate_test_data(1024);
            let id = store.put(data).await.unwrap();

            // Delete the object
            let _ = store.delete(id).await.unwrap();
        });
    });
}

#[bench]
fn bench_memory_pressure_handling(b: &mut Bencher) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    let config = StoreConfig {
        max_memory: 10 * 1024 * 1024, // 10MB
        max_object_size: 1024 * 1024, // 1MB
        memory_pressure_threshold: 0.7,
        track_access_time: true,
    };
    let store = ObjectStore::new(config);

    b.iter(|| {
        rt.block_on(async {
            // Pressure the memory limit by adding objects
            for _ in 0..12 {
                let data = generate_test_data(1024 * 1024); // 1MB each
                let _ = store.put(data).await; // It will trigger memory pressure handling
            }

            // Clear the store to release memory
            store.clear().await;
        });
    });
}

// ============ Parallel and concurrent operations ============

#[bench]
fn bench_concurrent_puts(b: &mut Bencher) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let store = Arc::new(create_test_store());

    b.iter(|| {
        rt.block_on(async {
            let mut handles = Vec::new();

            for i in 0..10 {
                let store_clone = Arc::clone(&store);
                let handle = tokio::spawn(async move {
                    let data = generate_test_data(1024 + i * 100);
                    store_clone.put(data).await
                });
                handles.push(handle);
            }

            for handle in handles {
                let _ = handle.await.unwrap();
            }
        });
    });
}

#[bench]
fn bench_concurrent_gets(b: &mut Bencher) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let store = Arc::new(create_test_store());

    // Pre-fill data
    let ids = rt.block_on(async {
        let mut ids = Vec::new();
        for _ in 0..100 {
            let data = generate_test_data(1024);
            let id = store.put(data).await.unwrap();
            ids.push(id);
        }
        ids
    });

    b.iter(|| {
        rt.block_on(async {
            let mut handles = Vec::new();

            for &id in ids.iter().take(10) {
                let store_clone = Arc::clone(&store);
                let handle = tokio::spawn(async move { store_clone.get(id).await });
                handles.push(handle);
            }

            for handle in handles {
                let _ = handle.await.unwrap();
            }
        });
    });
}

// ============ Some additional benchmarks ============

#[bench]
fn bench_object_builder_overhead(b: &mut Bencher) {
    let data = generate_test_data(1024);

    b.iter(|| {
        let obj = ObjectBuilder::new()
            .from_vec(data.clone())
            .with_numpy_metadata("float64".to_string(), vec![128], 'C')
            .build()
            .unwrap();

        test::black_box(obj);
    });
}

#[bench]
fn bench_contains_check(b: &mut Bencher) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let store = create_test_store();

    let mut ids = Vec::new();
    rt.block_on(async {
        for _ in 0..1000 {
            let data = generate_test_data(1024);
            let id = store.put(data).await.unwrap();
            ids.push(id);
        }
    });

    let mut index = 0;
    b.iter(|| {
        rt.block_on(async {
            let id = ids[index % ids.len()];
            let _ = store.contains(id).await;
            index += 1;
        });
    });
}

#[bench]
fn bench_stats_collection(b: &mut Bencher) {
    let store = create_test_store();

    b.iter(|| {
        let stats = store.stats();
        test::black_box(stats);
    });
}

// ============ Throughput benchmarks ============

#[bench]
fn bench_throughput_small_objects(b: &mut Bencher) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let store = create_test_store();

    b.iter(|| {
        rt.block_on(async {
            let mut ids = Vec::new();

            // Batch insert 100 small objects
            for _ in 0..100 {
                let data = generate_test_data(1024);
                let id = store.put(data).await.unwrap();
                ids.push(id);
            }

            // Batch get 100 objects
            for id in &ids {
                let _ = store.get(*id).await.unwrap();
            }

            // Batch delete the objects
            for id in ids {
                let _ = store.delete(id).await.unwrap();
            }
        });
    });
}
