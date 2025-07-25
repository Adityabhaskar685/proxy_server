pub mod scopeguard {
    pub struct ScopeGuard<T, F: FnOnce(T)> {
        value: Option<T>,
        dropfn: Option<F>,
    }

    impl<T, F: FnOnce(T)> Drop for ScopeGuard<T, F> {
        fn drop(&mut self) {
            if let (Some(value), Some(dropfn)) = (self.value.take(), self.dropfn.take()) {
                dropfn(value);
            }
        }
    }

    pub fn guard<T, F: FnOnce(T)>(value: T, dropfn: F) -> ScopeGuard<T, F> {
        ScopeGuard {
            value: Some(value),
            dropfn: Some(dropfn),
        }
    }
}
