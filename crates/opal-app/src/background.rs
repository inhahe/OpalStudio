//! Work done off the interface thread.

use eframe::egui;
use std::sync::{OnceLock, mpsc};

/// The threads background work runs its parallel parts on (projections, saves and exports,
/// copies): a pool apart from rayon's global one, which the interface thread uses (for a
/// layer's outline, say), so that a small parallel job there never waits behind them. At
/// least two, so that a job waiting for another one's work (an export's encoder waiting for
/// its bands) never holds the only thread.
pub fn pool() -> &'static rayon::ThreadPool {
    static POOL: OnceLock<rayon::ThreadPool> = OnceLock::new();
    POOL.get_or_init(|| {
        let n = std::thread::available_parallelism().map_or(2, |n| n.get().max(2));
        rayon::ThreadPoolBuilder::new()
            .num_threads(n)
            .thread_name(|i| format!("background {i}"))
            .build()
            .expect("starting the background threads")
    })
}

type Job = Box<dyn FnOnce() + Send>;

/// A thread that runs jobs one after another, in the order given, each inside [`pool`];
/// started when first needed.
pub struct JobThread {
    name: &'static str,
    jobs: Option<mpsc::Sender<Job>>,
}

impl JobThread {
    pub const fn new(name: &'static str) -> JobThread {
        JobThread { name, jobs: None }
    }

    /// Runs `work` once the jobs given before it are done, then `done` with what it gave
    /// (an error if it panicked), then asks `ctx` to repaint.
    pub fn run<R: Send + 'static>(
        &mut self,
        work: impl FnOnce() -> R + Send + 'static,
        done: impl FnOnce(std::thread::Result<R>) + Send + 'static,
        ctx: &egui::Context,
    ) {
        let ctx = ctx.clone();
        let mut job: Job = Box::new(move || {
            done(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| pool().install(work))));
            ctx.request_repaint();
        });
        // A thread that has stopped (it cannot, as jobs do not panic out of it, but if it
        // did) is started again.
        for _ in 0..2 {
            let name = self.name;
            let jobs = self.jobs.get_or_insert_with(|| {
                let (tx, rx) = mpsc::channel::<Job>();
                std::thread::Builder::new()
                    .name(name.into())
                    .spawn(move || rx.into_iter().for_each(|job| job()))
                    .expect("starting a background thread");
                tx
            });
            match jobs.send(job) {
                Ok(()) => return,
                Err(mpsc::SendError(back)) => {
                    job = back;
                    self.jobs = None;
                }
            }
        }
        // Not to be lost however unlikely this is: done here, then.
        job();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rayon::prelude::*;
    use std::sync::{Arc, Condvar, Mutex};
    use std::time::Duration;

    #[test]
    fn jobs_run_in_order_and_a_panic_is_reported() {
        let mut t = JobThread::new("test jobs");
        let ctx = egui::Context::default();
        let (tx, rx) = mpsc::channel();
        for i in 0..5 {
            let tx = tx.clone();
            let work = move || {
                assert!(i != 2, "job 2 fails");
                i
            };
            t.run(work, move |r| tx.send(r.ok()).unwrap(), &ctx);
        }
        let got: Vec<Option<i32>> = (0..5).map(|_| rx.recv_timeout(Duration::from_secs(10)).unwrap()).collect();
        assert_eq!(got, [Some(0), Some(1), None, Some(3), Some(4)]);
    }

    #[test]
    fn jobs_do_their_parallel_work_off_the_global_pool() {
        // Every thread of rayon's global pool held while the job runs.
        let n = rayon::current_num_threads();
        let (started, all_started) = mpsc::channel();
        let release = Arc::new((Mutex::new(false), Condvar::new()));
        for _ in 0..n {
            let (started, release) = (started.clone(), release.clone());
            rayon::spawn(move || {
                started.send(()).unwrap();
                let (m, c) = &*release;
                let mut go = m.lock().unwrap();
                while !*go {
                    go = c.wait(go).unwrap();
                }
            });
        }
        for _ in 0..n {
            all_started.recv().unwrap();
        }
        let mut t = JobThread::new("test jobs");
        let (tx, rx) = mpsc::channel();
        let numbers: Vec<u64> = (0..100_000).collect();
        let work = move || numbers.par_iter().sum::<u64>();
        t.run(work, move |r| tx.send(r.ok()).unwrap(), &egui::Context::default());
        let got = rx.recv_timeout(Duration::from_secs(10));
        let (m, c) = &*release;
        *m.lock().unwrap() = true;
        c.notify_all();
        assert_eq!(got.ok().flatten(), Some(4_999_950_000), "the job waited for the global pool");
    }
}
