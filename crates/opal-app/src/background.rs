//! Work done off the interface thread.

use eframe::egui;
use std::sync::{Arc, Condvar, Mutex, OnceLock, mpsc};

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

/// What a panic said, given what it was made with (`panic!`'s message, as a rule).
pub fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|m| m.to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "a panic".into())
}

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

/// The job waiting for a [`LatestJob`]'s thread, if one is, and what tells the thread so.
type Slot = (Mutex<Option<Job>>, Condvar);

/// A thread that runs only the latest of the jobs given it, one at a time, each inside a pool
/// of its own: they neither wait behind [`pool`]'s work (saves, exports) nor keep it waiting.
/// A job given while another runs replaces any waiting, which is dropped without running.
/// Started when first given a job.
pub struct LatestJob {
    name: &'static str,
    slot: OnceLock<Arc<Slot>>,
}

impl LatestJob {
    pub const fn new(name: &'static str) -> LatestJob {
        LatestJob { name, slot: OnceLock::new() }
    }

    /// Runs `work`, unless another job is given before it begins, then `done` with what it
    /// gave (an error if it panicked), then asks `ctx` to repaint.
    pub fn run<R: Send + 'static>(
        &self,
        work: impl FnOnce() -> R + Send + 'static,
        done: impl FnOnce(std::thread::Result<R>) + Send + 'static,
        ctx: &egui::Context,
    ) {
        let name = self.name;
        let slot = self.slot.get_or_init(|| {
            let slot: Arc<Slot> = Arc::default();
            let theirs = slot.clone();
            std::thread::Builder::new()
                .name(name.into())
                .spawn(move || {
                    let n = std::thread::available_parallelism().map_or(2, |n| n.get());
                    let pool = rayon::ThreadPoolBuilder::new()
                        .num_threads(n)
                        .thread_name(move |i| format!("{name} {i}"))
                        .build()
                        .expect("starting a pool of threads");
                    let (waiting, given) = &*theirs;
                    loop {
                        // The slot let go of before the job runs, for the next to be given.
                        let job = {
                            let mut slot = waiting.lock().unwrap_or_else(|e| e.into_inner());
                            while slot.is_none() {
                                slot = given.wait(slot).unwrap_or_else(|e| e.into_inner());
                            }
                            slot.take().expect("given")
                        };
                        // Jobs catch their own panics; the thread goes on whatever comes.
                        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| pool.install(job)));
                    }
                })
                .expect("starting a background thread");
            slot
        });
        let ctx = ctx.clone();
        let job: Job = Box::new(move || {
            done(std::panic::catch_unwind(std::panic::AssertUnwindSafe(work)));
            ctx.request_repaint();
        });
        let (waiting, given) = &**slot;
        *waiting.lock().unwrap_or_else(|e| e.into_inner()) = Some(job);
        given.notify_one();
    }
}

/// Helpers for tests of work done off the interface thread.
#[cfg(test)]
pub mod testing {
    use std::sync::{Arc, Condvar, Mutex, MutexGuard, mpsc};

    /// Every thread of a pool, kept busy until this is dropped.
    pub struct Held {
        release: Arc<(Mutex<bool>, Condvar)>,
        /// This test's turn to hold a pool (see [`hold`]); let go after the threads.
        _turn: MutexGuard<'static, ()>,
    }

    impl Drop for Held {
        fn drop(&mut self) {
            let (go, c) = &*self.release;
            *go.lock().unwrap_or_else(|e| e.into_inner()) = true;
            c.notify_all();
        }
    }

    /// A test's turn with the pools, which [`hold`] takes, and which a test running work
    /// on a pool that waits for more of the pool's tasks (an export's rows) must take too:
    /// a hold begun meanwhile would take every other thread and wait for that work's, while
    /// the tasks it waits for queue behind the hold's.
    pub fn turn() -> std::sync::MutexGuard<'static, ()> {
        static TURN: Mutex<()> = Mutex::new(());
        TURN.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Holds every thread of `pool` (rayon's global pool if `None`), returning once they
    /// all are. One test holds a pool at a time, whichever pool ([`turn`]): two holding one
    /// at once would each take some of its threads and wait for the rest forever, and two
    /// holding different ones may each need the other's to go on. (So no test holds two.)
    pub fn hold(pool: Option<&rayon::ThreadPool>) -> Held {
        let turn = turn();
        let n = pool.map_or_else(rayon::current_num_threads, |p| p.current_num_threads());
        let (started, all_started) = mpsc::channel();
        let release = Arc::new((Mutex::new(false), Condvar::new()));
        for _ in 0..n {
            let (started, release) = (started.clone(), release.clone());
            let wait = move || {
                started.send(()).unwrap();
                let (go, c) = &*release;
                let mut g = go.lock().unwrap_or_else(|e| e.into_inner());
                while !*g {
                    g = c.wait(g).unwrap_or_else(|e| e.into_inner());
                }
            };
            match pool {
                Some(p) => p.spawn(wait),
                None => rayon::spawn(wait),
            }
        }
        for _ in 0..n {
            all_started.recv().unwrap();
        }
        Held { release, _turn: turn }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rayon::prelude::*;
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
        let held = testing::hold(None);
        let mut t = JobThread::new("test jobs");
        let (tx, rx) = mpsc::channel();
        let numbers: Vec<u64> = (0..100_000).collect();
        let work = move || numbers.par_iter().sum::<u64>();
        t.run(work, move |r| tx.send(r.ok()).unwrap(), &egui::Context::default());
        let got = rx.recv_timeout(Duration::from_secs(10));
        drop(held);
        assert_eq!(got.ok().flatten(), Some(4_999_950_000), "the job waited for the global pool");
    }

    #[test]
    fn of_the_jobs_given_meanwhile_only_the_latest_runs_and_off_the_background_pool() {
        // Every thread of the background pool held while the jobs run.
        let held = testing::hold(Some(pool()));
        let t = LatestJob::new("test latest");
        let ctx = egui::Context::default();
        let (tx, rx) = mpsc::channel();
        // The first keeps the thread until let go, while two more are given.
        let (began, go) = (mpsc::channel::<()>(), mpsc::channel::<()>());
        let (first_began, release) = (began.0, go.1);
        let first = tx.clone();
        let work = move || {
            first_began.send(()).unwrap();
            release.recv().unwrap();
            1
        };
        t.run(work, move |r| first.send(r.ok()).unwrap(), &ctx);
        began.1.recv_timeout(Duration::from_secs(10)).expect("the first job never began");
        for i in [2, 3] {
            let tx = tx.clone();
            let numbers: Vec<u64> = (0..100_000).collect();
            let work = move || (numbers.par_iter().sum::<u64>() == 4_999_950_000).then_some(i);
            t.run(work, move |r| tx.send(r.ok().flatten()).unwrap(), &ctx);
        }
        go.0.send(()).unwrap();
        let got: Vec<Option<i32>> = (0..2).map(|_| rx.recv_timeout(Duration::from_secs(10)).ok().flatten()).collect();
        drop(held);
        assert_eq!(got, [Some(1), Some(3)], "the latest did not run, or waited for the background pool");
        assert!(rx.recv_timeout(Duration::from_millis(200)).is_err(), "the job replaced ran");
    }

    #[test]
    fn a_latest_job_s_panic_is_reported_and_the_thread_goes_on() {
        let t = LatestJob::new("test latest");
        let ctx = egui::Context::default();
        let (tx, rx) = mpsc::channel();
        for i in [1, 2] {
            let tx = tx.clone();
            let work = move || {
                assert!(i != 1, "job 1 fails");
                i
            };
            t.run(work, move |r| tx.send(r.ok()).unwrap(), &ctx);
            assert_eq!(rx.recv_timeout(Duration::from_secs(10)), Ok((i == 2).then_some(i)));
        }
    }
}
