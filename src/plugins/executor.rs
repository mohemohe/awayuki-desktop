use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, VecDeque};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use boa_engine::context::time::{JsDuration, JsInstant};
use boa_engine::job::{
    GenericJob, IntervalJob, Job, JobExecutor, NativeAsyncJob, PromiseJob, TimeoutJob,
};
use boa_engine::{Context, JsNativeError, JsResult};

use super::fetcher::TOTAL_TIMEOUT;

const MAX_JOBS_PER_TURN: usize = 128;

#[derive(Debug)]
enum ClockJob {
    Timeout(TimeoutJob),
    Interval(IntervalJob),
}

impl ClockJob {
    fn cancelled(&self) -> bool {
        match self {
            Self::Timeout(job) => job.cancelled(),
            Self::Interval(job) => job.cancelled(),
        }
    }
}

/// A bounded event-loop turn for the plugin actor.
///
/// Boa's `SimpleJobExecutor` waits for its entire clock queue to become empty,
/// which means an active `setInterval` prevents it from returning. The plugin
/// actor instead needs to regain control after every turn so it can receive
/// reload, unload, and shutdown requests. This executor therefore runs all
/// immediately runnable jobs and only the timers that are already due.
#[derive(Debug, Default)]
pub(super) struct TurnJobExecutor {
    promise_jobs: RefCell<VecDeque<PromiseJob>>,
    async_jobs: RefCell<VecDeque<NativeAsyncJob>>,
    generic_jobs: RefCell<VecDeque<GenericJob>>,
    clock_jobs: RefCell<BTreeMap<JsInstant, Vec<ClockJob>>>,
    completed_jobs: Cell<u64>,
    execution_deadline: Cell<Option<Instant>>,
    control_error: Cell<Option<JobControlError>>,
    stop: Arc<AtomicBool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum JobControlError {
    Deadline,
    Stopped,
}

impl TurnJobExecutor {
    pub(super) fn with_stop(stop: Arc<AtomicBool>) -> Self {
        Self {
            stop,
            ..Self::default()
        }
    }

    pub(super) fn set_execution_deadline(&self, deadline: Instant) {
        self.execution_deadline.set(Some(deadline));
        self.control_error.set(None);
    }

    pub(super) fn clear_execution_deadline(&self) {
        self.execution_deadline.set(None);
    }

    pub(super) fn take_control_error(&self) -> Option<JobControlError> {
        self.control_error.take()
    }

    pub(super) fn completed_jobs(&self) -> u64 {
        self.completed_jobs.get()
    }

    pub(super) fn has_pending_immediate_jobs(&self) -> bool {
        !self.promise_jobs.borrow().is_empty()
            || !self.async_jobs.borrow().is_empty()
            || !self.generic_jobs.borrow().is_empty()
    }

    fn record_completed_job(&self) {
        self.completed_jobs
            .set(self.completed_jobs.get().wrapping_add(1));
    }

    fn prune_cancelled_clock_jobs(&self) {
        self.clock_jobs.borrow_mut().retain(|_, jobs| {
            jobs.retain(|job| !job.cancelled());
            !jobs.is_empty()
        });
    }

    #[cfg(test)]
    fn clock_job_count(&self) -> usize {
        self.clock_jobs.borrow().values().map(Vec::len).sum()
    }

    #[cfg(test)]
    fn async_job_count(&self) -> usize {
        self.async_jobs.borrow().len()
    }

    fn check_execution_control(&self, before_blocking_async_job: bool) -> JsResult<()> {
        let error = if self.stop.load(Ordering::Acquire) {
            Some(JobControlError::Stopped)
        } else if let Some(deadline) = self.execution_deadline.get() {
            let remaining = deadline.saturating_duration_since(Instant::now());
            (remaining.is_zero() || (before_blocking_async_job && remaining < TOTAL_TIMEOUT))
                .then_some(JobControlError::Deadline)
        } else {
            None
        };
        if let Some(error) = error {
            self.control_error.set(Some(error));
            let message = match error {
                JobControlError::Deadline => {
                    "plugin job turn cannot finish before its absolute deadline"
                }
                JobControlError::Stopped => "plugin runtime is shutting down",
            };
            return Err(JsNativeError::error().with_message(message).into());
        }
        Ok(())
    }

    fn drain_immediate_jobs(
        &self,
        context: &mut Context,
        budget: &mut usize,
        async_job_ran: &mut bool,
    ) -> JsResult<()> {
        while *budget > 0 {
            let mut progressed = false;

            if !*async_job_ran && !self.async_jobs.borrow().is_empty() {
                self.check_execution_control(true)?;
                let job = self
                    .async_jobs
                    .borrow_mut()
                    .pop_front()
                    .expect("async queue was checked as non-empty");
                progressed = true;
                *async_job_ran = true;
                *budget -= 1;
                // Boa's bundled reqwest fetcher performs the HTTP operation in
                // its blocking backend before completing this future. Keeping
                // it on the dedicated plugin thread prevents UI-thread stalls.
                let context_cell = RefCell::new(&mut *context);
                let result = futures::executor::block_on(job.call(&context_cell));
                self.record_completed_job();
                result?;
            }

            if *budget > 0 && !self.promise_jobs.borrow().is_empty() {
                self.check_execution_control(false)?;
                let promise_job = self.promise_jobs.borrow_mut().pop_front();
                if let Some(job) = promise_job {
                    progressed = true;
                    *budget -= 1;
                    let result = job.call(context);
                    self.record_completed_job();
                    result?;
                }
            }

            if *budget > 0 && !self.generic_jobs.borrow().is_empty() {
                self.check_execution_control(false)?;
                let generic_job = self.generic_jobs.borrow_mut().pop_front();
                if let Some(job) = generic_job {
                    progressed = true;
                    *budget -= 1;
                    let result = job.call(context);
                    self.record_completed_job();
                    result?;
                }
            }

            if !progressed {
                break;
            }
        }
        Ok(())
    }

    fn take_due_clock_jobs(&self, now: JsInstant) -> VecDeque<(JsInstant, ClockJob)> {
        let keys = self
            .clock_jobs
            .borrow()
            .range(..=now)
            .map(|(instant, _)| *instant)
            .collect::<Vec<_>>();

        let mut due = VecDeque::new();
        let mut jobs = self.clock_jobs.borrow_mut();
        for key in keys {
            if let Some(at_instant) = jobs.remove(&key) {
                due.extend(at_instant.into_iter().map(|job| (key, job)));
            }
        }
        due
    }

    fn restore_due_clock_jobs(&self, due: VecDeque<(JsInstant, ClockJob)>) {
        let mut clock_jobs = self.clock_jobs.borrow_mut();
        for (instant, job) in due {
            clock_jobs.entry(instant).or_default().push(job);
        }
    }

    fn run_due_clock_jobs(&self, context: &mut Context, budget: &mut usize) -> JsResult<()> {
        let now = context.clock().now();
        let mut due = self.take_due_clock_jobs(now);
        while let Some((instant, job)) = due.pop_front() {
            if job.cancelled() {
                continue;
            }
            if *budget == 0 {
                due.push_front((instant, job));
                self.restore_due_clock_jobs(due);
                break;
            }
            if let Err(error) = self.check_execution_control(false) {
                due.push_front((instant, job));
                self.restore_due_clock_jobs(due);
                return Err(error);
            }
            *budget -= 1;

            match job {
                ClockJob::Timeout(job) => {
                    let result = job.call(context);
                    self.record_completed_job();
                    if let Err(error) = result {
                        self.restore_due_clock_jobs(due);
                        return Err(error);
                    }
                }
                ClockJob::Interval(job) => {
                    let result = job.call(context);
                    self.record_completed_job();
                    if let Err(error) = result {
                        self.restore_due_clock_jobs(due);
                        return Err(error);
                    }
                    if !job.cancelled() {
                        // Clamp a zero interval so one plugin cannot starve the
                        // actor by becoming due again in every CPU cycle.
                        let interval = if job.interval().as_millis() == 0 {
                            JsDuration::from_millis(1)
                        } else {
                            job.interval()
                        };
                        self.clock_jobs
                            .borrow_mut()
                            .entry(context.clock().now() + interval)
                            .or_default()
                            .push(ClockJob::Interval(job));
                    }
                }
            }
        }
        Ok(())
    }
}

impl JobExecutor for TurnJobExecutor {
    fn enqueue_job(self: Rc<Self>, job: Job, context: &mut Context) {
        match job {
            Job::PromiseJob(job) => self.promise_jobs.borrow_mut().push_back(job),
            Job::AsyncJob(job) => self.async_jobs.borrow_mut().push_back(job),
            Job::TimeoutJob(job) => {
                self.clock_jobs
                    .borrow_mut()
                    .entry(context.clock().now() + job.timeout())
                    .or_default()
                    .push(ClockJob::Timeout(job));
            }
            Job::IntervalJob(job) => {
                let interval = if job.interval().as_millis() == 0 {
                    JsDuration::from_millis(1)
                } else {
                    job.interval()
                };
                self.clock_jobs
                    .borrow_mut()
                    .entry(context.clock().now() + interval)
                    .or_default()
                    .push(ClockJob::Interval(job));
            }
            Job::GenericJob(job) => self.generic_jobs.borrow_mut().push_back(job),
            // FinalizationRegistry cleanup is explicitly optional for hosts and
            // must not keep this event-loop turn alive.
            Job::FinalizationRegistryCleanupJob(_) => {}
            _ => {}
        }
    }

    fn run_jobs(self: Rc<Self>, context: &mut Context) -> JsResult<()> {
        self.prune_cancelled_clock_jobs();
        let mut budget = MAX_JOBS_PER_TURN;
        let mut async_job_ran = false;
        let result = (|| {
            self.check_execution_control(false)?;
            self.drain_immediate_jobs(context, &mut budget, &mut async_job_ran)?;
            self.run_due_clock_jobs(context, &mut budget)?;
            // Timer callbacks commonly queue PromiseJobs/queueMicrotask work;
            // those belong to the same turn's microtask checkpoint.
            self.drain_immediate_jobs(context, &mut budget, &mut async_job_ran)
        })();
        self.prune_cancelled_clock_jobs();
        context.clear_kept_objects();
        result
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;
    use std::time::{Duration, Instant};

    use boa_engine::job::{IntervalJob, Job, JobExecutor, NativeAsyncJob, TimeoutJob};
    use boa_engine::{Context, JsValue};

    use super::{JobControlError, TurnJobExecutor};

    struct DropFlag(Rc<Cell<bool>>);

    impl Drop for DropFlag {
        fn drop(&mut self) {
            self.0.set(true);
        }
    }

    #[test]
    fn cancelled_far_future_clock_jobs_release_their_callbacks_on_the_next_turn() {
        let executor = Rc::new(TurnJobExecutor::default());
        let mut context = Context::builder()
            .job_executor(executor.clone())
            .build()
            .expect("test context should build");
        let timeout_dropped = Rc::new(Cell::new(false));
        let interval_dropped = Rc::new(Cell::new(false));

        let timeout_guard = DropFlag(Rc::clone(&timeout_dropped));
        let timeout = TimeoutJob::from_duration(
            move |_| {
                let _guard = &timeout_guard;
                Ok(JsValue::undefined())
            },
            Duration::from_secs(60 * 60),
        );
        let timeout_cancellation = timeout.cancellation_token().clone();
        executor
            .clone()
            .enqueue_job(Job::TimeoutJob(timeout), &mut context);

        let interval_guard = DropFlag(Rc::clone(&interval_dropped));
        let interval = IntervalJob::from_duration(
            move |_| {
                let _guard = &interval_guard;
                Ok(JsValue::undefined())
            },
            Duration::from_secs(60 * 60),
        );
        let interval_cancellation = interval.cancellation_token().clone();
        executor
            .clone()
            .enqueue_job(Job::IntervalJob(interval), &mut context);

        assert_eq!(executor.clock_job_count(), 2);
        timeout_cancellation.cancel(&mut context);
        interval_cancellation.cancel(&mut context);
        assert!(!timeout_dropped.get());
        assert!(!interval_dropped.get());

        executor
            .clone()
            .run_jobs(&mut context)
            .expect("pruning cancelled jobs should succeed");

        assert_eq!(executor.clock_job_count(), 0);
        assert!(timeout_dropped.get());
        assert!(interval_dropped.get());
    }

    #[test]
    fn a_turn_runs_at_most_one_native_async_job() {
        let executor = Rc::new(TurnJobExecutor::default());
        let mut context = Context::builder()
            .job_executor(executor.clone())
            .build()
            .expect("test context should build");
        let completed = Rc::new(Cell::new(0_u8));
        for _ in 0..2 {
            let completed = Rc::clone(&completed);
            executor.clone().enqueue_job(
                Job::AsyncJob(NativeAsyncJob::new(async move |_| {
                    completed.set(completed.get() + 1);
                    Ok(JsValue::undefined())
                })),
                &mut context,
            );
        }

        executor
            .clone()
            .run_jobs(&mut context)
            .expect("first bounded turn should run");
        assert_eq!(completed.get(), 1);
        assert_eq!(executor.async_job_count(), 1);

        executor
            .clone()
            .run_jobs(&mut context)
            .expect("second bounded turn should run");
        assert_eq!(completed.get(), 2);
        assert_eq!(executor.async_job_count(), 0);
    }

    #[test]
    fn a_blocking_async_job_is_not_started_without_time_for_its_bounded_io() {
        let executor = Rc::new(TurnJobExecutor::default());
        let mut context = Context::builder()
            .job_executor(executor.clone())
            .build()
            .expect("test context should build");
        let started = Rc::new(Cell::new(false));
        let job_started = Rc::clone(&started);
        executor.clone().enqueue_job(
            Job::AsyncJob(NativeAsyncJob::new(async move |_| {
                job_started.set(true);
                Ok(JsValue::undefined())
            })),
            &mut context,
        );
        executor.set_execution_deadline(Instant::now() + Duration::from_millis(25));

        let before = Instant::now();
        executor
            .clone()
            .run_jobs(&mut context)
            .expect_err("insufficient blocking-I/O budget must stop the turn");

        assert!(before.elapsed() < Duration::from_secs(1));
        assert!(!started.get());
        assert_eq!(executor.async_job_count(), 1);
        assert_eq!(
            executor.take_control_error(),
            Some(JobControlError::Deadline)
        );
    }
}
