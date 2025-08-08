use super::fast_ops::{ExternalDataKeeper, LifetimeKeeper};
use bytes::Bytes;
use std::sync::Arc;

/// The data storage type, which can be:
#[derive(Debug)]
pub enum StorageData {
    /// Standard storage using Bytes
    Standard(Bytes),
    /// Shared storage using Arc<[u8]>
    Shared(Arc<[u8]>),
    /// External reference storage, which can be zero-copy
    ExternalRef {
        data: Bytes,
        _keeper: Box<dyn LifetimeKeeper>,
    },
    /// Memory-mapped storage, which can also be zero-copy
    #[cfg(feature = "mmap")]
    MemoryMapped {
        _mmap: Arc<memmap2::Mmap>, // Use storage type for memory-mapped data, // which can be zero-copy
    },
}

impl StorageData {
    /// To convert to standard Bytes (for backward compatibility)
    pub fn to_bytes(&self) -> Bytes {
        match self {
            StorageData::Standard(bytes) => bytes.clone(),
            StorageData::Shared(arc) => Bytes::from(arc.as_ref().to_vec()),
            StorageData::ExternalRef { data, .. } => data.clone(),
            #[cfg(feature = "mmap")]
            StorageData::MemoryMapped { _mmap } => Bytes::copy_from_slice(&_mmap[..]),
        }
    }
    /// Get a slice of the data
    pub fn as_slice(&self) -> &[u8] {
        match self {
            StorageData::Standard(bytes) => bytes,
            StorageData::Shared(arc) => arc,
            StorageData::ExternalRef { data, .. } => data,
            #[cfg(feature = "mmap")]
            StorageData::MemoryMapped { _mmap } => &_mmap[..], // 直接从 mmap 获取数据
        }
    }

    /// Get the length of the data
    pub fn len(&self) -> usize {
        self.as_slice().len()
    }

    /// Get a pointer to the data
    pub fn as_ptr(&self) -> *const u8 {
        self.as_slice().as_ptr()
    }

    /// Get the storage type as a string identifier
    pub fn storage_type(&self) -> &'static str {
        match self {
            StorageData::Standard(_) => "standard",
            StorageData::Shared(_) => "shared",
            StorageData::ExternalRef { .. } => "external_ref",
            #[cfg(feature = "mmap")]
            StorageData::MemoryMapped { .. } => "memory_mapped",
        }
    }

    /// Whether the storage data is zero-copy
    pub fn is_zero_copy(&self) -> bool {
        match self {
            StorageData::ExternalRef { .. } => true,
            #[cfg(feature = "mmap")]
            StorageData::MemoryMapped { .. } => true,
            _ => false,
        }
    }
}

impl From<Bytes> for StorageData {
    fn from(bytes: Bytes) -> Self {
        StorageData::Standard(bytes)
    }
}

impl From<Vec<u8>> for StorageData {
    fn from(vec: Vec<u8>) -> Self {
        StorageData::Standard(vec.into())
    }
}

impl From<Arc<[u8]>> for StorageData {
    fn from(arc: Arc<[u8]>) -> Self {
        StorageData::Shared(arc)
    }
}
