#![feature(vec_into_raw_parts)]
#![feature(const_fn)]
#![feature(const_type_id)]
#![feature(const_type_name)]

use std::mem;

mod vtable {
    use std::any::{type_name, Any, TypeId};

    #[derive(Clone)]
    pub struct VTable {
        id: TypeId,
        pub display_name: &'static str,
        pub drop_vec: unsafe fn(*mut u8, usize, usize),
        pub drop_slice: unsafe fn(*mut u8, usize),
        pub clone: unsafe fn(*const u8, *mut u8),
        pub mv: unsafe fn(*const u8, *mut u8),
        pub eq: unsafe fn(*const u8, *const u8) -> bool,
        pub reserve: unsafe fn(usize, *mut u8, usize, usize) -> (*mut u8, usize, usize),
        pub size: usize,
    }

    impl VTable {
        pub const fn new<T: Any + Clone + PartialEq>() -> VTable {
            VTable {
                id: TypeId::of::<T>(),
                display_name: type_name::<T>(),
                drop_vec: drop_vec::<T>,
                drop_slice: drop_slice::<T>,
                clone: clone::<T>,
                eq: eq::<T>,
                mv: mv::<T>,
                reserve: reserve::<T>,
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

    impl PartialEq for VTable {
        fn eq(&self, other: &Self) -> bool {
            self.id == other.id
        }
    }
    impl Eq for VTable {}

    unsafe fn eq<T: PartialEq>(lhs_ptr: *const u8, rhs_ptr: *const u8) -> bool {
        let lhs: &T = &*(lhs_ptr as *const T);
        let rhs: &T = &*(rhs_ptr as *const T);
        lhs == rhs
    }

    unsafe fn clone<T: Clone>(src_ptr: *const u8, dest_ptr: *mut u8) {
        let src: &T = &*(src_ptr as *const T);
        let dest: &mut T = &mut *(dest_ptr as *mut T);
        dest.clone_from(src);
    }

    unsafe fn mv<T>(src: *const u8, dest: *mut u8) {
        // XXX: Can we guarantee that src is properly aligned?
        std::ptr::copy_nonoverlapping(src, dest, std::mem::size_of::<T>());
    }

    unsafe fn reserve<T>(
        newsize: usize,
        data: *mut u8,
        length: usize,
        capacity: usize,
    ) -> (*mut u8, usize, usize) {
        let mut v = Vec::from_raw_parts(data as *mut T, length, capacity);
        v.reserve(newsize);

        let (new_data, new_length, new_capacity) = v.into_raw_parts();
        (new_data as *mut u8, new_length, new_capacity)
    }

    unsafe fn drop_vec<T>(data: *mut u8, length: usize, capacity: usize) {
        Vec::from_raw_parts(data as *mut T, length, capacity);
    }

    unsafe fn drop_slice<T>(data: *mut u8, length: usize) {
        let s: &mut [T] = std::slice::from_raw_parts_mut(data as *mut T, length);
        std::ptr::drop_in_place(s);
    }

    pub trait StaticVTable {
        const VTABLE: VTable;
    }

    impl<T> StaticVTable for T
    where
        T: Any + Clone + PartialEq + 'static,
    {
        const VTABLE: VTable = VTable::new::<T>();
    }
}

use vtable::{StaticVTable, VTable};

#[derive(Clone)]
pub struct AnyRef<'a> {
    data: *const u8,
    vtable: &'static VTable,
    phantom: std::marker::PhantomData<&'a [u8]>,
}

impl<'a> AnyRef<'a> {
    fn new<T: StaticVTable>(val: &'a T) -> AnyRef<'a> {
        AnyRef {
            data: val as *const T as *const u8,
            vtable: &T::VTABLE,
            phantom: std::marker::PhantomData,
        }
    }
}

impl std::fmt::Debug for AnyRef<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "AnyRef")
    }
}

impl<T: StaticVTable> PartialEq<&T> for AnyRef<'_> {
    fn eq(&self, other: &&T) -> bool {
        if self.vtable != &T::VTABLE {
            return false;
        }
        let addr: *const u8 = *other as *const T as *const u8;
        unsafe { (self.vtable.eq)(self.data, addr) }
    }
}

impl PartialEq<AnyRef<'_>> for AnyRef<'_> {
    fn eq(&self, other: &AnyRef<'_>) -> bool {
        if self.vtable != other.vtable {
            return false;
        }
        unsafe { (self.vtable.eq)(self.data, other.data) }
    }
}

pub struct AnyVec {
    data: *mut u8,
    length: usize,
    capacity: usize,
    vtable: &'static VTable,
}

use std::any::Any;

impl AnyVec {
    pub fn new<T: Any + Clone + PartialEq>() -> AnyVec {
        AnyVec::from_vec(Vec::<T>::new())
    }

    pub fn from_vec<T: StaticVTable>(vec: Vec<T>) -> AnyVec {
        let (data, length, capacity) = vec.into_raw_parts();
        AnyVec {
            data: data as *mut u8,
            length,
            capacity,
            vtable: &T::VTABLE,
        }
    }

    fn assert_typecheck<T: Any>(&self) {
        self.vtable.assert_typecheck::<T>();
    }

    unsafe fn typed<T>(&self) -> std::mem::ManuallyDrop<Vec<T>> {
        std::mem::ManuallyDrop::new(Vec::from_raw_parts(
            self.data as *mut T,
            self.length,
            self.capacity,
        ))
    }

    fn into_vec<T: Any + Clone + PartialEq>(self) -> Vec<T> {
        self.assert_typecheck::<T>();
        let moved = unsafe { self.typed() };
        // We're transferring ownership of the memory we own into ``moved``, so
        // don't run our destructor.
        mem::forget(self);
        std::mem::ManuallyDrop::into_inner(moved)
    }

    fn at(&self, n: usize) -> *mut u8 {
        if n >= self.capacity {
            panic!("{} > self.capacity ({})", n, self.length);
        }
        unsafe { self.data.add(n * self.vtable.size) }
    }

    fn reserve(&mut self, size: usize) {
        let (data, length, capacity) =
            unsafe { (self.vtable.reserve)(size, self.data, self.length, self.capacity) };
        self.data = data;
        self.length = length;
        self.capacity = capacity;
    }

    // Vec API
    pub fn push<T: Any>(&mut self, value: T) {
        self.reserve(self.length + 1);
        assert!(self.capacity > self.length);

        let src: *const u8 = &value as *const T as *const u8;
        let dest: *mut u8 = self.at(self.length);

        // Move value into the vector and then forget about it so that we don't
        // drop it when we leave this function.
        unsafe {
            (self.vtable.mv)(src, dest);
            self.length += 1;
            mem::forget(value);
        }
    }

    pub fn truncate(&mut self, length: usize) {
        if length > self.length {
            return;
        }

        // See Vec::truncate impl.
        let ndropped: usize = self.length - length;
        self.length = length;
        unsafe { (self.vtable.drop_slice)(self.data.add(length * self.vtable.size), ndropped) };
    }

    pub fn clear(&mut self) {
        self.truncate(0);
    }

    pub fn dedup(&mut self) -> () {
        unimplemented!("dedup");
    }

    // Slice API
    pub fn get<'a>(&'a self, index: usize) -> Option<AnyRef<'a>> {
        if index >= self.length {
            return None;
        }
        let addr = self.at(index);
        return Some(AnyRef {
            data: addr,
            vtable: self.vtable,
            phantom: std::marker::PhantomData,
        });
    }

    pub fn first<'a>(&'a self) -> Option<AnyRef<'a>> {
        self.get(0)
    }

    // pub fn first_mut<'a>(&'a mut self) -> Option<AnyMutRef<'a>> {
    //     unimplemented!();
    // }
    // End Vec API
}

impl Drop for AnyVec {
    fn drop(&mut self) {
        unsafe { (self.vtable.drop_vec)(self.data, self.length, self.capacity) }
    }
}

#[cfg(test)]
mod tests {
    use super::{AnyRef, AnyVec};
    use std::cell::RefCell;
    use std::rc::Rc;

    // A struct that appends its id into a shared vector when it's dropped.
    // This is useful for testing that values of this type get dropped when
    // they should.
    #[derive(Clone)]
    struct HasDrop {
        id: i64,
        chan: std::rc::Rc<std::cell::RefCell<Vec<i64>>>,
    }

    impl PartialEq for HasDrop {
        fn eq(&self, other: &Self) -> bool {
            self.id == other.id
        }
    }

    impl Drop for HasDrop {
        fn drop(&mut self) {
            self.chan.borrow_mut().push(self.id);
        }
    }

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
        let value = dynamic.get(0);
        assert_eq!(value, Some(AnyRef::new(&(3 as u64))));

        for i in 0..3 {
            let expected_value: u64 = i + 3;
            assert_eq!(dynamic.get(i as usize), Some(AnyRef::new(&expected_value)));
        }

        for i in 4..6 {
            let expected: Option<AnyRef> = None;
            assert_eq!(dynamic.get(i), expected);
        }
    }

    #[test]
    fn test_first() {
        let dynamic: AnyVec = AnyVec::from_vec::<u64>(vec![3, 4, 5]);

        let result = dynamic.first();
        let expected: u64 = 3;
        assert_eq!(result, Some(AnyRef::new(&expected)));
    }

    #[test]
    fn test_compare_ref_to_value() {
        let val: f64 = 3.5;
        let dynval = AnyRef::new(&val);
        assert_eq!(dynval, &val);
    }

    // #[test]
    // fn test_first_mut() {
    //     let mut dynamic: AnyVec = AnyVec::from_vec::<u64>(vec![3, 4, 5]);

    //     {
    //         let mut result = dynamic.first_mut();
    //         let mut expected: u64 = 3;
    //         assert_eq!(result, Some(&mut expected));

    //         // Write to the front of the vector through the received reference.
    //         *result.unwrap() = 100;
    //     }

    //     // result is now out of scope, so we can read from the original vector again.
    //     let typed = dynamic.into_vec::<u64>();
    //     assert_eq!(typed, vec![100, 4, 5]);
    // }

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

    #[test]
    fn test_truncate() {
        let chan: Rc<RefCell<Vec<i64>>> = Rc::new(RefCell::new(vec![]));
        let mut dynamic = AnyVec::from_vec(vec![
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

        dynamic.truncate(2);
        let result: Vec<i64> = chan.borrow().clone();
        let expected = vec![3];
        assert_eq!(result, expected);

        dynamic.truncate(1);
        let result: Vec<i64> = chan.borrow().clone();
        let expected = vec![3, 2];
        assert_eq!(result, expected);

        dynamic.truncate(0);
        let result: Vec<i64> = chan.borrow().clone();
        let expected = vec![3, 2, 1];
        assert_eq!(result, expected);

        dynamic.truncate(3);
        let result: Vec<i64> = chan.borrow().clone();
        let expected = vec![3, 2, 1];
        assert_eq!(result, expected);
    }
}
