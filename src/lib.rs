#![feature(vec_into_raw_parts)]

use std::any::{type_name, Any};
use std::mem;
use std::slice::SliceIndex;

mod vtable {
    use std::any::{Any, TypeId};

    pub struct VTable {
        id: TypeId,
        pub drop_vec: fn(*mut u8, usize, usize),
    }

    impl VTable {
        pub fn new<T: Any>() -> VTable {
            VTable {
                id: TypeId::of::<T>(),
                drop_vec: drop_vec::<T>,
            }
        }

        pub fn is<T: Any>(&self) -> bool {
            TypeId::of::<T>() == self.id
        }
    }

    fn drop_vec<T>(data: *mut u8, length: usize, capacity: usize) {
        unsafe { Vec::from_raw_parts(data as *mut T, length, capacity) };
    }
}

use vtable::VTable;

pub struct AnyVec {
    data: *mut u8,
    length: usize,
    capacity: usize,
    vtable: VTable,
}

impl AnyVec {
    pub fn new<T: Any>() -> AnyVec {
        AnyVec::from_vec(Vec::<T>::new())
    }

    pub fn from_vec<T: Any>(vec: Vec<T>) -> AnyVec {
        let (data, length, capacity) = vec.into_raw_parts();
        AnyVec {
            data: data as *mut u8,
            length,
            capacity,
            vtable: VTable::new::<T>(),
        }
    }

    fn typecheck<T: Any>(&self) -> bool {
        self.vtable.is::<T>()
    }

    fn assert_typecheck<T: Any>(&self) {
        if !self.typecheck::<T>() {
            panic!("AnyVec type does not match {}", type_name::<T>());
        }
    }

    fn into_vec<T: Any>(self) -> Vec<T> {
        self.assert_typecheck::<T>();
        let moved = unsafe { Vec::from_raw_parts(self.data as *mut T, self.length, self.capacity) };
        // We've transferred ownership of the memory we own into ``moved``, so
        // don't run our destructor.
        mem::forget(self);
        moved
    }

    fn with_vec<'a, T: Any, F, R>(&'a self, f: F) -> R
    where
        F: FnOnce(&'a Vec<T>) -> R,
    {
        self.assert_typecheck::<T>();
        let vec = unsafe { Vec::from_raw_parts(self.data as *mut T, self.length, self.capacity) };
        let vec_ptr = &vec as *const Vec<T>;
        unsafe {
            let result = f(&*vec_ptr);
            mem::forget(vec);
            return result;
        }
    }

    fn with_mut_vec<'a, T: Any, F, R>(&'a mut self, f: F) -> R
    where
        F: FnOnce(&mut Vec<T>) -> R,
    {
        self.assert_typecheck::<T>();

        let mut vec =
            unsafe { Vec::from_raw_parts(self.data as *mut T, self.length, self.capacity) };

        let result = f(&mut vec);

        // Mutating operation might have changed our data members, so write
        // them back on completion.
        let (data, length, capacity) = vec.into_raw_parts();
        self.data = data as *mut u8;
        self.length = length;
        self.capacity = capacity;

        result
    }

    // Vec API

    pub fn push<T: Any>(&mut self, value: T) {
        self.with_mut_vec(|vec: &mut Vec<T>| vec.push(value));
    }

    pub fn get<'a, T: Any, I>(&'a self, index: I) -> Option<&'a <I as SliceIndex<[T]>>::Output>
    where
        I: SliceIndex<[T]>,
    {
        self.with_vec(|vec: &'a Vec<T>| vec.get(index))
    }

    // End Vec API
}

impl Drop for AnyVec {
    fn drop(&mut self) {
        (self.vtable.drop_vec)(self.data, self.length, self.capacity)
    }
}

#[cfg(test)]
mod tests {
    use super::AnyVec;

    #[test]
    fn test_push_u64() {
        let mut dynamic: AnyVec = AnyVec::new::<u64>();

        for i in 0..1000 {
            dynamic.push(i as u64);
        }

        let result: Vec<u64> = dynamic.into_vec();
        let expected: Vec<u64> = (0..1000).collect();
        assert_eq!(result, expected);
    }

    #[test]
    fn test_push_f64() {
        let mut dynamic: AnyVec = AnyVec::new::<f64>();

        for i in 0..1000 {
            dynamic.push(i as f64);
        }

        let result: Vec<f64> = dynamic.into_vec();
        let expected: Vec<f64> = (0..1000).map(|x| x as f64).collect();
        assert_eq!(result, expected);
    }

    #[test]
    #[should_panic]
    fn test_assert_typecheck() {
        let mut dynamic: AnyVec = AnyVec::new::<f64>();
        dynamic.assert_typecheck::<u64>();
    }

    #[test]
    fn test_get() {
        let mut dynamic: AnyVec = AnyVec::from_vec::<u64>(vec![3, 4, 5]);

        for i in 0..3 {
            let expected_value: u64 = i + 3;
            assert_eq!(dynamic.get(i as usize), Some(&expected_value));
        }

        for i in 4..6 {
            let expected: Option<&u64> = None;
            assert_eq!(dynamic.get(i), expected);
        }
    }
}
