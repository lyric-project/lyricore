#[cfg(test)]
mod mmap_tests {
    use lyricore_store::*;
    use std::fs::File;
    use std::io::Write;
    use std::sync::Arc;
    use tempfile::TempDir;

    /// Helper function: Create test file
    fn create_test_file(content: &[u8]) -> (TempDir, std::path::PathBuf) {
        let temp_dir = TempDir::new().expect("Failed to create temp directory");
        let file_path = temp_dir.path().join("test_file.bin");

        let mut file = File::create(&file_path).expect("Failed to create test file");
        file.write_all(content).expect("Failed to write test data");
        file.sync_all().expect("Failed to sync file");

        (temp_dir, file_path)
    }

    #[cfg(feature = "mmap")]
    #[test]
    fn test_mmap_basic_functionality() {
        use memmap2::Mmap;

        // Create test data
        let test_data = b"Hello, mmap world! This is test data for memory mapping.";
        let (_temp_dir, file_path) = create_test_file(test_data);

        // Create memory mapping
        let file = File::open(&file_path).expect("Failed to open test file");
        let mmap = unsafe { Mmap::map(&file).expect("Failed to create mmap") };
        let mmap_arc = Arc::new(mmap);

        // Test StoredObject::from_mmap
        let id = ObjectId::new();
        let metadata = ObjectMetadata::default();
        let stored_obj = StoredObject::from_mmap(id, mmap_arc.clone(), metadata);

        // Verify basic properties
        assert_eq!(stored_obj.id, id);
        assert_eq!(stored_obj.size, test_data.len());
        assert_eq!(stored_obj.storage_type(), "memory_mapped");
        assert!(stored_obj.is_zero_copy());

        // Verify data content
        assert_eq!(stored_obj.data_slice(), test_data);

        // Verify Arc reference count
        assert_eq!(Arc::strong_count(&mmap_arc), 2); // Original + reference in StoredObject
    }

    #[cfg(feature = "mmap")]
    #[test]
    fn test_mmap_zero_copy_access() {
        use memmap2::Mmap;

        let test_data = b"Zero copy test data for memory mapped file access verification.";
        let (_temp_dir, file_path) = create_test_file(test_data);

        let file = File::open(&file_path).expect("Failed to open test file");
        let mmap = unsafe { Mmap::map(&file).expect("Failed to create mmap") };
        let mmap_arc = Arc::new(mmap);

        let stored_obj =
            StoredObject::from_mmap(ObjectId::new(), mmap_arc.clone(), ObjectMetadata::default());

        // Test zero-copy access
        let data_slice = stored_obj.data_slice();
        assert_eq!(data_slice, test_data);

        // Verify pointer addresses are identical (true zero-copy)
        let mmap_ptr = mmap_arc.as_ptr();
        let stored_ptr = data_slice.as_ptr();
        assert_eq!(
            mmap_ptr, stored_ptr,
            "Pointers should be identical for zero-copy access"
        );

        // Test multiple accesses
        let data_slice2 = stored_obj.data_slice();
        assert_eq!(data_slice.as_ptr(), data_slice2.as_ptr());
    }

    #[cfg(feature = "mmap")]
    #[test]
    fn test_mmap_with_metadata() {
        use memmap2::Mmap;

        // Create mock NumPy array data
        let numpy_data = vec![1u8, 2, 3, 4, 5, 6, 7, 8]; // 2x4 uint8 array
        let (_temp_dir, file_path) = create_test_file(&numpy_data);

        let file = File::open(&file_path).expect("Failed to open test file");
        let mmap = unsafe { Mmap::map(&file).expect("Failed to create mmap") };
        let mmap_arc = Arc::new(mmap);

        // Create stored object with NumPy metadata
        let metadata = ObjectMetadata {
            data_type: DataType::NumpyArray {
                dtype: "uint8".to_string(),
                order: 'C',
            },
            shape: Some(vec![2, 4]),
            strides: None,
            alignment: 1,
            track_access_time: true,
        };

        let stored_obj = StoredObject::from_mmap(ObjectId::new(), mmap_arc, metadata);

        // Verify metadata
        match &stored_obj.metadata.data_type {
            DataType::NumpyArray { dtype, order } => {
                assert_eq!(dtype, "uint8");
                assert_eq!(*order, 'C');
            }
            _ => panic!("Expected NumpyArray data type"),
        }

        assert_eq!(stored_obj.metadata.shape, Some(vec![2, 4]));
        assert_eq!(stored_obj.data_slice(), &numpy_data[..]);
    }

    #[cfg(feature = "mmap")]
    #[tokio::test]
    async fn test_mmap_with_object_store() {
        let test_data = b"Integration test data for ObjectStore with memory mapping support.";
        let (_temp_dir, file_path) = create_test_file(test_data);

        let store = ObjectStore::default();

        // Use ObjectStore's mmap functionality
        let id = store
            .put_mmap_zero_copy(&file_path)
            .await
            .expect("Failed to put mmap file");

        // Verify object exists
        assert!(store.contains(id).await);

        // Get object reference
        let obj_ref = store.get(id).await.expect("Failed to get object");
        assert_eq!(obj_ref.data(), test_data);
        assert_eq!(obj_ref.size(), test_data.len());
        assert_eq!(obj_ref.storage_type(), "memory_mapped");
        assert!(obj_ref.is_zero_copy());

        // Test view access
        let view = store.get_view(id).await.expect("Failed to get view");
        assert_eq!(view.as_bytes(), test_data);

        // Verify statistics
        let stats = store.stats();
        assert_eq!(stats.total_objects, 1);
        assert_eq!(stats.total_memory, test_data.len());

        // Get storage information
        let storage_info = store
            .get_storage_info(id)
            .await
            .expect("Failed to get storage info");
        assert_eq!(storage_info.storage_type, "memory_mapped");
        assert!(storage_info.is_zero_copy);
    }

    #[cfg(feature = "mmap")]
    #[test]
    fn test_mmap_large_file() {
        use memmap2::Mmap;

        // Create larger test file (1MB)
        let large_data: Vec<u8> = (0..1024 * 1024).map(|i| (i % 256) as u8).collect();
        let (_temp_dir, file_path) = create_test_file(&large_data);

        let file = File::open(&file_path).expect("Failed to open large test file");
        let mmap = unsafe { Mmap::map(&file).expect("Failed to create large mmap") };
        let mmap_arc = Arc::new(mmap);

        let stored_obj =
            StoredObject::from_mmap(ObjectId::new(), mmap_arc, ObjectMetadata::default());

        // Verify large file handling
        assert_eq!(stored_obj.size, large_data.len());
        assert_eq!(stored_obj.data_slice().len(), large_data.len());

        // Verify data integrity (spot checks)
        let data_slice = stored_obj.data_slice();
        assert_eq!(data_slice[0], 0);
        assert_eq!(data_slice[255], 255);
        assert_eq!(data_slice[256], 0);
        assert_eq!(data_slice[1024 * 1024 - 1], ((1024 * 1024 - 1) % 256) as u8);
    }

    #[cfg(feature = "mmap")]
    #[tokio::test]
    async fn test_mmap_concurrent_access() {
        use memmap2::Mmap;
        use tokio::task::JoinSet;

        let test_data = b"Concurrent access test data for memory mapped file operations.";
        let (_temp_dir, file_path) = create_test_file(test_data);

        let file = File::open(&file_path).expect("Failed to open test file");
        let mmap = unsafe { Mmap::map(&file).expect("Failed to create mmap") };
        let mmap_arc = Arc::new(mmap);

        let store = Arc::new(ObjectStore::default());

        // Create multiple mmap objects
        let mut ids = Vec::new();
        for i in 0..5 {
            let metadata = ObjectMetadata {
                data_type: DataType::Custom(format!("test_type_{}", i)),
                ..Default::default()
            };
            let obj = StoredObject::from_mmap(ObjectId::new(), mmap_arc.clone(), metadata);
            let id = store
                .put_object(obj)
                .await
                .expect("Failed to put mmap object");
            ids.push(id);
        }

        // Concurrent access test
        let mut join_set = JoinSet::new();

        for (i, id) in ids.into_iter().enumerate() {
            let store_clone = Arc::clone(&store);
            join_set.spawn(async move {
                // Read the same object multiple times
                for _ in 0..10 {
                    let obj_ref = store_clone.get(id).await.expect("Failed to get object");
                    assert_eq!(obj_ref.data(), test_data);
                    assert_eq!(obj_ref.storage_type(), "memory_mapped");

                    // Verify metadata
                    if let DataType::Custom(type_name) = &obj_ref.metadata().data_type {
                        assert_eq!(type_name, &format!("test_type_{}", i));
                    }
                }
                id
            });
        }

        // Wait for all tasks to complete
        let mut completed_ids = Vec::new();
        while let Some(result) = join_set.join_next().await {
            completed_ids.push(result.expect("Task should not panic"));
        }

        assert_eq!(completed_ids.len(), 5);

        // Verify mmap reference count
        assert_eq!(Arc::strong_count(&mmap_arc), 6); // Original + 5 stored objects
    }

    #[cfg(feature = "mmap")]
    #[tokio::test]
    async fn test_mmap_cleanup_and_deletion() {
        use memmap2::Mmap;

        let test_data = b"Cleanup test data for memory mapped file object deletion.";
        let (_temp_dir, file_path) = create_test_file(test_data);

        let file = File::open(&file_path).expect("Failed to open test file");
        let mmap = unsafe { Mmap::map(&file).expect("Failed to create mmap") };
        let mmap_arc = Arc::new(mmap);
        let initial_refcount = Arc::strong_count(&mmap_arc);

        let store = ObjectStore::default();

        // Create and store mmap object
        let obj =
            StoredObject::from_mmap(ObjectId::new(), mmap_arc.clone(), ObjectMetadata::default());
        let id = obj.id;
        store
            .put_object(obj)
            .await
            .expect("Failed to put mmap object");

        // Verify reference count increased
        assert_eq!(Arc::strong_count(&mmap_arc), initial_refcount + 1);

        // Verify object exists
        assert!(store.contains(id).await);
        let stats = store.stats();
        assert_eq!(stats.total_objects, 1);

        // Delete object
        store.delete(id).await.expect("Failed to delete object");

        // Verify object was deleted
        assert!(!store.contains(id).await);
        let stats = store.stats();
        assert_eq!(stats.total_objects, 0);

        // Verify reference count restored
        assert_eq!(Arc::strong_count(&mmap_arc), initial_refcount);
    }

    #[cfg(feature = "mmap")]
    #[test]
    fn test_mmap_error_handling() {
        use std::path::Path;

        let store = ObjectStore::default();

        // Test non-existent file
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(store.put_mmap_zero_copy(Path::new("/nonexistent/file.bin")));
        assert!(result.is_err());

        if let Err(StoreError::Internal(msg)) = result {
            assert!(msg.contains("Failed to open file"));
        } else {
            panic!("Expected Internal error with file open message");
        }
    }

    #[cfg(feature = "mmap")]
    #[test]
    fn test_storage_data_methods() {
        use memmap2::Mmap;

        let test_data = b"Testing StorageData methods with memory mapped data.";
        let (_temp_dir, file_path) = create_test_file(test_data);

        let file = File::open(&file_path).expect("Failed to open test file");
        let mmap = unsafe { Mmap::map(&file).expect("Failed to create mmap") };
        let mmap_arc = Arc::new(mmap);

        let storage_data = StorageData::MemoryMapped { _mmap: mmap_arc };

        // Test StorageData methods
        assert_eq!(storage_data.len(), test_data.len());
        assert_eq!(storage_data.as_slice(), test_data);
        assert_eq!(storage_data.storage_type(), "memory_mapped");
        assert!(storage_data.is_zero_copy());

        // Test to_bytes() method
        let bytes = storage_data.to_bytes();
        assert_eq!(bytes.len(), test_data.len());
        assert_eq!(&bytes[..], test_data);
    }

    #[cfg(feature = "mmap")]
    #[tokio::test]
    async fn test_mmap_with_zero_copy_stats() {
        use memmap2::Mmap;

        let test_data = b"Zero copy statistics test for memory mapped objects.";
        let (_temp_dir, file_path) = create_test_file(test_data);

        let file = File::open(&file_path).expect("Failed to open test file");
        let mmap = unsafe { Mmap::map(&file).expect("Failed to create mmap") };
        let mmap_arc = Arc::new(mmap);

        let store = ObjectStore::default();

        // Add standard object
        let standard_id = store
            .put(vec![1, 2, 3, 4, 5])
            .await
            .expect("Failed to put standard object");

        // Add mmap object
        let obj = StoredObject::from_mmap(ObjectId::new(), mmap_arc, ObjectMetadata::default());
        let mmap_id = store
            .put_object(obj)
            .await
            .expect("Failed to put mmap object");

        // Get zero-copy statistics
        let zero_copy_stats = store.get_zero_copy_stats().await;

        assert_eq!(zero_copy_stats.total_objects, 2);
        assert_eq!(zero_copy_stats.standard_objects, 1);
        assert_eq!(zero_copy_stats.zero_copy_objects, 1); // mmap counted as zero-copy
        assert_eq!(zero_copy_stats.zero_copy_memory_bytes, test_data.len());
        assert_eq!(zero_copy_stats.zero_copy_ratio, 0.5);

        // Verify storage information
        let mmap_info = store
            .get_storage_info(mmap_id)
            .await
            .expect("Failed to get mmap storage info");
        assert_eq!(mmap_info.storage_type, "memory_mapped");
        assert!(mmap_info.is_zero_copy);

        let standard_info = store
            .get_storage_info(standard_id)
            .await
            .expect("Failed to get standard storage info");
        assert_eq!(standard_info.storage_type, "standard");
        assert!(!standard_info.is_zero_copy);
    }

    #[cfg(feature = "mmap")]
    #[test]
    fn test_mmap_debug_display() {
        use memmap2::Mmap;

        let test_data = b"Debug display test data.";
        let (_temp_dir, file_path) = create_test_file(test_data);

        let file = File::open(&file_path).expect("Failed to open test file");
        let mmap = unsafe { Mmap::map(&file).expect("Failed to create mmap") };
        let mmap_arc = Arc::new(mmap);

        let stored_obj =
            StoredObject::from_mmap(ObjectId::new(), mmap_arc, ObjectMetadata::default());

        // Test Debug formatting
        let debug_output = format!("{:?}", stored_obj);
        assert!(debug_output.contains("StoredObject"));
        assert!(debug_output.contains("memory_mapped"));
        assert!(debug_output.contains("is_zero_copy: true"));

        // Test ObjectRef Debug output
        let obj_ref = ObjectRef::new(Arc::new(stored_obj));
        let ref_debug = format!("{:?}", obj_ref);
        assert!(ref_debug.contains("ObjectRef"));
    }
}
