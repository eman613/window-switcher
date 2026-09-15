//! A single owning slot. Swapping transfers the Box exactly once; no borrowed
//! pointer escapes the slot and no UI operation waits for its consumer.
use std::{
    marker::PhantomData,
    ptr,
    sync::atomic::{AtomicPtr, Ordering},
};

pub(super) struct LatestRequest<T> {
    value: AtomicPtr<T>,
    // Keep Send/Sync constrained by T even though AtomicPtr itself is untyped
    // ownership as far as auto traits are concerned.
    _ownership: PhantomData<Box<T>>,
}

impl<T> LatestRequest<T> {
    pub(super) fn new() -> Self {
        Self {
            value: AtomicPtr::new(ptr::null_mut()),
            _ownership: PhantomData,
        }
    }

    pub(super) fn replace(&self, value: T) {
        let previous = self
            .value
            .swap(Box::into_raw(Box::new(value)), Ordering::AcqRel);
        drop(Self::owned(previous));
    }

    pub(super) fn take(&self) -> Option<T> {
        Self::owned(self.value.swap(ptr::null_mut(), Ordering::AcqRel))
    }

    fn owned(pointer: *mut T) -> Option<T> {
        if pointer.is_null() {
            return None;
        }
        // Every non-null pointer came from Box::into_raw. The atomic swap (or
        // exclusive Drop below) removed it, so only this caller can reclaim it.
        Some(unsafe { *Box::from_raw(pointer) })
    }
}

impl<T> Drop for LatestRequest<T> {
    fn drop(&mut self) {
        drop(Self::owned(*self.value.get_mut()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicBool, AtomicUsize},
        Arc,
    };

    struct Tracked(Arc<AtomicUsize>);
    impl Drop for Tracked {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn replacing_and_consuming_concurrently_releases_every_payload_once() {
        let slot = Arc::new(LatestRequest::new());
        let done = Arc::new(AtomicBool::new(false));
        let dropped = Arc::new(AtomicUsize::new(0));
        let consumer_slot = slot.clone();
        let consumer_done = done.clone();
        let consumer = std::thread::spawn(move || loop {
            if let Some(value) = consumer_slot.take() {
                drop(value);
            } else if consumer_done.load(Ordering::Acquire) {
                break;
            } else {
                std::thread::yield_now();
            }
        });
        for _ in 0..10000 {
            slot.replace(Tracked(dropped.clone()));
        }
        done.store(true, Ordering::Release);
        consumer.join().unwrap();
        drop(slot);
        assert_eq!(dropped.load(Ordering::Acquire), 10000);
    }
}
