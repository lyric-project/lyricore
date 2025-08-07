use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ObjectId(pub u64);

impl ObjectId {
    pub fn new() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        Self(COUNTER.fetch_add(1, Ordering::Relaxed))
    }
    pub fn from_str(s: &str) -> Result<Self> {
        s.parse::<u64>()
            .map(ObjectId)
            .map_err(|_| StoreError::Internal("Invalid object ID format".into()))
    }

    pub fn to_string(&self) -> String {
        self.0.to_string()
    }
}

impl Default for ObjectId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for ObjectId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ObjectId({})", self.0)
    }
}

#[derive(Debug, Clone)]
pub struct ObjectMetadata {
    pub data_type: DataType,
    pub shape: Option<Vec<usize>>,
    pub strides: Option<Vec<isize>>,
    pub alignment: usize,
    pub track_access_time: bool,
}

impl Default for ObjectMetadata {
    fn default() -> Self {
        Self {
            data_type: DataType::Bytes,
            shape: None,
            strides: None,
            alignment: 1,
            track_access_time: true,
        }
    }
}

#[derive(Debug, Clone)]
pub enum DataType {
    Bytes,
    ArrowBuffer,
    NumpyArray { dtype: String, order: char },
    Custom(String),
}

pub struct StoredObject {
    pub id: ObjectId,
    pub size: usize,
    pub(crate) data: StorageData,
    pub created_at: std::time::Instant,
    pub last_accessed: AtomicU64,
    pub metadata: ObjectMetadata,
}

impl StoredObject {
    pub fn new<B: Into<StorageData>>(id: ObjectId, data: B, metadata: ObjectMetadata) -> Self {
        let data = data.into();
        let size = data.len();
        Self {
            id,
            size,
            data,
            created_at: std::time::Instant::now(),
            last_accessed: AtomicU64::new(0),
            metadata,
        }
    }

    /// Create a shared storage object
    pub fn from_arc(id: ObjectId, data: std::sync::Arc<[u8]>, metadata: ObjectMetadata) -> Self {
        Self {
            id,
            size: data.len(),
            data: StorageData::from(data),
            created_at: std::time::Instant::now(),
            last_accessed: AtomicU64::new(0),
            metadata,
        }
    }
    /// Create a storage object from an external reference
    ///
    /// # Safety
    /// The caller must ensure that the pointer remains valid for the lifetime of the `keeper`.
    pub unsafe fn from_external_ref<T: 'static + Send + Sync>(
        id: ObjectId,
        ptr: *const u8,
        len: usize,
        keeper: T,
        metadata: ObjectMetadata,
    ) -> Self {
        let slice = std::slice::from_raw_parts(ptr, len);
        let bytes = bytes::Bytes::from_static(slice);
        let data = StorageData::ExternalRef {
            data: bytes,
            _keeper: Box::new(ExternalDataKeeper::new(keeper)),
        };

        Self {
            id,
            size: len,
            data,
            created_at: std::time::Instant::now(),
            last_accessed: AtomicU64::new(0),
            metadata,
        }
    }
    /// Memory-mapped storage object
    #[cfg(feature = "mmap")]
    pub fn from_mmap(
        id: ObjectId,
        mmap: std::sync::Arc<memmap2::Mmap>,
        metadata: ObjectMetadata,
    ) -> Self {
        let len = mmap.len();

        let data = StorageData::MemoryMapped { _mmap: mmap };

        Self {
            id,
            size: len,
            data,
            created_at: std::time::Instant::now(),
            last_accessed: AtomicU64::new(0),
            metadata,
        }
    }
    /// Get the storage type as a string identifier
    pub fn storage_type(&self) -> &'static str {
        self.data.storage_type()
    }

    /// Whether the data is stored in a zero-copy manner
    pub fn is_zero_copy(&self) -> bool {
        self.data.is_zero_copy()
    }

    /// Get bytes representation of the data
    pub fn data_bytes(&self) -> bytes::Bytes {
        self.data.to_bytes()
    }

    /// Get a slice of the data
    pub fn data_slice(&self) -> &[u8] {
        self.data.as_slice()
    }
}

impl std::fmt::Debug for StoredObject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoredObject")
            .field("id", &self.id)
            .field("size", &self.size)
            .field("storage_type", &self.storage_type())
            .field("is_zero_copy", &self.is_zero_copy())
            .field("created_at", &self.created_at)
            .field("last_accessed", &self.last_accessed.load(Ordering::Relaxed))
            .field("metadata", &self.metadata)
            .finish()
    }
}

pub mod error;
pub mod fast_ops;
pub mod storage_data;
pub mod store;
pub mod views;

pub use error::*;
pub use fast_ops::*;
pub use storage_data::*;
pub use store::*;
pub use views::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_object_id() {
        let id1 = ObjectId::new();
        let id2 = ObjectId::new();
        assert_ne!(id1, id2);
        assert!(id2.0 > id1.0);
    }

    #[test]
    fn test_object_metadata_default() {
        let meta = ObjectMetadata::default();
        assert!(matches!(meta.data_type, DataType::Bytes));
        assert_eq!(meta.alignment, 1);
        assert!(meta.shape.is_none());
        assert!(meta.strides.is_none());
    }
}
