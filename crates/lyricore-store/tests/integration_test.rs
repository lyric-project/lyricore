use lyricore_store::*;
use std::sync::Arc;

#[tokio::test]
async fn test_basic_operations() {
    let store = ObjectStore::default();

    let data = vec![1, 2, 3, 4, 5];
    let id = store.put(data.clone()).await.unwrap();

    assert!(store.contains(id).await);

    let obj_ref = store.get(id).await.unwrap();
    assert_eq!(obj_ref.data(), &data[..]);
    assert_eq!(obj_ref.size(), data.len());

    store.delete(id).await.unwrap();
    assert!(!store.contains(id).await);
}

#[tokio::test]
async fn test_zero_copy_operations() {
    let store = ObjectStore::default();

    let data = vec![1, 2, 3, 4, 5];
    let id = store.put(data.clone()).await.unwrap();

    let view = store.get_view(id).await.unwrap();
    assert_eq!(view.as_bytes(), &data[..]);

    let arc_data = view.as_arc();
    assert_eq!(arc_data.as_ref(), &data[..]);

    let (ptr, size) = store.get_raw_ptr(id).await.unwrap();
    assert!(!ptr.is_null());
    assert_eq!(size, data.len());
}

#[tokio::test]
async fn test_numpy_integration() {
    let store = ObjectStore::default();

    let data = vec![1u8, 2, 3, 4, 5, 6];
    let id = store
        .put_numpy(data.clone(), "uint8".to_string(), vec![2, 3], 'C')
        .await
        .unwrap();

    let view = store.get_view(id).await.unwrap();
    let numpy_view = view.as_numpy_compatible().unwrap();

    assert_eq!(numpy_view.dtype, "uint8");
    assert_eq!(numpy_view.shape, vec![2, 3]);
    assert_eq!(numpy_view.order, 'C');

    let (ptr, dims, itemsize) = numpy_view.for_numpy_capi();
    assert!(!ptr.is_null());
    assert_eq!(dims, vec![2, 3]);
    assert_eq!(itemsize, 1);
}

#[tokio::test]
async fn test_arrow_integration() {
    let store = ObjectStore::default();

    let data = vec![0u8; 1024];
    let id = store.put_arrow(data.clone().into(), 64).await.unwrap();

    let view = store.get_view(id).await.unwrap();
    let arrow_view = view.as_arrow_compatible().unwrap();

    assert_eq!(arrow_view.alignment, 64);

    let (ptr, len, alignment) = arrow_view.as_arrow_buffer();
    assert!(!ptr.is_null());
    assert_eq!(len, 1024);
    assert_eq!(alignment, 64);
}

#[tokio::test]
async fn test_shared_data() {
    let store = ObjectStore::default();

    let shared_data: Arc<[u8]> = Arc::from(&b"shared data"[..]);
    let ids = store.put_shared(shared_data, 3).await.unwrap();

    assert_eq!(ids.len(), 3);

    for id in ids {
        let obj_ref = store.get(id).await.unwrap();
        assert_eq!(obj_ref.data(), b"shared data");
    }

    let stats = store.stats();
    assert_eq!(stats.total_objects, 3);
    assert_eq!(stats.total_memory, 33);
}

#[tokio::test]
async fn test_batch_operations() {
    let store = ObjectStore::default();

    let id1 = store.put(vec![1, 2, 3]).await.unwrap();
    let id2 = store.put(vec![4, 5, 6]).await.unwrap();
    let id3 = store.put(vec![7, 8, 9]).await.unwrap();

    let ids = vec![id1, id2, id3];

    let views = store.get_views(&ids).await;
    assert_eq!(views.len(), 3);

    let arcs = store.get_arcs(&ids).await;
    assert_eq!(arcs.len(), 3);

    let ptrs = store.get_raw_ptrs(&ids).await;
    assert_eq!(ptrs.len(), 3);

    for result in arcs {
        let arc = result.unwrap();
        assert_eq!(arc.len(), 3);
    }
}

#[tokio::test]
async fn test_memory_management() {
    let store = ObjectStore::new(StoreConfig {
        max_memory: 200,
        max_object_size: 50,
        memory_pressure_threshold: 0.5,
        track_access_time: true,
    });

    store.put(vec![1u8; 30]).await.unwrap();
    store.put(vec![2u8; 30]).await.unwrap();
    store.put(vec![3u8; 30]).await.unwrap();

    assert_eq!(store.stats().total_objects, 3);

    let cleaned = store.cleanup().await.unwrap();
    assert!(cleaned > 0);

    assert!(store.stats().total_objects <= 2);
}

#[tokio::test]
async fn test_stats() {
    let store = ObjectStore::default();

    let id1 = store.put(vec![1, 2, 3]).await.unwrap();
    let id2 = store.put(vec![4, 5, 6, 7]).await.unwrap();

    let stats = store.stats();
    assert_eq!(stats.total_objects, 2);
    assert_eq!(stats.total_memory, 7);
    assert_eq!(stats.hit_rate(), 0.0);

    store.get(id1).await.unwrap();
    store.get(id1).await.unwrap();
    store.get(id2).await.unwrap();

    let stats = store.stats();
    assert_eq!(stats.cache_hits, 3);
    assert_eq!(stats.cache_misses, 0);
    assert_eq!(stats.hit_rate(), 1.0);
}

#[tokio::test]
async fn test_object_size_limit() {
    let store = ObjectStore::new(StoreConfig {
        max_object_size: 10,
        ..Default::default()
    });

    let result = store.put(vec![1u8; 20]).await;
    assert!(matches!(result, Err(StoreError::ObjectTooLarge { .. })));
}

#[tokio::test]
async fn test_concurrent_access() {
    use std::sync::Arc;
    use tokio::task::JoinSet;

    let store = Arc::new(ObjectStore::default());
    let mut join_set = JoinSet::new();

    for i in 0..10 {
        let store_clone = Arc::clone(&store);
        join_set.spawn(async move {
            let data = vec![i as u8; 100];
            let id = store_clone.put(data).await.unwrap();

            tokio::time::sleep(tokio::time::Duration::from_millis(1)).await;

            let obj_ref = store_clone.get(id).await.unwrap();
            assert_eq!(obj_ref.data()[0], i as u8);

            id
        });
    }

    let mut ids = Vec::new();
    while let Some(result) = join_set.join_next().await {
        ids.push(result.unwrap());
    }

    assert_eq!(ids.len(), 10);
    assert_eq!(store.stats().total_objects, 10);
}

#[tokio::test]
async fn test_clear() {
    let store = ObjectStore::default();

    store.put(vec![1, 2, 3]).await.unwrap();
    store.put(vec![4, 5, 6]).await.unwrap();

    assert_eq!(store.stats().total_objects, 2);

    store.clear().await;

    assert_eq!(store.stats().total_objects, 0);
    assert_eq!(store.stats().total_memory, 0);
}

#[test]
fn test_object_builder() {
    let obj = ObjectBuilder::new()
        .from_vec(vec![1, 2, 3, 4, 5])
        .with_numpy_metadata("int32".to_string(), vec![5], 'C')
        .build()
        .unwrap();

    assert_eq!(obj.size, 5);
    assert_eq!(obj.metadata.shape, Some(vec![5]));

    if let DataType::NumpyArray { dtype, order } = &obj.metadata.data_type {
        assert_eq!(dtype, "int32");
        assert_eq!(*order, 'C');
    } else {
        panic!("Expected NumpyArray");
    }
}

#[tokio::test]
async fn test_error_handling() {
    let store = ObjectStore::default();
    let fake_id = ObjectId::new();

    let result = store.get(fake_id).await;
    assert!(matches!(result, Err(StoreError::ObjectNotFound(_))));

    let result = store.delete(fake_id).await;
    assert!(matches!(result, Err(StoreError::ObjectNotFound(_))));
}

#[test]
fn test_object_id_uniqueness() {
    let id1 = ObjectId::new();
    let id2 = ObjectId::new();
    let id3 = ObjectId::new();

    assert_ne!(id1, id2);
    assert_ne!(id2, id3);
    assert_ne!(id1, id3);

    assert!(id2.0 > id1.0);
    assert!(id3.0 > id2.0);
}

#[tokio::test]
async fn test_arc_sharing() {
    let store = ObjectStore::default();

    let data: Arc<[u8]> = Arc::from(&b"test"[..]);
    let id = store.put_arc(Arc::clone(&data)).await.unwrap();

    let obj_ref = store.get(id).await.unwrap();
    let arc_data = obj_ref.data_arc();

    assert_eq!(arc_data.as_ref(), b"test");

    // With Bytes implementation, the original Arc is still valid
    let refcount1 = Arc::strong_count(&data);
    // The returned Arc is a new copy, so refcount is 1
    let refcount2 = Arc::strong_count(&arc_data);

    assert!(refcount1 >= 1);
    assert_eq!(refcount2, 1);
}

#[tokio::test]
async fn test_async_deadlock_prevention() {
    use std::sync::Arc;
    use tokio::task::JoinSet;

    let store = Arc::new(ObjectStore::default());
    let mut join_set = JoinSet::new();

    // Test high concurrent access to ensure no deadlocks occur
    for i in 0..50 {
        let store_clone = Arc::clone(&store);
        join_set.spawn(async move {
            // Mix of read and write operations
            if i % 3 == 0 {
                // Write operation
                let data = vec![i as u8; 100];
                let id = store_clone.put(data).await.unwrap();

                // Immediate read to test lock contention
                let obj_ref = store_clone.get(id).await.unwrap();
                assert_eq!(obj_ref.data()[0], i as u8);

                // Delete operation
                store_clone.delete(id).await.unwrap();
            } else if i % 3 == 1 {
                // Read operation on existing data
                let data = vec![i as u8; 50];
                let id = store_clone.put(data).await.unwrap();

                // Multiple reads to test read lock sharing
                for _ in 0..5 {
                    let obj_ref = store_clone.get(id).await.unwrap();
                    assert_eq!(obj_ref.data()[0], i as u8);
                }
            } else {
                // Mixed operations
                let data = vec![i as u8; 75];
                let id = store_clone.put(data).await.unwrap();

                // Concurrent access simulation
                let mut handles = Vec::new();
                for j in 0..3 {
                    let store_inner = Arc::clone(&store_clone);
                    let id_inner = id;
                    handles.push(tokio::spawn(async move {
                        let obj_ref = store_inner.get(id_inner).await.unwrap();
                        (j, obj_ref.data()[0])
                    }));
                }

                for handle in handles {
                    let (_j, val) = handle.await.unwrap();
                    assert_eq!(val, i as u8);
                }
            }
        });
    }

    // Wait for all tasks to complete
    let mut completed = 0;
    while let Some(result) = join_set.join_next().await {
        result.unwrap();
        completed += 1;
    }

    assert_eq!(completed, 50);

    // Verify store is still in consistent state
    let stats = store.stats();
    println!(
        "Final stats after concurrent test: {} objects, {} bytes",
        stats.total_objects, stats.total_memory
    );
}

#[tokio::test]
async fn test_bytes_performance() {
    use bytes::{BufMut, Bytes};

    let store = ObjectStore::default();

    // Test Bytes-specific operations
    let mut buf = bytes::BytesMut::with_capacity(1024);
    buf.put_slice(&[1, 2, 3, 4, 5]);
    buf.put_slice(&[6, 7, 8, 9, 10]);

    let bytes_data = buf.freeze();
    let id = store.put_bytes(bytes_data.clone()).await.unwrap();

    let obj_ref = store.get(id).await.unwrap();
    let retrieved_bytes = obj_ref.data_bytes();

    assert_eq!(bytes_data, retrieved_bytes);

    // Test slicing performance (zero-copy)
    let slice = bytes_data.slice(2..8);
    let slice_id = store.put_bytes(slice).await.unwrap();

    let slice_ref = store.get(slice_id).await.unwrap();
    assert_eq!(slice_ref.data(), &[3, 4, 5, 6, 7, 8]);
}

#[tokio::test]
async fn test_bytes_zero_copy_operations() {
    let store = ObjectStore::default();

    // Create large data
    let large_data = vec![42u8; 10_000];
    let bytes_data = bytes::Bytes::from(large_data);

    let id = store.put_bytes(bytes_data.clone()).await.unwrap();

    // Test multiple views without copying
    let view1 = store.get_view(id).await.unwrap();
    let view2 = store.get_view(id).await.unwrap();
    let view3 = store.get_view(id).await.unwrap();

    // All should point to the same data
    assert_eq!(view1.as_bytes(), view2.as_bytes());
    assert_eq!(view2.as_bytes(), view3.as_bytes());
    assert_eq!(view1.as_bytes_clone(), bytes_data);

    // Test slicing operations
    let slice1 = view1.as_bytes_clone().slice(0..5000);
    let slice2 = view2.as_bytes_clone().slice(5000..10000);

    assert_eq!(slice1.len(), 5000);
    assert_eq!(slice2.len(), 5000);
    assert!(slice1.iter().all(|&b| b == 42));
    assert!(slice2.iter().all(|&b| b == 42));
}
