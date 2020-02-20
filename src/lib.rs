#![feature(vec_into_raw_parts)]

use std::any::Any;
use std::mem;
use std::slice::SliceIndex;

mod vtable {
    use std::any::{type_name, Any, TypeId};

    pub struct VTable {
        id: TypeId,
        pub display_name: &'static str,
        pub drop_vec: fn(*mut u8, usize, usize),
        pub drop_slice: fn(*mut u8, usize),
        pub size: usize,
    }

    impl VTable {
        pub fn new<T: Any>() -> VTable {
            VTable {
                id: TypeId::of::<T>(),
                display_name: type_name::<T>(),
                drop_vec: drop_vec::<T>,
                drop_slice: drop_slice::<T>,
                size: std::mem::size_of::<T>(),
            }
        }

        pub fn is<T: Any>(&self) -> bool {
            TypeId::of::<T>() == self.id
        }

        fn typecheck<T: Any>(&self) -> bool {
            self.is::<T>()
        }

        pub fn assert_typecheck<T: Any>(&self) {
            if !self.typecheck::<T>() {
                panic!(
                    "Static type ({}) does not match runtime type ({})",
                    self.display_name,
                    type_name::<T>()
                );
            }
        }
    }

    fn drop_vec<T>(data: *mut u8, length: usize, capacity: usize) {
        unsafe { Vec::from_raw_parts(data as *mut T, length, capacity) };
    }

    fn drop_slice<T>(data: *mut u8, length: usize) {
        unsafe {
            let s: &mut [T] = std::slice::from_raw_parts_mut(data as *mut T, length);
            std::ptr::drop_in_place(s);
        }
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

    fn assert_typecheck<T: Any>(&self) {
        self.vtable.assert_typecheck::<T>();
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

        unsafe {
            // Temporarily materialize a vector and pass it through to ``f``,
            let vec = Vec::from_raw_parts(self.data as *mut T, self.length, self.capacity);
            let vec_ptr = &vec as *const Vec<T>;
            let result: R = f(&*vec_ptr);
            mem::forget(vec);
            result
        }
    }

    fn with_mut_vec<'a, T: Any, F, R>(&'a mut self, f: F) -> R
    where
        F: FnOnce(&'a mut Vec<T>) -> R,
    {
        self.assert_typecheck::<T>();

        let (result, (data, length, capacity)) = unsafe {
            let mut vec = Vec::from_raw_parts(self.data as *mut T, self.length, self.capacity);
            let vec_ptr = &mut vec as *mut Vec<T>;
            let result: R = f(&mut *vec_ptr);
            (result, vec.into_raw_parts())
        };

        self.data = data as *mut u8;
        self.length = length;
        self.capacity = capacity;
        return result;
    }

    // Vec API
    pub fn push<T: Any>(&mut self, value: T) {
        self.with_mut_vec(|vec: &mut Vec<T>| vec.push(value));
    }

    pub fn truncate(&mut self, length: usize) {
        if length > self.length {
            return;
        }

        // See Vec::truncate impl.
        self.length = length;
        (self.vtable.drop_slice)(
            unsafe { self.data.add(length * self.vtable.size) },
            length - self.length,
        );
    }

    pub fn clear(&mut self) {
        self.truncate(0);
    }

    // Slice API

    pub fn get<'a, T: Any, I>(&'a self, index: I) -> Option<&'a <I as SliceIndex<[T]>>::Output>
    where
        I: SliceIndex<[T]>,
    {
        self.with_vec(|vec: &'a Vec<T>| vec.get(index))
    }

    pub fn first<'a, T: Any>(&'a self) -> Option<&'a T> {
        self.with_vec(|vec: &'a Vec<T>| vec.first())
    }

    pub fn first_mut<'a, T: Any>(&'a mut self) -> Option<&'a mut T> {
        self.with_mut_vec(|vec: &'a mut Vec<T>| vec.first_mut())
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

    use std::cell::RefCell;
    use std::rc::Rc;

    #[test]
    fn test_push_u64() {
        let mut dynamic: AnyVec = AnyVec::new::<u64>();

        for i in 0..10000 {
            dynamic.push(i as u64);
        }

        let result: Vec<u64> = dynamic.into_vec();
        let expected: Vec<u64> = (0..10000).collect();
        assert_eq!(result, expected);
    }

    #[test]
    fn test_push_f64() {
        let mut dynamic: AnyVec = AnyVec::new::<f64>();

        for i in 0..10000 {
            dynamic.push(i as f64);
        }

        let result: Vec<f64> = dynamic.into_vec();
        let expected: Vec<f64> = (0..10000).map(|x| x as f64).collect();
        assert_eq!(result, expected);
    }

    #[test]
    #[should_panic]
    fn test_assert_typecheck_passes() {
        let dynamic: AnyVec = AnyVec::new::<f64>();
        dynamic.assert_typecheck::<f64>();

        let dynamic2: AnyVec = AnyVec::new::<f64>();
        dynamic2.assert_typecheck::<u64>();
    }

    #[test]
    #[should_panic]
    fn test_assert_typecheck_fails() {
        let dynamic: AnyVec = AnyVec::new::<f64>();
        dynamic.assert_typecheck::<u64>();
    }

    #[test]
    fn test_get() {
        let dynamic: AnyVec = AnyVec::from_vec::<u64>(vec![3, 4, 5]);

        for i in 0..3 {
            let expected_value: u64 = i + 3;
            assert_eq!(dynamic.get(i as usize), Some(&expected_value));
        }

        for i in 4..6 {
            let expected: Option<&u64> = None;
            assert_eq!(dynamic.get(i), expected);
        }
    }

    #[test]
    fn test_first() {
        let dynamic: AnyVec = AnyVec::from_vec::<u64>(vec![3, 4, 5]);

        let result = dynamic.first();
        let expected: u64 = 3;
        assert_eq!(result, Some(&expected));
    }

    #[test]
    fn test_first_mut() {
        let mut dynamic: AnyVec = AnyVec::from_vec::<u64>(vec![3, 4, 5]);

        {
            let mut result = dynamic.first_mut();
            let mut expected: u64 = 3;
            assert_eq!(result, Some(&mut expected));

            // Write to the front of the vector through the received reference.
            *result.unwrap() = 100;
        }

        // result is now out of scope, so we can read from the original vector again.
        let typed = dynamic.into_vec::<u64>();
        assert_eq!(typed, vec![100, 4, 5]);
    }

    #[test]
    fn test_drop_vec() {
        let chan: Rc<RefCell<Vec<i64>>> = Rc::new(RefCell::new(vec![]));
        let dynamic = AnyVec::from_vec(vec![
            HasDrop {
                id: 1,
                chan: chan.clone(),
            },
            HasDrop {
                id: 2,
                chan: chan.clone(),
            },
            HasDrop {
                id: 3,
                chan: chan.clone(),
            },
        ]);
        std::mem::drop(dynamic);

        let result: Vec<i64> = chan.borrow().clone();
        let expected = vec![1, 2, 3];

        assert_eq!(result, expected);
    }

    // A struct that appends its id into a shared vector when it's dropped.
    // This is useful for testing that values of this type get dropped when
    // they should.
    struct HasDrop {
        id: i64,
        chan: std::rc::Rc<std::cell::RefCell<Vec<i64>>>,
    }

    impl Drop for HasDrop {
        fn drop(&mut self) {
            self.chan.borrow_mut().push(self.id);
        }
    }
}
