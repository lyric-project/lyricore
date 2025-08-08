use crate::fast_ops::{ExternalDataKeeper, FastCopyMode, FastMemOps};
use crate::storage_data::StorageData;
use crate::{DataType, ObjectId, ObjectMetadata, StoreError, StoredObject};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize};
use std::sync::Arc;
use tokio::sync::RwLock;

pub struct ObjectRef {
    inner: Arc<StoredObject>,
}

impl ObjectRef {
    pub fn new(inner: Arc<StoredObject>) -> Self {
        Self { inner }
    }

    pub fn data(&self) -> &[u8] {
        self.update_access_time();
        self.inner.data_slice()
    }

    pub fn data_bytes(&self) -> bytes::Bytes {
        self.update_access_time();
        self.inner.data_bytes()
    }

    // Backward compatibility method
    pub fn data_arc(&self) -> Arc<[u8]> {
        self.update_access_time();
        self.inner.data_slice().into()
    }

    pub fn as_ptr(&self) -> *const u8 {
        self.update_access_time();
        self.inner.data.as_ptr()
    }

    pub fn as_mut_ptr(&mut self) -> Option<*mut u8> {
        // Bytes doesn't expose mutability, so we return None for now
        // This could be implemented with BytesMut if needed
        None
    }

    pub fn metadata(&self) -> &ObjectMetadata {
        &self.inner.metadata
    }

    pub fn size(&self) -> usize {
        self.inner.size
    }

    pub fn id(&self) -> ObjectId {
        self.inner.id
    }
    pub fn storage_type(&self) -> &'static str {
        self.inner.storage_type()
    }
    pub fn is_zero_copy(&self) -> bool {
        self.inner.is_zero_copy()
    }
    fn update_access_time(&self) {
        if self.inner.metadata.track_access_time {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            self.inner
                .last_accessed
                .store(now, std::sync::atomic::Ordering::Relaxed);
        }
    }
}

impl std::fmt::Debug for ObjectRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ObjectRef")
            .field("id", &self.inner.id)
            .field("size", &self.inner.size)
            .field("refcount", &"N/A (Bytes)")
            .field("metadata", &self.inner.metadata)
            .finish()
    }
}

impl Clone for ObjectRef {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

pub struct ObjectBuilder {
    data: Option<StorageData>,
    metadata: ObjectMetadata,
}

impl ObjectBuilder {
    pub fn new() -> Self {
        Self {
            data: None,
            metadata: ObjectMetadata::default(),
        }
    }

    pub fn from_vec(mut self, data: Vec<u8>) -> Self {
        self.data = Some(data.into());
        self
    }

    pub fn from_arc(mut self, data: Arc<[u8]>) -> Self {
        self.data = Some(StorageData::from(data));
        self
    }

    /// Zero-copy construction from an external reference
    ///
    /// # Safety
    /// The caller must ensure that the pointer remains valid for the lifetime of the `keeper`.
    pub unsafe fn from_external_ref<T: 'static + Send + Sync>(
        mut self,
        ptr: *const u8,
        len: usize,
        keeper: T,
    ) -> Self {
        let slice = std::slice::from_raw_parts(ptr, len);
        let bytes = bytes::Bytes::from_static(slice);
        self.data = Some(StorageData::ExternalRef {
            data: bytes,
            _keeper: Box::new(ExternalDataKeeper::new(keeper)),
        });
        self
    }
    /// Fast copy construction
    ///
    /// # Safety
    /// Caller must ensure that the pointer is valid and readable.
    pub unsafe fn from_fast_copy(
        mut self,
        ptr: *const u8,
        len: usize,
        copy_mode: FastCopyMode,
    ) -> Self {
        let data = FastMemOps::fast_copy(ptr, len, copy_mode);
        self.data = Some(StorageData::from(data));
        self
    }
    pub unsafe fn from_raw_parts(mut self, ptr: *const u8, len: usize, capacity: usize) -> Self {
        let vec = Vec::from_raw_parts(ptr as *mut u8, len, capacity);
        self.data = Some(vec.into());
        self
    }

    pub fn with_metadata(mut self, metadata: ObjectMetadata) -> Self {
        self.metadata = metadata;
        self
    }

    pub fn with_track_access_time(mut self, track: bool) -> Self {
        self.metadata.track_access_time = track;
        self
    }

    pub fn with_numpy_metadata(mut self, dtype: String, shape: Vec<usize>, order: char) -> Self {
        self.metadata = ObjectMetadata {
            data_type: DataType::NumpyArray { dtype, order },
            shape: Some(shape),
            strides: None,
            alignment: 8,
            track_access_time: true,
        };
        self
    }

    pub fn with_arrow_metadata(mut self, alignment: usize) -> Self {
        self.metadata = ObjectMetadata {
            data_type: DataType::ArrowBuffer,
            shape: None,
            strides: None,
            alignment,
            track_access_time: true,
        };
        self
    }

    pub fn from_bytes(mut self, data: bytes::Bytes) -> Self {
        self.data = Some(data.into());
        self
    }
    pub fn from_storage_data(mut self, data: StorageData) -> Self {
        self.data = Some(data);
        self
    }
    pub fn build(self) -> Result<StoredObject, StoreError> {
        let data = self
            .data
            .ok_or(StoreError::Internal("No data provided".into()))?;
        let size = data.len();

        Ok(StoredObject {
            id: ObjectId::new(),
            size,
            data,
            created_at: std::time::Instant::now(),
            last_accessed: AtomicU64::new(0),
            metadata: self.metadata,
        })
    }
}

impl Default for ObjectBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Default)]
pub struct StoreStats {
    pub total_objects: AtomicUsize,
    pub total_memory: AtomicUsize,
    pub cache_hits: AtomicU64,
    pub cache_misses: AtomicU64,
}

#[derive(Debug, Clone)]
pub struct StoreConfig {
    pub max_memory: usize,
    pub max_object_size: usize,
    pub memory_pressure_threshold: f32,
    pub track_access_time: bool,
}

impl Default for StoreConfig {
    fn default() -> Self {
        Self {
            max_memory: 1024 * 1024 * 1024,
            max_object_size: 64 * 1024 * 1024,
            memory_pressure_threshold: 0.8,
            track_access_time: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct StoreStatsSnapshot {
    pub total_objects: usize,
    pub total_memory: usize,
    pub cache_hits: u64,
    pub cache_misses: u64,
}

impl StoreStatsSnapshot {
    pub fn hit_rate(&self) -> f64 {
        let total = self.cache_hits + self.cache_misses;
        if total == 0 {
            0.0
        } else {
            self.cache_hits as f64 / total as f64
        }
    }

    pub fn memory_usage_mb(&self) -> f64 {
        self.total_memory as f64 / (1024.0 * 1024.0)
    }
}

pub struct ObjectStore {
    objects: RwLock<HashMap<ObjectId, Arc<StoredObject>>>,
    stats: Arc<StoreStats>,
    config: StoreConfig,
}

impl ObjectStore {
    pub fn new(config: StoreConfig) -> Self {
        Self {
            objects: RwLock::new(HashMap::new()),
            stats: Arc::new(StoreStats::default()),
            config,
        }
    }

    /// Zero-copy(store external data reference)
    ///
    /// # Safety
    /// Caller must ensure that the pointer remains valid for the lifetime of the `keeper`.
    pub async unsafe fn put_external_ref<T: 'static + Send + Sync>(
        &self,
        ptr: usize,
        len: usize,
        keeper: T,
    ) -> Result<ObjectId, StoreError> {
        let obj = StoredObject::from_external_ref(
            ObjectId::new(),
            ptr as *const u8,
            len,
            keeper,
            ObjectMetadata::default(),
        );
        self.put_object(obj).await
    }

    /// Zero-copy NumPy array storage
    ///
    /// # Safety
    /// caller must ensure that the pointer remains valid for the lifetime of the `keeper`.
    pub async unsafe fn put_numpy_external_ref<T: 'static + Send + Sync>(
        &self,
        ptr: usize,
        len: usize,
        dtype: String,
        shape: Vec<usize>,
        order: char,
        keeper: T,
    ) -> Result<ObjectId, StoreError> {
        let metadata = ObjectMetadata {
            data_type: DataType::NumpyArray { dtype, order },
            shape: Some(shape),
            strides: None,
            alignment: 8,
            track_access_time: true,
        };

        let obj = StoredObject::from_external_ref(
            ObjectId::new(),
            ptr as *const u8,
            len,
            keeper,
            metadata,
        );
        self.put_object(obj).await
    }

    /// High-performance zero-copy storage
    ///
    /// # Safety
    /// Caller must ensure that the pointer is valid and readable
    pub async unsafe fn put_fast_copy(
        &self,
        ptr: *const u8,
        len: usize,
        copy_mode: FastCopyMode,
    ) -> Result<ObjectId, StoreError> {
        let data = FastMemOps::fast_copy(ptr, len, copy_mode);
        self.put(data).await
    }

    /// High-performance NumPy array storage with fast copy
    ///
    /// # Safety
    /// Caller must ensure that the pointer is valid and readable
    pub async unsafe fn put_numpy_fast_copy(
        &self,
        ptr: *const u8,
        len: usize,
        dtype: String,
        shape: Vec<usize>,
        order: char,
        copy_mode: FastCopyMode,
    ) -> Result<ObjectId, StoreError> {
        let data = FastMemOps::fast_copy(ptr, len, copy_mode);
        self.put_numpy(data, dtype, shape, order).await
    }

    /// Batch storage of objects
    ///
    /// # Safety
    /// Caller must ensure that all pointers are valid and readable
    pub async unsafe fn put_batch_external_ref<T: 'static + Send + Sync + Clone>(
        &self,
        data_list: Vec<(*const u8, usize)>,
        keeper: T,
    ) -> Result<Vec<ObjectId>, StoreError> {
        let mut objects = Vec::with_capacity(data_list.len());

        for (ptr, len) in data_list {
            let obj = StoredObject::from_external_ref(
                ObjectId::new(),
                ptr,
                len,
                keeper.clone(),
                ObjectMetadata::default(),
            );
            objects.push(obj);
        }

        self.put_batch_objects(objects).await
    }

    /// High-performance batch storage with fast copy
    ///
    /// # Safety
    /// Caller must ensure that all pointers are valid and readable
    pub async unsafe fn put_batch_fast_copy(
        &self,
        data_list: Vec<(*const u8, usize)>,
        copy_mode: FastCopyMode,
    ) -> Result<Vec<ObjectId>, StoreError> {
        let mut objects = Vec::with_capacity(data_list.len());

        for (ptr, len) in data_list {
            let data = FastMemOps::fast_copy(ptr, len, copy_mode);
            let obj = ObjectBuilder::new().from_vec(data).build()?;
            objects.push(obj);
        }

        self.put_batch_objects(objects).await
    }
    /// Memory-mapped file storage
    #[cfg(feature = "mmap")]
    pub async fn put_mmap_zero_copy(
        &self,
        file_path: &std::path::Path,
    ) -> Result<ObjectId, StoreError> {
        use memmap2::Mmap;
        use std::fs::File;

        let file = File::open(file_path)
            .map_err(|e| StoreError::Internal(format!("Failed to open file: {}", e)))?;

        let mmap = unsafe { Mmap::map(&file) }
            .map_err(|e| StoreError::Internal(format!("Failed to mmap file: {}", e)))?;

        let obj =
            StoredObject::from_mmap(ObjectId::new(), Arc::new(mmap), ObjectMetadata::default());

        self.put_object(obj).await
    }
    /// Get storage information for a single object
    pub async fn get_storage_info(&self, id: ObjectId) -> Result<StorageInfo, StoreError> {
        let objects = self.objects.read().await;

        match objects.get(&id) {
            Some(obj) => Ok(StorageInfo {
                id,
                size: obj.size,
                storage_type: obj.storage_type(),
                is_zero_copy: obj.is_zero_copy(),
                data_type: obj.metadata.data_type.clone(),
                shape: obj.metadata.shape.clone(),
                alignment: obj.metadata.alignment,
                created_at: obj.created_at,
                last_accessed: obj.last_accessed.load(std::sync::atomic::Ordering::Relaxed),
            }),
            None => Err(StoreError::ObjectNotFound(id)),
        }
    }
    /// Get storage information for a batch of objects
    pub async fn get_storage_info_batch(
        &self,
        ids: &[ObjectId],
    ) -> Vec<Result<StorageInfo, StoreError>> {
        let mut results = Vec::with_capacity(ids.len());
        for &id in ids {
            results.push(self.get_storage_info(id).await);
        }
        results
    }

    pub async fn put(&self, data: Vec<u8>) -> Result<ObjectId, StoreError> {
        let obj = ObjectBuilder::new().from_vec(data).build()?;
        self.put_object(obj).await
    }

    pub async fn put_arc(&self, data: Arc<[u8]>) -> Result<ObjectId, StoreError> {
        let obj = ObjectBuilder::new().from_arc(data).build()?;
        self.put_object(obj).await
    }

    pub async fn put_bytes(&self, data: bytes::Bytes) -> Result<ObjectId, StoreError> {
        let obj = ObjectBuilder::new().from_bytes(data).build()?;
        self.put_object(obj).await
    }

    pub async fn put_numpy(
        &self,
        data: Vec<u8>,
        dtype: String,
        shape: Vec<usize>,
        order: char,
    ) -> Result<ObjectId, StoreError> {
        let obj = ObjectBuilder::new()
            .from_vec(data)
            .with_numpy_metadata(dtype, shape, order)
            .build()?;
        self.put_object(obj).await
    }

    pub async fn put_arrow(
        &self,
        data: Arc<[u8]>,
        alignment: usize,
    ) -> Result<ObjectId, StoreError> {
        let obj = ObjectBuilder::new()
            .from_arc(data)
            .with_arrow_metadata(alignment)
            .build()?;
        self.put_object(obj).await
    }

    pub async fn put_arrow_bytes(
        &self,
        data: bytes::Bytes,
        alignment: usize,
    ) -> Result<ObjectId, StoreError> {
        let obj = ObjectBuilder::new()
            .from_bytes(data)
            .with_arrow_metadata(alignment)
            .build()?;
        self.put_object(obj).await
    }

    pub async fn put_object(&self, obj: StoredObject) -> Result<ObjectId, StoreError> {
        if obj.size > self.config.max_object_size {
            return Err(StoreError::ObjectTooLarge {
                size: obj.size,
                max_size: self.config.max_object_size,
            });
        }

        self.check_memory_pressure().await?;

        let object_id = obj.id;
        let size = obj.size;

        {
            let mut objects = self.objects.write().await;
            objects.insert(object_id, Arc::new(obj));
        }

        self.stats
            .total_objects
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.stats
            .total_memory
            .fetch_add(size, std::sync::atomic::Ordering::Relaxed);

        Ok(object_id)
    }
    /// Store a batch of objects
    pub async fn put_shared_optimized(
        &self,
        shared_data: std::sync::Arc<[u8]>,
        count: usize,
        metadata_list: Option<Vec<ObjectMetadata>>,
    ) -> Result<Vec<ObjectId>, StoreError> {
        let metadata_list = metadata_list.unwrap_or_else(|| vec![ObjectMetadata::default(); count]);

        if metadata_list.len() != count {
            return Err(StoreError::Internal(
                "Metadata list length must match count".into(),
            ));
        }

        let mut objects = Vec::with_capacity(count);
        for metadata in metadata_list {
            let obj = StoredObject::from_arc(ObjectId::new(), shared_data.clone(), metadata);
            objects.push(obj);
        }

        self.put_batch_objects(objects).await
    }
    pub async fn get(&self, id: ObjectId) -> Result<ObjectRef, StoreError> {
        let objects = self.objects.read().await;

        match objects.get(&id) {
            Some(obj) => {
                self.stats
                    .cache_hits
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                Ok(ObjectRef::new(Arc::clone(obj)))
            }
            None => {
                self.stats
                    .cache_misses
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                Err(StoreError::ObjectNotFound(id))
            }
        }
    }
    pub async fn get_batch(&self, ids: &[ObjectId]) -> Vec<Result<ObjectRef, StoreError>> {
        use futures::future::join_all;

        let futures = ids.iter().map(|&id| self.get(id));
        join_all(futures).await
    }
    pub async fn delete(&self, id: ObjectId) -> Result<(), StoreError> {
        let mut objects = self.objects.write().await;

        match objects.remove(&id) {
            Some(obj) => {
                self.stats
                    .total_objects
                    .fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
                self.stats
                    .total_memory
                    .fetch_sub(obj.size, std::sync::atomic::Ordering::Relaxed);
                Ok(())
            }
            None => Err(StoreError::ObjectNotFound(id)),
        }
    }

    pub async fn contains(&self, id: ObjectId) -> bool {
        let objects = self.objects.read().await;
        objects.contains_key(&id)
    }

    /// Check if a batch of IDs exists in the store
    pub async fn contains_batch(&self, ids: &[ObjectId]) -> Vec<bool> {
        let objects = self.objects.read().await;
        ids.iter().map(|id| objects.contains_key(id)).collect()
    }
    pub fn stats(&self) -> StoreStatsSnapshot {
        StoreStatsSnapshot {
            total_objects: self
                .stats
                .total_objects
                .load(std::sync::atomic::Ordering::Relaxed),
            total_memory: self
                .stats
                .total_memory
                .load(std::sync::atomic::Ordering::Relaxed),
            cache_hits: self
                .stats
                .cache_hits
                .load(std::sync::atomic::Ordering::Relaxed),
            cache_misses: self
                .stats
                .cache_misses
                .load(std::sync::atomic::Ordering::Relaxed),
        }
    }

    pub async fn put_shared(
        &self,
        shared_data: Arc<[u8]>,
        count: usize,
    ) -> Result<Vec<ObjectId>, StoreError> {
        let mut ids = Vec::with_capacity(count);

        for _ in 0..count {
            let obj = ObjectBuilder::new()
                .from_arc(Arc::clone(&shared_data))
                .build()?;
            ids.push(self.put_object(obj).await?);
        }

        Ok(ids)
    }

    pub async fn put_shared_bytes(
        &self,
        shared_data: bytes::Bytes,
        count: usize,
    ) -> Result<Vec<ObjectId>, StoreError> {
        let mut ids = Vec::with_capacity(count);

        for _ in 0..count {
            let obj = ObjectBuilder::new()
                .from_bytes(shared_data.clone())
                .build()?;
            ids.push(self.put_object(obj).await?);
        }

        Ok(ids)
    }

    pub async fn get_view(&self, id: ObjectId) -> Result<crate::views::DataView, StoreError> {
        let obj_ref = self.get(id).await?;
        Ok(crate::views::DataView::new(obj_ref))
    }

    pub async fn get_raw_ptr(&self, id: ObjectId) -> Result<(*const u8, usize), StoreError> {
        let obj_ref = self.get(id).await?;
        Ok((obj_ref.as_ptr(), obj_ref.size()))
    }

    pub async fn get_views(
        &self,
        ids: &[ObjectId],
    ) -> Vec<Result<crate::views::DataView, StoreError>> {
        let mut results = Vec::with_capacity(ids.len());
        for &id in ids {
            results.push(self.get_view(id).await);
        }
        results
    }

    pub async fn get_arcs(
        &self,
        ids: &[ObjectId],
    ) -> Vec<Result<std::sync::Arc<[u8]>, StoreError>> {
        let mut results = Vec::with_capacity(ids.len());
        for &id in ids {
            results.push(self.get(id).await.map(|obj_ref| obj_ref.data_arc()));
        }
        results
    }

    pub async fn get_bytes(&self, ids: &[ObjectId]) -> Vec<Result<bytes::Bytes, StoreError>> {
        let mut results = Vec::with_capacity(ids.len());
        for &id in ids {
            results.push(self.get(id).await.map(|obj_ref| obj_ref.data_bytes()));
        }
        results
    }

    pub async fn get_raw_ptrs(
        &self,
        ids: &[ObjectId],
    ) -> Vec<Result<(*const u8, usize), StoreError>> {
        let mut results = Vec::with_capacity(ids.len());
        for &id in ids {
            results.push(self.get_raw_ptr(id).await);
        }
        results
    }

    #[cfg(feature = "mmap")]
    pub async fn put_mmap(&self, file_path: &std::path::Path) -> Result<ObjectId, StoreError> {
        use memmap2::Mmap;
        use std::fs::File;

        let file = File::open(file_path)
            .map_err(|e| StoreError::Internal(format!("Failed to open file: {}", e)))?;

        let mmap = unsafe { Mmap::map(&file) }
            .map_err(|e| StoreError::Internal(format!("Failed to mmap file: {}", e)))?;

        let data: bytes::Bytes = bytes::Bytes::copy_from_slice(&mmap[..]);

        self.put_bytes(data).await
    }

    async fn check_memory_pressure(&self) -> Result<(), StoreError> {
        let current_memory = self
            .stats
            .total_memory
            .load(std::sync::atomic::Ordering::Relaxed);
        let memory_usage = current_memory as f32 / self.config.max_memory as f32;

        if memory_usage > self.config.memory_pressure_threshold {
            self.cleanup_lru_objects().await?;
        }

        Ok(())
    }

    async fn cleanup_lru_objects(&self) -> Result<(), StoreError> {
        let mut objects = self.objects.write().await;

        let mut access_times: Vec<(ObjectId, u64)> = objects
            .iter()
            .map(|(&id, obj)| {
                (
                    id,
                    obj.last_accessed.load(std::sync::atomic::Ordering::Relaxed),
                )
            })
            .collect();

        access_times.sort_by_key(|&(_, time)| time);

        let to_remove = (access_times.len() as f32 * 0.1) as usize;
        let to_remove = to_remove.max(1);

        for &(id, _) in access_times.iter().take(to_remove) {
            if let Some(obj) = objects.remove(&id) {
                self.stats
                    .total_objects
                    .fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
                self.stats
                    .total_memory
                    .fetch_sub(obj.size, std::sync::atomic::Ordering::Relaxed);
            }
        }

        Ok(())
    }

    pub async fn cleanup(&self) -> Result<usize, StoreError> {
        let before_count = self
            .stats
            .total_objects
            .load(std::sync::atomic::Ordering::Relaxed);
        self.cleanup_lru_objects().await?;
        let after_count = self
            .stats
            .total_objects
            .load(std::sync::atomic::Ordering::Relaxed);

        Ok(before_count - after_count)
    }

    pub async fn clear(&self) {
        let mut objects = self.objects.write().await;
        objects.clear();

        self.stats
            .total_objects
            .store(0, std::sync::atomic::Ordering::Relaxed);
        self.stats
            .total_memory
            .store(0, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn print_stats(&self) {
        let stats = self.stats();
        println!("=== Object Store Stats ===");
        println!("Objects: {}", stats.total_objects);
        println!("Memory: {:.2} MB", stats.memory_usage_mb());
        println!("Hit Rate: {:.2}%", stats.hit_rate() * 100.0);
        println!("========================");
    }
    /// Get memory usage analysis
    pub fn analyze_memory_usage(&self) -> MemoryAnalysis {
        let stats = self.stats();
        let avg_object_size = if stats.total_objects > 0 {
            stats.total_memory / stats.total_objects
        } else {
            0
        };

        MemoryAnalysis {
            total_objects: stats.total_objects,
            total_memory_bytes: stats.total_memory,
            memory_usage_mb: stats.memory_usage_mb(),
            average_object_size_bytes: avg_object_size,
            hit_rate: stats.hit_rate(),
            cache_efficiency_percent: stats.hit_rate() * 100.0,
            max_memory_bytes: self.config.max_memory,
            max_object_size_bytes: self.config.max_object_size,
            memory_pressure_threshold: self.config.memory_pressure_threshold,
            is_under_pressure: (stats.total_memory as f32 / self.config.max_memory as f32)
                > self.config.memory_pressure_threshold,
        }
    }
    /// Get zero-copy statistics
    pub async fn get_zero_copy_stats(&self) -> ZeroCopyStats {
        let objects = self.objects.read().await;
        let mut zero_copy_count = 0;
        let mut zero_copy_memory = 0;
        let mut shared_count = 0;
        let mut shared_memory = 0;
        let mut standard_count = 0;
        let mut standard_memory = 0;

        for obj in objects.values() {
            match obj.storage_type() {
                "external_ref" => {
                    zero_copy_count += 1;
                    zero_copy_memory += obj.size;
                }
                "memory_mapped" => {
                    // 修复：添加 memory_mapped 的处理
                    zero_copy_count += 1;
                    zero_copy_memory += obj.size;
                }
                "shared" => {
                    shared_count += 1;
                    shared_memory += obj.size;
                }
                "standard" => {
                    standard_count += 1;
                    standard_memory += obj.size;
                }
                _ => {
                    // Unknown storage type
                    standard_count += 1;
                    standard_memory += obj.size;
                }
            }
        }

        ZeroCopyStats {
            zero_copy_objects: zero_copy_count,
            zero_copy_memory_bytes: zero_copy_memory,
            shared_objects: shared_count,
            shared_memory_bytes: shared_memory,
            standard_objects: standard_count,
            standard_memory_bytes: standard_memory,
            total_objects: objects.len(),
            zero_copy_ratio: if objects.is_empty() {
                0.0
            } else {
                zero_copy_count as f64 / objects.len() as f64
            },
        }
    }
    /// Cleanup objects by storage type
    pub async fn cleanup_by_storage_type(&self, storage_type: &str) -> Result<usize, StoreError> {
        let mut objects = self.objects.write().await;
        let mut removed_count = 0;
        let mut removed_memory = 0;

        let to_remove: Vec<ObjectId> = objects
            .iter()
            .filter(|(_, obj)| obj.storage_type() == storage_type)
            .map(|(&id, obj)| {
                removed_memory += obj.size;
                id
            })
            .collect();

        removed_count = to_remove.len();

        for id in to_remove {
            objects.remove(&id);
        }

        // Update statistics
        self.stats
            .total_objects
            .fetch_sub(removed_count, std::sync::atomic::Ordering::Relaxed);
        self.stats
            .total_memory
            .fetch_sub(removed_memory, std::sync::atomic::Ordering::Relaxed);

        Ok(removed_count)
    }
    /// Pre-warm cache for a batch of IDs
    pub async fn warmup_cache(&self, ids: &[ObjectId]) -> Vec<Result<ObjectRef, StoreError>> {
        // Read the objects from the store
        let objects = self.objects.read().await;

        ids.iter()
            .map(|&id| match objects.get(&id) {
                Some(obj) => {
                    self.stats
                        .cache_hits
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    Ok(ObjectRef::new(std::sync::Arc::clone(obj)))
                }
                None => {
                    self.stats
                        .cache_misses
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    Err(StoreError::ObjectNotFound(id))
                }
            })
            .collect()
    }
    /// Get memory pressure information
    pub fn get_memory_pressure_info(&self) -> MemoryPressureInfo {
        let current_memory = self
            .stats
            .total_memory
            .load(std::sync::atomic::Ordering::Relaxed);
        let memory_ratio = current_memory as f32 / self.config.max_memory as f32;
        let is_under_pressure = memory_ratio > self.config.memory_pressure_threshold;

        MemoryPressureInfo {
            current_memory_bytes: current_memory,
            max_memory_bytes: self.config.max_memory,
            memory_usage_ratio: memory_ratio,
            threshold: self.config.memory_pressure_threshold,
            is_under_pressure,
            bytes_until_pressure: if is_under_pressure {
                0
            } else {
                ((self.config.max_memory as f32 * self.config.memory_pressure_threshold) as usize)
                    .saturating_sub(current_memory)
            },
            bytes_over_pressure: if is_under_pressure {
                current_memory.saturating_sub(
                    (self.config.max_memory as f32 * self.config.memory_pressure_threshold)
                        as usize,
                )
            } else {
                0
            },
        }
    }
    /// Store a batch of objects
    async fn put_batch_objects(
        &self,
        objects: Vec<StoredObject>,
    ) -> Result<Vec<ObjectId>, StoreError> {
        if objects.is_empty() {
            return Ok(Vec::new());
        }

        // Pre-check object sizes
        let total_size: usize = objects.iter().map(|o| o.size).sum();
        for obj in &objects {
            if obj.size > self.config.max_object_size {
                return Err(StoreError::ObjectTooLarge {
                    size: obj.size,
                    max_size: self.config.max_object_size,
                });
            }
        }

        // Check memory pressure before inserting
        let current_memory = self
            .stats
            .total_memory
            .load(std::sync::atomic::Ordering::Relaxed);
        if current_memory + total_size > self.config.max_memory {
            self.cleanup_lru_objects().await?;
        }

        let mut ids = Vec::with_capacity(objects.len());

        // Batch insert objects
        {
            let mut store_objects = self.objects.write().await;
            for obj in objects {
                let id = obj.id;
                let size = obj.size;

                store_objects.insert(id, std::sync::Arc::new(obj));
                ids.push(id);

                // 更新统计
                self.stats
                    .total_objects
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                self.stats
                    .total_memory
                    .fetch_add(size, std::sync::atomic::Ordering::Relaxed);
            }
        }

        Ok(ids)
    }
}

impl Default for ObjectStore {
    fn default() -> Self {
        Self::new(StoreConfig::default())
    }
}

#[derive(Debug, Clone)]
pub struct StorageInfo {
    pub id: ObjectId,
    pub size: usize,
    pub storage_type: &'static str,
    pub is_zero_copy: bool,
    pub data_type: DataType,
    pub shape: Option<Vec<usize>>,
    pub alignment: usize,
    pub created_at: std::time::Instant,
    pub last_accessed: u64,
}

#[derive(Debug, Clone)]
pub struct MemoryAnalysis {
    pub total_objects: usize,
    pub total_memory_bytes: usize,
    pub memory_usage_mb: f64,
    pub average_object_size_bytes: usize,
    pub hit_rate: f64,
    pub cache_efficiency_percent: f64,
    pub max_memory_bytes: usize,
    pub max_object_size_bytes: usize,
    pub memory_pressure_threshold: f32,
    pub is_under_pressure: bool,
}
/// Zero-copy statistics
#[derive(Debug, Clone)]
pub struct ZeroCopyStats {
    pub zero_copy_objects: usize,
    pub zero_copy_memory_bytes: usize,
    pub shared_objects: usize,
    pub shared_memory_bytes: usize,
    pub standard_objects: usize,
    pub standard_memory_bytes: usize,
    pub total_objects: usize,
    pub zero_copy_ratio: f64,
}

/// memory pressure information
#[derive(Debug, Clone)]
pub struct MemoryPressureInfo {
    pub current_memory_bytes: usize,
    pub max_memory_bytes: usize,
    pub memory_usage_ratio: f32,
    pub threshold: f32,
    pub is_under_pressure: bool,
    pub bytes_until_pressure: usize,
    pub bytes_over_pressure: usize,
}

impl MemoryPressureInfo {
    /// Pressure level is calculated as a percentage of memory usage relative to the maximum memory.
    /// 0 means no pressure, 100 means maximum pressure.
    pub fn pressure_level(&self) -> u8 {
        ((self.memory_usage_ratio * 100.0) as u8).min(100)
    }

    /// Cleanup is needed if memory usage exceeds 95% of the maximum memory.
    pub fn needs_immediate_cleanup(&self) -> bool {
        self.memory_usage_ratio > 0.95
    }
}
#[cfg(test)]
mod tests {
    use bytes::Bytes;

    use super::*;

    #[test]
    fn test_object_builder() {
        let data = vec![1, 2, 3, 4, 5];
        let obj = ObjectBuilder::new().from_vec(data.clone()).build().unwrap();

        assert_eq!(obj.size, data.len());
        assert_eq!(obj.data.as_slice(), &data[..]);
        assert!(matches!(obj.metadata.data_type, DataType::Bytes));
    }

    #[test]
    fn test_object_builder_with_metadata() {
        let data = vec![1, 2, 3, 4, 5, 6];
        let obj = ObjectBuilder::new()
            .from_vec(data)
            .with_numpy_metadata("uint8".to_string(), vec![2, 3], 'C')
            .build()
            .unwrap();

        if let DataType::NumpyArray { dtype, order } = &obj.metadata.data_type {
            assert_eq!(dtype, "uint8");
            assert_eq!(*order, 'C');
        } else {
            panic!("Expected NumpyArray data type");
        }

        assert_eq!(obj.metadata.shape, Some(vec![2, 3]));
        assert_eq!(obj.metadata.alignment, 8);
    }

    #[test]
    fn test_object_ref() {
        let data = vec![1, 2, 3, 4, 5];
        let obj =
            StoredObject::new::<Bytes>(ObjectId::new(), data.into(), ObjectMetadata::default());

        let obj_ref = ObjectRef::new(Arc::new(obj));
        assert_eq!(obj_ref.size(), 5);
        assert_eq!(obj_ref.data(), &[1, 2, 3, 4, 5]);
        assert!(!obj_ref.as_ptr().is_null());
    }

    #[test]
    fn test_object_ref_clone() {
        let data = vec![1, 2, 3, 4, 5];
        let obj =
            StoredObject::new::<Bytes>(ObjectId::new(), data.into(), ObjectMetadata::default());

        let obj_ref1 = ObjectRef::new(Arc::new(obj));
        let obj_ref2 = obj_ref1.clone();

        assert_eq!(obj_ref1.id(), obj_ref2.id());
        assert_eq!(obj_ref1.data(), obj_ref2.data());
    }

    #[tokio::test]
    async fn test_object_store_basic() {
        let store = ObjectStore::default();
        let data = vec![1, 2, 3, 4, 5];

        let id = store.put(data.clone()).await.unwrap();
        assert!(store.contains(id).await);

        let obj_ref = store.get(id).await.unwrap();
        assert_eq!(obj_ref.data(), &data[..]);
        assert_eq!(obj_ref.size(), data.len());

        let stats = store.stats();
        assert_eq!(stats.total_objects, 1);
        assert_eq!(stats.total_memory, data.len());
    }

    #[tokio::test]
    async fn test_object_store_delete() {
        let store = ObjectStore::default();
        let data = vec![1, 2, 3, 4, 5];

        let id = store.put(data).await.unwrap();
        assert!(store.contains(id).await);

        store.delete(id).await.unwrap();
        assert!(!store.contains(id).await);

        let stats = store.stats();
        assert_eq!(stats.total_objects, 0);
        assert_eq!(stats.total_memory, 0);
    }

    #[tokio::test]
    async fn test_object_store_stats() {
        let store = ObjectStore::default();

        let id1 = store.put(vec![1, 2, 3]).await.unwrap();
        let id2 = store.put(vec![4, 5, 6, 7]).await.unwrap();

        let stats = store.stats();
        assert_eq!(stats.total_objects, 2);
        assert_eq!(stats.total_memory, 7);

        store.get(id1).await.unwrap();
        store.get(id1).await.unwrap();
        store.get(id2).await.unwrap();

        let stats = store.stats();
        assert_eq!(stats.cache_hits, 3);
        assert_eq!(stats.cache_misses, 0);
        assert_eq!(stats.hit_rate(), 1.0);
    }

    #[tokio::test]
    async fn test_object_store_not_found() {
        let store = ObjectStore::default();
        let id = ObjectId::new();

        let result = store.get(id).await;
        assert!(matches!(result, Err(StoreError::ObjectNotFound(_))));

        let result = store.delete(id).await;
        assert!(matches!(result, Err(StoreError::ObjectNotFound(_))));
    }

    #[tokio::test]
    async fn test_object_store_shared_data() {
        let store = ObjectStore::default();
        let shared_data: Arc<[u8]> = Arc::from(&b"shared"[..]);

        let ids = store.put_shared(shared_data, 3).await.unwrap();
        assert_eq!(ids.len(), 3);

        for id in ids {
            let obj_ref = store.get(id).await.unwrap();
            assert_eq!(obj_ref.data(), b"shared");
        }

        let stats = store.stats();
        assert_eq!(stats.total_objects, 3);
        assert_eq!(stats.total_memory, 18);
    }

    #[tokio::test]
    async fn test_object_store_clear() {
        let store = ObjectStore::default();

        store.put(vec![1, 2, 3]).await.unwrap();
        store.put(vec![4, 5, 6]).await.unwrap();

        assert_eq!(store.stats().total_objects, 2);

        store.clear().await;

        assert_eq!(store.stats().total_objects, 0);
        assert_eq!(store.stats().total_memory, 0);
    }
    #[tokio::test]
    async fn test_object_store_clear_ref() {
        let store = ObjectStore::default();
        let tmp_ref = {
            let o1 = store.put(vec![1, 2, 3]).await.unwrap();
            store.put(vec![4, 5, 6]).await.unwrap();

            let obj_ref = store.get(o1).await.unwrap();

            assert_eq!(store.stats().total_objects, 2);
            store.clear().await;
            assert_eq!(store.stats().total_objects, 0);
            assert_eq!(store.stats().total_memory, 0);
            // 确保 obj_ref 仍然有效
            assert_eq!(obj_ref.data(), &[1, 2, 3]);
            obj_ref
        };
        // 确保 tmp_ref 在清理后仍然有效
        assert_eq!(tmp_ref.data(), &[1, 2, 3]);
        // 确保清理后可以重新存储
    }
}

#[cfg(test)]
mod mmap_simple_tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;
    use tempfile::TempDir;

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
    fn test_mmap_zero_copy_recognition() {
        use memmap2::Mmap;

        let test_data = b"Test zero copy recognition";
        let (_temp_dir, file_path) = create_test_file(test_data);

        let file = File::open(&file_path).expect("Failed to open test file");
        let mmap = unsafe { Mmap::map(&file).expect("Failed to create mmap") };
        let mmap_arc = std::sync::Arc::new(mmap);

        // 测试 StorageData
        let storage_data = StorageData::MemoryMapped {
            _mmap: mmap_arc.clone(),
        };

        assert!(
            storage_data.is_zero_copy(),
            "StorageData should recognize mmap as zero-copy"
        );
        assert_eq!(storage_data.storage_type(), "memory_mapped");
        assert_eq!(storage_data.as_slice(), test_data);

        // 测试 StoredObject
        let stored_obj =
            StoredObject::from_mmap(ObjectId::new(), mmap_arc, ObjectMetadata::default());

        assert!(
            stored_obj.is_zero_copy(),
            "StoredObject should recognize mmap as zero-copy"
        );
        assert_eq!(stored_obj.storage_type(), "memory_mapped");
        assert_eq!(stored_obj.data_slice(), test_data);
    }

    #[cfg(feature = "mmap")]
    #[tokio::test]
    async fn test_mmap_object_store_integration() {
        let test_data = b"Object store integration test";
        let (_temp_dir, file_path) = create_test_file(test_data);

        let store = ObjectStore::default();

        // 测试通过 ObjectStore 的 mmap 功能
        let id = store
            .put_mmap_zero_copy(&file_path)
            .await
            .expect("Failed to put mmap");

        let obj_ref = store.get(id).await.expect("Failed to get object");
        assert_eq!(obj_ref.storage_type(), "memory_mapped");
        assert!(
            obj_ref.is_zero_copy(),
            "ObjectRef should recognize mmap as zero-copy"
        );
        assert_eq!(obj_ref.data(), test_data);

        // 测试零拷贝统计
        let stats = store.get_zero_copy_stats().await;
        assert_eq!(stats.zero_copy_objects, 1);
        assert_eq!(stats.total_objects, 1);
        assert_eq!(stats.zero_copy_ratio, 1.0);
    }

    #[cfg(feature = "mmap")]
    #[test]
    fn test_mmap_pointer_consistency() {
        use memmap2::Mmap;

        let test_data = b"Pointer consistency test";
        let (_temp_dir, file_path) = create_test_file(test_data);

        let file = File::open(&file_path).expect("Failed to open test file");
        let mmap = unsafe { Mmap::map(&file).expect("Failed to create mmap") };
        let original_ptr = mmap.as_ptr();
        let mmap_arc = std::sync::Arc::new(mmap);

        let stored_obj =
            StoredObject::from_mmap(ObjectId::new(), mmap_arc.clone(), ObjectMetadata::default());

        // 验证指针一致性（真正的零拷贝）
        let stored_ptr = stored_obj.data_slice().as_ptr();
        assert_eq!(
            original_ptr, stored_ptr,
            "StoredObject data pointer should match original mmap pointer for true zero-copy"
        );

        // 验证通过 StorageData 获取的指针也相同
        let storage_ptr = stored_obj.data.as_ptr();
        assert_eq!(
            original_ptr, storage_ptr,
            "StorageData as_ptr should match original mmap pointer"
        );
    }
}
