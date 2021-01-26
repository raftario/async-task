#![feature(test)]

extern crate test;

use smol::future;
use test::Bencher;

#[bench]
fn task_create(b: &mut Bencher) {
    b.iter(|| {
        let _ = async_task_ffi::spawn(async {}, drop);
    });
}

#[bench]
fn task_run(b: &mut Bencher) {
    b.iter(|| {
        let (runnable, task) = async_task_ffi::spawn(async {}, drop);
        runnable.run();
        future::block_on(task);
    });
}
