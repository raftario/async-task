use core::fmt;
use core::future::Future;
use core::marker::PhantomData;
use core::mem;
use core::ptr::NonNull;
use core::sync::atomic::Ordering;
use core::task::Waker;

use crate::header::Header;
use crate::raw::RawTask;
use crate::state::*;
use crate::utils::checked::{Checked, CheckedFuture};
use crate::Task;

pub fn spawn<F, S>(future: F, schedule: S) -> (Runnable, Task<F::Output>)
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
    S: Fn(Runnable) + Send + Sync + 'static,
{
    spawn_with(future, schedule, ())
}

/// Creates a new task.
///
/// The returned [`Runnable`] is used to poll the `future`, and the [`Task`] is used to await its
/// output.
///
/// Method [`run()`][`Runnable::run()`] polls the task's future once. Then, the [`Runnable`]
/// vanishes and only reappears when its [`Waker`] wakes the task, thus scheduling it to be run
/// again.
///
/// When the task is woken, its [`Runnable`] is passed to the `schedule` function.
/// The `schedule` function should not attempt to run the [`Runnable`] nor to drop it. Instead, it
/// should push it into a task queue so that it can be processed later.
///
/// If you need to spawn a future that does not implement [`Send`] or isn't `'static`, consider
/// using [`spawn_local()`] or [`spawn_unchecked()`] instead.
///
/// # Examples
///
/// ```
/// // The future inside the task.
/// let future = async {
///     println!("Hello, world!");
/// };
///
/// // A function that schedules the task when it gets woken up.
/// let (s, r) = flume::unbounded();
/// let schedule = move |runnable| s.send(runnable).unwrap();
///
/// // Create a task with the future and the schedule function.
/// let (runnable, task) = async_task::spawn(future, schedule);
/// ```
pub fn spawn_with<F, S, D>(future: F, schedule: S, data: D) -> (Runnable<D>, Task<F::Output>)
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
    S: Fn(Runnable<D>) + Send + Sync + 'static,
    D: Send + 'static,
{
    unsafe { spawn_unchecked_with(future, schedule, data) }
}

#[cfg(feature = "std")]
pub fn spawn_local<F, S>(future: F, schedule: S) -> (Runnable, Task<F::Output>)
where
    F: Future + 'static,
    F::Output: 'static,
    S: Fn(Runnable) + Send + Sync + 'static,
{
    // Wrap the future into one that checks which thread it's on.
    let future = CheckedFuture::new(future);

    unsafe { spawn_unchecked(future, schedule) }
}

/// Creates a new thread-local task.
///
/// This function is same as [`spawn()`], except it does not require [`Send`] on `future`. If the
/// [`Runnable`] is used or dropped on another thread, a panic will occur.
///
/// This function is only available when the `std` feature for this crate is enabled.
///
/// # Examples
///
/// ```
/// use async_task::Runnable;
/// use flume::{Receiver, Sender};
/// use std::rc::Rc;
///
/// thread_local! {
///     // A queue that holds scheduled tasks.
///     static QUEUE: (Sender<Runnable>, Receiver<Runnable>) = flume::unbounded();
/// }
///
/// // Make a non-Send future.
/// let msg: Rc<str> = "Hello, world!".into();
/// let future = async move {
///     println!("{}", msg);
/// };
///
/// // A function that schedules the task when it gets woken up.
/// let s = QUEUE.with(|(s, _)| s.clone());
/// let schedule = move |runnable| s.send(runnable).unwrap();
///
/// // Create a task with the future and the schedule function.
/// let (runnable, task) = async_task::spawn_local(future, schedule);
/// ```
#[cfg(feature = "std")]
pub fn spawn_local_with<F, S, D>(
    future: F,
    schedule: S,
    data: D,
) -> (Runnable<Checked<D>>, Task<F::Output>)
where
    F: Future + 'static,
    F::Output: 'static,
    S: Fn(Runnable<Checked<D>>) + Send + Sync + 'static,
    D: 'static,
{
    // Wrap the future into one that checks which thread it's on.
    let future = CheckedFuture::new(future);

    // Wrap the data the same way
    let data = Checked::new(data);

    unsafe { spawn_unchecked_with(future, schedule, data) }
}

pub unsafe fn spawn_unchecked<F, S>(future: F, schedule: S) -> (Runnable, Task<F::Output>)
where
    F: Future,
    S: Fn(Runnable),
{
    spawn_unchecked_with(future, schedule, ())
}

/// Creates a new task without [`Send`], [`Sync`], and `'static` bounds.
///
/// This function is same as [`spawn()`], except it does not require [`Send`], [`Sync`], and
/// `'static` on `future` and `schedule`.
///
/// Safety requirements:
///
/// - If `future` is not [`Send`], its [`Runnable`] must be used and dropped on the original
///   thread.
/// - If `future` is not `'static`, borrowed variables must outlive its [`Runnable`].
/// - If `schedule` is not [`Send`] and [`Sync`], the task's [`Waker`] must be used and dropped on
///   the original thread.
/// - If `schedule` is not `'static`, borrowed variables must outlive the task's [`Waker`].
///
/// # Examples
///
/// ```
/// // The future inside the task.
/// let future = async {
///     println!("Hello, world!");
/// };
///
/// // If the task gets woken up, it will be sent into this channel.
/// let (s, r) = flume::unbounded();
/// let schedule = move |runnable| s.send(runnable).unwrap();
///
/// // Create a task with the future and the schedule function.
/// let (runnable, task) = unsafe { async_task::spawn_unchecked(future, schedule) };
/// ```
pub unsafe fn spawn_unchecked_with<F, S, D>(
    future: F,
    schedule: S,
    data: D,
) -> (Runnable<D>, Task<F::Output>)
where
    F: Future,
    S: Fn(Runnable<D>),
{
    // Allocate large futures on the heap.
    let ptr = if mem::size_of::<F>() >= 2048 {
        let future = alloc::boxed::Box::pin(future);
        RawTask::<_, F::Output, S, D>::allocate(future, schedule, data)
    } else {
        RawTask::<F, F::Output, S, D>::allocate(future, schedule, data)
    };

    let runnable = Runnable {
        ptr,
        _marker: Default::default(),
    };
    let task = Task {
        ptr,
        _marker: PhantomData,
    };
    (runnable, task)
}

/// A handle to a runnable task.
///
/// Every spawned task has a single [`Runnable`] handle, which only exists when the task is
/// scheduled for running.
///
/// Method [`run()`][`Runnable::run()`] polls the task's future once. Then, the [`Runnable`]
/// vanishes and only reappears when its [`Waker`] wakes the task, thus scheduling it to be run
/// again.
///
/// Dropping a [`Runnable`] cancels the task, which means its future won't be polled again, and
/// awaiting the [`Task`] after that will result in a panic.
///
/// # Examples
///
/// ```
/// use async_task::Runnable;
/// use once_cell::sync::Lazy;
/// use std::{panic, thread};
///
/// // A simple executor.
/// static QUEUE: Lazy<flume::Sender<Runnable>> = Lazy::new(|| {
///     let (sender, receiver) = flume::unbounded::<Runnable>();
///     thread::spawn(|| {
///         for runnable in receiver {
///             let _ignore_panic = panic::catch_unwind(|| runnable.run());
///         }
///     });
///     sender
/// });
///
/// // Create a task with a simple future.
/// let schedule = |runnable| QUEUE.send(runnable).unwrap();
/// let (runnable, task) = async_task::spawn(async { 1 + 2 }, schedule);
///
/// // Schedule the task and await its output.
/// runnable.schedule();
/// assert_eq!(smol::future::block_on(task), 3);
/// ```
pub struct Runnable<D = ()> {
    /// A pointer to the heap-allocated task.
    pub(crate) ptr: NonNull<()>,

    /// A marker capturing generic type `D`.
    pub(crate) _marker: PhantomData<D>,
}

unsafe impl<D: Sync> Send for Runnable<D> {}
unsafe impl<D: Sync> Sync for Runnable<D> {}

#[cfg(feature = "std")]
impl<D: std::panic::RefUnwindSafe> std::panic::UnwindSafe for Runnable<D> {}
#[cfg(feature = "std")]
impl<D: std::panic::RefUnwindSafe> std::panic::RefUnwindSafe for Runnable<D> {}

impl<D> Runnable<D> {
    /// Schedules the task.
    ///
    /// This is a convenience method that passes the [`Runnable`] to the schedule function.
    ///
    /// # Examples
    ///
    /// ```
    /// // A function that schedules the task when it gets woken up.
    /// let (s, r) = flume::unbounded();
    /// let schedule = move |runnable| s.send(runnable).unwrap();
    ///
    /// // Create a task with a simple future and the schedule function.
    /// let (runnable, task) = async_task::spawn(async {}, schedule);
    ///
    /// // Schedule the task.
    /// assert_eq!(r.len(), 0);
    /// runnable.schedule();
    /// assert_eq!(r.len(), 1);
    /// ```
    pub fn schedule(self) {
        let ptr = self.ptr.as_ptr();
        let header = ptr as *const Header;
        mem::forget(self);

        unsafe {
            ((*header).vtable.schedule)(ptr);
        }
    }

    /// Runs the task by polling its future.
    ///
    /// Returns `true` if the task was woken while running, in which case the [`Runnable`] gets
    /// rescheduled at the end of this method invocation. Otherwise, returns `false` and the
    /// [`Runnable`] vanishes until the task is woken.
    /// The return value is just a hint: `true` usually indicates that the task has yielded, i.e.
    /// it woke itself and then gave the control back to the executor.
    ///
    /// If the [`Task`] handle was dropped or if [`cancel()`][`Task::cancel()`] was called, then
    /// this method simply destroys the task.
    ///
    /// If the polled future panics, this method propagates the panic, and awaiting the [`Task`]
    /// after that will also result in a panic.
    ///
    /// # Examples
    ///
    /// ```
    /// // A function that schedules the task when it gets woken up.
    /// let (s, r) = flume::unbounded();
    /// let schedule = move |runnable| s.send(runnable).unwrap();
    ///
    /// // Create a task with a simple future and the schedule function.
    /// let (runnable, task) = async_task::spawn(async { 1 + 2 }, schedule);
    ///
    /// // Run the task and check its output.
    /// runnable.run();
    /// assert_eq!(smol::future::block_on(task), 3);
    /// ```
    pub fn run(self) -> bool {
        let ptr = self.ptr.as_ptr();
        let header = ptr as *const Header;
        mem::forget(self);

        unsafe { ((*header).vtable.run)(ptr) }
    }

    /// Returns a waker associated with this task.
    ///
    /// # Examples
    ///
    /// ```
    /// use smol::future;
    ///
    /// // A function that schedules the task when it gets woken up.
    /// let (s, r) = flume::unbounded();
    /// let schedule = move |runnable| s.send(runnable).unwrap();
    ///
    /// // Create a task with a simple future and the schedule function.
    /// let (runnable, task) = async_task::spawn(future::pending::<()>(), schedule);
    ///
    /// // Take a waker and run the task.
    /// let waker = runnable.waker();
    /// runnable.run();
    ///
    /// // Reschedule the task by waking it.
    /// assert_eq!(r.len(), 0);
    /// waker.wake();
    /// assert_eq!(r.len(), 1);
    /// ```
    pub fn waker(&self) -> Waker {
        let ptr = self.ptr.as_ptr();
        let header = ptr as *const Header;

        unsafe {
            let raw_waker = ((*header).vtable.clone_waker)(ptr);
            Waker::from_raw(raw_waker)
        }
    }

    pub fn data(&self) -> &D {
        let ptr = self.ptr.as_ptr();
        let header = ptr as *const Header;

        unsafe {
            let data = ((*header).vtable.get_data)(ptr) as *const D;
            &*data
        }
    }

    pub fn data_mut(&mut self) -> &mut D {
        let ptr = self.ptr.as_ptr();
        let header = ptr as *const Header;

        unsafe {
            let data = ((*header).vtable.get_data)(ptr) as *mut D;
            &mut *data
        }
    }

    pub fn into_raw(this: Runnable<D>) -> *mut () {
        let ptr = this.ptr;
        mem::forget(this);
        ptr.as_ptr()
    }

    pub unsafe fn from_raw(ptr: *mut ()) -> Runnable<D> {
        Runnable {
            ptr: NonNull::new_unchecked(ptr),
            _marker: Default::default(),
        }
    }
}

impl<D> Drop for Runnable<D> {
    fn drop(&mut self) {
        let ptr = self.ptr.as_ptr() as *mut ();
        let header = ptr as *const Header;

        unsafe {
            let mut state = (*header).state.load(Ordering::Acquire);

            loop {
                // If the task has been completed or closed, it can't be canceled.
                if state & (COMPLETED | CLOSED) != 0 {
                    break;
                }

                // Mark the task as closed.
                match (*header).state.compare_exchange_weak(
                    state,
                    state | CLOSED,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ) {
                    Ok(_) => break,
                    Err(s) => state = s,
                }
            }

            // Drop the future.
            ((*header).vtable.drop_future)(ptr);

            // Mark the task as unscheduled.
            let state = (*header).state.fetch_and(!SCHEDULED, Ordering::AcqRel);

            // Notify the awaiter that the future has been dropped.
            if state & AWAITER != 0 {
                (*header).notify(None);
            }

            // Drop the task reference.
            ((*header).vtable.drop_ref)(ptr);
        }
    }
}

impl<D> fmt::Debug for Runnable<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ptr = self.ptr.as_ptr();
        let header = ptr as *const Header;

        f.debug_struct("Runnable")
            .field("header", unsafe { &(*header) })
            .finish()
    }
}
