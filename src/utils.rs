use core::alloc::Layout;
use core::mem;

/// Aborts the process.
///
/// To abort, this function simply panics while panicking.
pub(crate) fn abort() -> ! {
    struct Panic;

    impl Drop for Panic {
        fn drop(&mut self) {
            panic!("aborting the process");
        }
    }

    let _panic = Panic;
    panic!("aborting the process");
}

/// Calls a function and aborts if it panics.
///
/// This is useful in unsafe code where we can't recover from panics.
#[inline]
pub(crate) fn abort_on_panic<T>(f: impl FnOnce() -> T) -> T {
    struct Bomb;

    impl Drop for Bomb {
        fn drop(&mut self) {
            abort();
        }
    }

    let bomb = Bomb;
    let t = f();
    mem::forget(bomb);
    t
}

/// Returns the layout for `a` followed by `b` and the offset of `b`.
///
/// This function was adapted from the currently unstable `Layout::extend()`:
/// https://doc.rust-lang.org/nightly/std/alloc/struct.Layout.html#method.extend
#[inline]
pub(crate) fn extend(a: Layout, b: Layout) -> (Layout, usize) {
    let new_align = a.align().max(b.align());
    let pad = padding_needed_for(a, b.align());

    let offset = a.size().checked_add(pad).unwrap();
    let new_size = offset.checked_add(b.size()).unwrap();

    let layout = Layout::from_size_align(new_size, new_align).unwrap();
    (layout, offset)
}

/// Returns the padding after `layout` that aligns the following address to `align`.
///
/// This function was adapted from the currently unstable `Layout::padding_needed_for()`:
/// https://doc.rust-lang.org/nightly/std/alloc/struct.Layout.html#method.padding_needed_for
#[inline]
pub(crate) fn padding_needed_for(layout: Layout, align: usize) -> usize {
    let len = layout.size();
    let len_rounded_up = len.wrapping_add(align).wrapping_sub(1) & !align.wrapping_sub(1);
    len_rounded_up.wrapping_sub(len)
}

#[cfg(feature = "std")]
pub(crate) mod checked {
    use std::fmt;
    use std::future::Future;
    use std::mem::ManuallyDrop;
    use std::ops::{Deref, DerefMut};
    use std::pin::Pin;
    use std::task::{Context, Poll};
    use std::thread::{self, ThreadId};

    #[inline]
    fn thread_id() -> ThreadId {
        thread_local! {
            static ID: ThreadId = thread::current().id();
        }
        ID.try_with(|id| *id)
            .unwrap_or_else(|_| thread::current().id())
    }

    pub(crate) struct CheckedFuture<F> {
        id: ThreadId,
        inner: ManuallyDrop<F>,
    }

    impl<F> CheckedFuture<F> {
        pub(crate) fn new(future: F) -> CheckedFuture<F> {
            CheckedFuture {
                id: thread_id(),
                inner: ManuallyDrop::new(future),
            }
        }
    }

    impl<F> Drop for CheckedFuture<F> {
        fn drop(&mut self) {
            assert!(
                self.id == thread_id(),
                "local task dropped by a thread that didn't spawn it"
            );
            unsafe {
                ManuallyDrop::drop(&mut self.inner);
            }
        }
    }

    impl<F: Future> Future for CheckedFuture<F> {
        type Output = F::Output;

        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            assert!(
                self.id == thread_id(),
                "local task polled by a thread that didn't spawn it"
            );
            unsafe { self.map_unchecked_mut(|c| &mut *c.inner).poll(cx) }
        }
    }

    #[derive(Clone)]
    pub struct Checked<D> {
        id: ThreadId,
        inner: ManuallyDrop<D>,
    }

    impl<D> Checked<D> {
        pub(crate) fn new(data: D) -> Checked<D> {
            Checked {
                id: thread_id(),
                inner: ManuallyDrop::new(data),
            }
        }
    }

    impl<D> Drop for Checked<D> {
        fn drop(&mut self) {
            assert!(
                self.id == thread_id(),
                "local task dropped by a thread that didn't spawn it"
            );
            unsafe {
                ManuallyDrop::drop(&mut self.inner);
            }
        }
    }

    impl<D> Deref for Checked<D> {
        type Target = D;

        fn deref(&self) -> &Self::Target {
            assert!(
                self.id == thread_id(),
                "local task accessed by a thread that didn't spawn it"
            );
            &self.inner
        }
    }

    impl<D> DerefMut for Checked<D> {
        fn deref_mut(&mut self) -> &mut Self::Target {
            assert!(
                self.id == thread_id(),
                "local task accessed by a thread that didn't spawn it"
            );
            &mut self.inner
        }
    }
}
