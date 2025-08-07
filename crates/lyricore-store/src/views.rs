use crate::{DataType, ObjectRef};

pub struct DataView {
    pub obj_ref: ObjectRef,
}

impl DataView {
    pub fn new(obj_ref: ObjectRef) -> Self {
        Self { obj_ref }
    }

    pub fn as_bytes(&self) -> &[u8] {
        self.obj_ref.data()
    }

    pub fn as_arc(&self) -> std::sync::Arc<[u8]> {
        self.obj_ref.data_arc()
    }

    pub fn as_bytes_clone(&self) -> bytes::Bytes {
        self.obj_ref.data_bytes()
    }

    pub fn as_numpy_compatible(&self) -> Option<NumpyView<'_>> {
        match &self.obj_ref.metadata().data_type {
            DataType::NumpyArray { dtype, order } => Some(NumpyView {
                data: self.obj_ref.data(),
                dtype: dtype.clone(),
                shape: self.obj_ref.metadata().shape.clone().unwrap_or_default(),
                order: *order,
            }),
            _ => None,
        }
    }

    pub fn as_arrow_compatible(&self) -> Option<ArrowView> {
        match &self.obj_ref.metadata().data_type {
            DataType::ArrowBuffer => Some(ArrowView {
                data: self.obj_ref.data_bytes(),
                alignment: self.obj_ref.metadata().alignment,
            }),
            _ => None,
        }
    }
}

#[derive(Debug)]
pub struct NumpyView<'a> {
    pub data: &'a [u8],
    pub dtype: String,
    pub shape: Vec<usize>,
    pub order: char,
}

impl<'a> NumpyView<'a> {
    pub fn for_numpy_capi(&self) -> (*const u8, Vec<isize>, isize) {
        let ptr = self.data.as_ptr();
        let dims: Vec<isize> = self.shape.iter().map(|&x| x as isize).collect();
        let itemsize = self.get_itemsize();
        (ptr, dims, itemsize)
    }

    fn get_itemsize(&self) -> isize {
        match self.dtype.as_str() {
            "int8" | "uint8" | "bool" => 1,
            "int16" | "uint16" => 2,
            "int32" | "uint32" | "float32" => 4,
            "int64" | "uint64" | "float64" => 8,
            _ => 1,
        }
    }
}

#[derive(Debug)]
pub struct ArrowView {
    pub data: bytes::Bytes,
    pub alignment: usize,
}

impl ArrowView {
    pub fn as_arrow_buffer(&self) -> (*const u8, usize, usize) {
        (self.data.as_ptr(), self.data.len(), self.alignment)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ObjectBuilder;

    #[test]
    fn test_data_view() {
        let data = vec![1, 2, 3, 4, 5];
        let obj = ObjectBuilder::new().from_vec(data.clone()).build().unwrap();

        let obj_ref = crate::store::ObjectRef::new(std::sync::Arc::new(obj));
        let view = DataView::new(obj_ref);

        assert_eq!(view.as_bytes(), &data[..]);
        assert_eq!(view.as_arc().as_ref(), &data[..]);
    }

    #[test]
    fn test_numpy_view() {
        let data = vec![1u8, 2, 3, 4, 5, 6];
        let obj = ObjectBuilder::new()
            .from_vec(data)
            .with_numpy_metadata("uint8".to_string(), vec![2, 3], 'C')
            .build()
            .unwrap();

        let obj_ref = crate::store::ObjectRef::new(std::sync::Arc::new(obj));
        let view = DataView::new(obj_ref);

        let numpy_view = view.as_numpy_compatible().unwrap();
        assert_eq!(numpy_view.dtype, "uint8");
        assert_eq!(numpy_view.shape, vec![2, 3]);
        assert_eq!(numpy_view.order, 'C');

        let (ptr, dims, itemsize) = numpy_view.for_numpy_capi();
        assert!(!ptr.is_null());
        assert_eq!(dims, vec![2, 3]);
        assert_eq!(itemsize, 1);
    }

    #[test]
    fn test_arrow_view() {
        let data = vec![0u8; 1024];
        let obj = ObjectBuilder::new()
            .from_vec(data)
            .with_arrow_metadata(64)
            .build()
            .unwrap();

        let obj_ref = crate::store::ObjectRef::new(std::sync::Arc::new(obj));
        let view = DataView::new(obj_ref);

        let arrow_view = view.as_arrow_compatible().unwrap();
        assert_eq!(arrow_view.alignment, 64);

        let (ptr, len, alignment) = arrow_view.as_arrow_buffer();
        assert!(!ptr.is_null());
        assert_eq!(len, 1024);
        assert_eq!(alignment, 64);
    }

    #[test]
    fn test_data_view_no_numpy_metadata() {
        let data = vec![1, 2, 3, 4, 5];
        let obj = ObjectBuilder::new().from_vec(data).build().unwrap();

        let obj_ref = crate::store::ObjectRef::new(std::sync::Arc::new(obj));
        let view = DataView::new(obj_ref);

        assert!(view.as_numpy_compatible().is_none());
        assert!(view.as_arrow_compatible().is_none());
    }
}
