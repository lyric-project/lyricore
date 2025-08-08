/// Fast memory copy modes
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FastCopyMode {
    /// Auto-detect mode
    Auto,
    /// Using standard slice copy (safe)
    Standard,
    /// Use `ptr::copy_nonoverlapping` (unsafe but faster)
    Unsafe,
}

/// High-performance memory operations utility
pub struct FastMemOps;

impl FastMemOps {
    /// High-performance memory copy
    ///
    /// # Safety
    /// When using FastCopyMode::Unsafe, the caller must ensure:
    /// 1. src pointer is valid and readable
    /// 2. len is a valid length
    /// 3. memory does not overlap
    pub unsafe fn fast_copy(src: *const u8, len: usize, mode: FastCopyMode) -> Vec<u8> {
        let mut dst = Vec::with_capacity(len);

        match mode {
            FastCopyMode::Auto => {
                if len > 1024 * 1024 {
                    // 1MB+ use Unsafe
                    std::ptr::copy_nonoverlapping(src, dst.as_mut_ptr(), len);
                    dst.set_len(len);
                } else {
                    let slice = std::slice::from_raw_parts(src, len);
                    dst.extend_from_slice(slice);
                }
            }
            FastCopyMode::Unsafe => {
                std::ptr::copy_nonoverlapping(src, dst.as_mut_ptr(), len);
                dst.set_len(len);
            }
            FastCopyMode::Standard => {
                let slice = std::slice::from_raw_parts(src, len);
                dst.extend_from_slice(slice);
            }
        }

        dst
    }

    /// Check memory alignment
    pub fn is_aligned(ptr: *const u8, alignment: usize) -> bool {
        (ptr as usize) % alignment == 0
    }

    /// Get recommended alignment size based on data size
    pub fn recommend_alignment(size: usize) -> usize {
        if size >= 1024 * 1024 {
            64
        } else if size >= 1024 {
            16
        } else {
            8
        }
    }
}

/// Keepers are used to manage the lifetime of external data references
pub trait LifetimeKeeper: Send + Sync + std::fmt::Debug {
    fn as_any(&self) -> &dyn std::any::Any;
}

/// Outside data keeper for zero-copy references
#[derive(Debug)]
pub struct ExternalDataKeeper {
    data: Box<dyn std::any::Any + Send + Sync>,
}

impl ExternalDataKeeper {
    pub fn new<T: 'static + Send + Sync>(data: T) -> Self {
        Self {
            data: Box::new(data),
        }
    }
}

impl LifetimeKeeper for ExternalDataKeeper {
    fn as_any(&self) -> &dyn std::any::Any {
        self.data.as_ref()
    }
}
