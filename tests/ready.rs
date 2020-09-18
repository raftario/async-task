use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::thread;
use std::time::Duration;

use async_task::Task;
use crossbeam::atomic::AtomicCell;
use futures::executor::block_on;
use futures::future;
use lazy_static::lazy_static;

// Creates a future with event counters.
//
// Usage: `future!(f, POLL, DROP_F, DROP_O)`
//
// The future `f` sleeps for 200 ms and outputs `Poll::Ready`.
// When it gets polled, `POLL` is incremented.
// When it gets dropped, `DROP_F` is incremented.
// When the output gets dropped, `DROP_O` is incremented.
macro_rules! future {
    ($name:pat, $poll:ident, $drop_f:ident, $drop_o:ident) => {
        lazy_static! {
            static ref $poll: AtomicCell<usize> = AtomicCell::new(0);
            static ref $drop_f: AtomicCell<usize> = AtomicCell::new(0);
            static ref $drop_o: AtomicCell<usize> = AtomicCell::new(0);
        }

        let $name = {
            struct Fut(Box<i32>);

            impl Future for Fut {
                type Output = Out;

                fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
                    $poll.fetch_add(1);
                    thread::sleep(ms(400));
                    Poll::Ready(Out(Box::new(0)))
                }
            }

            impl Drop for Fut {
                fn drop(&mut self) {
                    $drop_f.fetch_add(1);
                }
            }

            struct Out(Box<i32>);

            impl Drop for Out {
                fn drop(&mut self) {
                    $drop_o.fetch_add(1);
                }
            }

            Fut(Box::new(0))
        };
    };
}

// Creates a schedule function with event counters.
//
// Usage: `schedule!(s, SCHED, DROP)`
//
// The schedule function `s` does nothing.
// When it gets invoked, `SCHED` is incremented.
// When it gets dropped, `DROP` is incremented.
macro_rules! schedule {
    ($name:pat, $sched:ident, $drop:ident) => {
        lazy_static! {
            static ref $sched: AtomicCell<usize> = AtomicCell::new(0);
            static ref $drop: AtomicCell<usize> = AtomicCell::new(0);
        }

        let $name = {
            struct Guard(Box<i32>);

            impl Drop for Guard {
                fn drop(&mut self) {
                    $drop.fetch_add(1);
                }
            }

            let guard = Guard(Box::new(0));
            move |_task: Task| {
                &guard;
                $sched.fetch_add(1);
            }
        };
    };
}

fn ms(ms: u64) -> Duration {
    Duration::from_millis(ms)
}

#[test]
fn cancel_during_run() {
    future!(f, POLL, DROP_F, DROP_O);
    schedule!(s, SCHEDULE, DROP_S);
    let (task, handle) = async_task::spawn(f, s);

    crossbeam::scope(|scope| {
        scope.spawn(|_| {
            task.run();
            assert_eq!(POLL.load(), 1);
            assert_eq!(SCHEDULE.load(), 0);
            assert_eq!(DROP_F.load(), 1);
            assert_eq!(DROP_S.load(), 1);
            assert_eq!(DROP_O.load(), 1);
        });

        thread::sleep(ms(200));

        assert_eq!(POLL.load(), 1);
        assert_eq!(SCHEDULE.load(), 0);
        assert_eq!(DROP_F.load(), 0);
        assert_eq!(DROP_S.load(), 0);
        assert_eq!(DROP_O.load(), 0);

        drop(handle);
        assert_eq!(POLL.load(), 1);
        assert_eq!(SCHEDULE.load(), 0);
        assert_eq!(DROP_F.load(), 0);
        assert_eq!(DROP_S.load(), 0);
        assert_eq!(DROP_O.load(), 0);

        thread::sleep(ms(400));

        assert_eq!(POLL.load(), 1);
        assert_eq!(SCHEDULE.load(), 0);
        assert_eq!(DROP_F.load(), 1);
        assert_eq!(DROP_S.load(), 1);
        assert_eq!(DROP_O.load(), 1);
    })
    .unwrap();
}

#[test]
fn join_during_run() {
    future!(f, POLL, DROP_F, DROP_O);
    schedule!(s, SCHEDULE, DROP_S);
    let (task, handle) = async_task::spawn(f, s);

    crossbeam::scope(|scope| {
        scope.spawn(|_| {
            task.run();
            assert_eq!(POLL.load(), 1);
            assert_eq!(SCHEDULE.load(), 0);
            assert_eq!(DROP_F.load(), 1);

            thread::sleep(ms(200));

            assert_eq!(DROP_S.load(), 1);
        });

        thread::sleep(ms(200));

        assert!(block_on(handle).is_some());
        assert_eq!(POLL.load(), 1);
        assert_eq!(SCHEDULE.load(), 0);
        assert_eq!(DROP_F.load(), 1);
        assert_eq!(DROP_O.load(), 1);

        thread::sleep(ms(200));

        assert_eq!(DROP_S.load(), 1);
    })
    .unwrap();
}

#[test]
fn try_join_during_run() {
    future!(f, POLL, DROP_F, DROP_O);
    schedule!(s, SCHEDULE, DROP_S);
    let (task, mut handle) = async_task::spawn(f, s);

    crossbeam::scope(|scope| {
        scope.spawn(|_| {
            task.run();
            assert_eq!(POLL.load(), 1);
            assert_eq!(SCHEDULE.load(), 0);
            assert_eq!(DROP_F.load(), 1);
            assert_eq!(DROP_S.load(), 1);
            assert_eq!(DROP_O.load(), 1);
        });

        thread::sleep(ms(200));

        block_on(future::select(&mut handle, future::ready(())));
        assert_eq!(POLL.load(), 1);
        assert_eq!(SCHEDULE.load(), 0);
        assert_eq!(DROP_F.load(), 0);
        assert_eq!(DROP_S.load(), 0);
        assert_eq!(DROP_O.load(), 0);
        drop(handle);
    })
    .unwrap();
}

#[test]
fn detach_during_run() {
    future!(f, POLL, DROP_F, DROP_O);
    schedule!(s, SCHEDULE, DROP_S);
    let (task, handle) = async_task::spawn(f, s);

    crossbeam::scope(|scope| {
        scope.spawn(|_| {
            task.run();
            assert_eq!(POLL.load(), 1);
            assert_eq!(SCHEDULE.load(), 0);
            assert_eq!(DROP_F.load(), 1);
            assert_eq!(DROP_S.load(), 1);
            assert_eq!(DROP_O.load(), 1);
        });

        thread::sleep(ms(200));

        handle.detach();
        assert_eq!(POLL.load(), 1);
        assert_eq!(SCHEDULE.load(), 0);
        assert_eq!(DROP_F.load(), 0);
        assert_eq!(DROP_S.load(), 0);
        assert_eq!(DROP_O.load(), 0);
    })
    .unwrap();
}
