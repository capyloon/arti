//! Utilities for dealing with periodic recurring tasks.

use crate::SleepProvider;
use futures::channel::mpsc;
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender};
use futures::{Stream, StreamExt};
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use pin_project::pin_project;

/// A command sent from task handles to schedule objects.
#[derive(Copy, Clone)]
enum SchedulerCommand {
    /// Run the task now.
    Fire,
    /// Run the task at the provided `Instant`.
    FireAt(Instant),
    /// Cancel a pending execution, if there is one.
    Cancel,
}

/// A remotely-controllable trigger for recurring tasks.
///
/// This implements [`Stream`], and is intended to be used in a `while` loop; you should
/// wrap your recurring task in a `while schedule.next().await.is_some()` or similar.
#[pin_project(project = TaskScheduleP)]
pub struct TaskSchedule<R: SleepProvider> {
    /// If we're waiting for a deadline to expire, the future for that.
    sleep: Option<Pin<Box<R::SleepFuture>>>,
    /// Receiver of scheduler commands from handles.
    rx: UnboundedReceiver<SchedulerCommand>,
    /// Runtime.
    rt: R,
    /// Whether or not to yield a result immediately when polled, once.
    ///
    /// This is used to avoid having to create a `SleepFuture` with zero duration,
    /// which is potentially a bit wasteful.
    instant_fire: bool,
}

/// A handle used to control a [`TaskSchedule`].
#[derive(Clone)]
pub struct TaskHandle {
    /// Sender of scheduler commands to the corresponding schedule.
    tx: UnboundedSender<SchedulerCommand>,
}

impl<R: SleepProvider> TaskSchedule<R> {
    /// Create a new schedule, and corresponding handle.
    pub fn new(rt: R) -> (Self, TaskHandle) {
        let (tx, rx) = mpsc::unbounded();
        (
            Self {
                sleep: None,
                rx,
                rt,
                // Start off ready.
                instant_fire: true,
            },
            TaskHandle { tx },
        )
    }

    /// Trigger the schedule after `dur`.
    pub fn fire_in(&mut self, dur: Duration) {
        self.instant_fire = false;
        self.sleep = Some(Box::pin(self.rt.sleep(dur)));
    }

    /// Trigger the schedule instantly.
    pub fn fire(&mut self) {
        self.instant_fire = true;
        self.sleep = None;
    }
}

impl TaskHandle {
    /// Trigger this handle's corresponding schedule now.
    ///
    /// Returns `true` if the schedule still exists, and `false` otherwise.
    pub fn fire(&self) -> bool {
        self.tx.unbounded_send(SchedulerCommand::Fire).is_ok()
    }
    /// Trigger this handle's corresponding schedule at `instant`.
    ///
    /// Returns `true` if the schedule still exists, and `false` otherwise.
    pub fn fire_at(&self, instant: Instant) -> bool {
        self.tx
            .unbounded_send(SchedulerCommand::FireAt(instant))
            .is_ok()
    }
    /// Cancel a pending firing of the handle's corresponding schedule.
    ///
    /// Returns `true` if the schedule still exists, and `false` otherwise.
    pub fn cancel(&self) -> bool {
        self.tx.unbounded_send(SchedulerCommand::Cancel).is_ok()
    }
}

// NOTE(eta): implemented on the *pin projection*, not the original type, because we don't want
//            to require `R: Unpin`. Accordingly, all the fields are mutable references.
impl<R: SleepProvider> TaskScheduleP<'_, R> {
    /// Handle an internal command.
    fn handle_command(&mut self, cmd: SchedulerCommand) {
        match cmd {
            SchedulerCommand::Fire => {
                *self.instant_fire = true;
                *self.sleep = None;
            }
            SchedulerCommand::FireAt(instant) => {
                let now = self.rt.now();
                let dur = instant.saturating_duration_since(now);
                *self.instant_fire = false;
                *self.sleep = Some(Box::pin(self.rt.sleep(dur)));
            }
            SchedulerCommand::Cancel => {
                *self.instant_fire = false;
                *self.sleep = None;
            }
        }
    }
}

impl<R: SleepProvider> Stream for TaskSchedule<R> {
    type Item = ();

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut this = self.project();
        while let Poll::Ready(maybe_cmd) = this.rx.poll_next_unpin(cx) {
            match maybe_cmd {
                Some(c) => this.handle_command(c),
                None => {
                    // All task handles dropped; return end of stream.
                    return Poll::Ready(None);
                }
            }
        }
        if *this.instant_fire {
            *this.instant_fire = false;
            return Poll::Ready(Some(()));
        }
        if this
            .sleep
            .as_mut()
            .map(|x| x.as_mut().poll(cx).is_ready())
            .unwrap_or(false)
        {
            *this.sleep = None;
            return Poll::Ready(Some(()));
        }
        Poll::Pending
    }
}

// test_with_all_runtimes! only exists if these features are satisfied.
#[cfg(all(
    test,
    any(feature = "native-tls", feature = "rustls"),
    any(feature = "tokio", feature = "async-std"),
))]
mod test {
    use crate::scheduler::TaskSchedule;
    use crate::{test_with_all_runtimes, SleepProvider};
    use futures::FutureExt;
    use futures::StreamExt;
    use std::time::{Duration, Instant};

    #[test]
    fn it_fires_immediately() {
        test_with_all_runtimes!(|rt| async move {
            let (mut sch, _hdl) = TaskSchedule::new(rt);
            assert!(sch.next().now_or_never().is_some());
        });
    }

    #[test]
    #[allow(clippy::unwrap_used)]
    fn it_dies_if_dropped() {
        test_with_all_runtimes!(|rt| async move {
            let (mut sch, hdl) = TaskSchedule::new(rt);
            drop(hdl);
            assert!(sch.next().now_or_never().unwrap().is_none());
        });
    }

    #[test]
    fn it_fires_on_demand() {
        test_with_all_runtimes!(|rt| async move {
            let (mut sch, hdl) = TaskSchedule::new(rt);
            assert!(sch.next().now_or_never().is_some());

            assert!(sch.next().now_or_never().is_none());
            assert!(hdl.fire());
            assert!(sch.next().now_or_never().is_some());
            assert!(sch.next().now_or_never().is_none());
        });
    }

    #[test]
    fn it_cancels_instant_firings() {
        // NOTE(eta): this test very much assumes that unbounded channels will
        //            transmit things instantly. If it breaks, that's probably why.
        test_with_all_runtimes!(|rt| async move {
            let (mut sch, hdl) = TaskSchedule::new(rt);
            assert!(sch.next().now_or_never().is_some());

            assert!(sch.next().now_or_never().is_none());
            assert!(hdl.fire());
            assert!(hdl.cancel());
            assert!(sch.next().now_or_never().is_none());
        });
    }

    #[test]
    fn it_fires_after_self_reschedule() {
        test_with_all_runtimes!(|rt| async move {
            let (mut sch, _hdl) = TaskSchedule::new(rt);
            assert!(sch.next().now_or_never().is_some());

            sch.fire_in(Duration::from_millis(100));

            assert!(sch.next().now_or_never().is_none());
            assert!(sch.next().await.is_some());
            assert!(sch.next().now_or_never().is_none());
        });
    }

    #[test]
    fn it_fires_after_external_reschedule() {
        test_with_all_runtimes!(|rt| async move {
            let (mut sch, hdl) = TaskSchedule::new(rt);
            assert!(sch.next().now_or_never().is_some());

            hdl.fire_at(Instant::now() + Duration::from_millis(100));

            assert!(sch.next().now_or_never().is_none());
            assert!(sch.next().await.is_some());
            assert!(sch.next().now_or_never().is_none());
        });
    }

    #[test]
    fn it_cancels_delayed_firings() {
        test_with_all_runtimes!(|rt| async move {
            let (mut sch, hdl) = TaskSchedule::new(rt.clone());
            assert!(sch.next().now_or_never().is_some());

            hdl.fire_at(Instant::now() + Duration::from_millis(100));

            assert!(sch.next().now_or_never().is_none());

            rt.sleep(Duration::from_millis(50)).await;

            assert!(sch.next().now_or_never().is_none());

            hdl.cancel();

            assert!(sch.next().now_or_never().is_none());

            rt.sleep(Duration::from_millis(100)).await;

            assert!(sch.next().now_or_never().is_none());
        });
    }

    #[test]
    fn last_fire_wins() {
        test_with_all_runtimes!(|rt| async move {
            let (mut sch, hdl) = TaskSchedule::new(rt.clone());
            assert!(sch.next().now_or_never().is_some());

            hdl.fire_at(Instant::now() + Duration::from_millis(100));
            hdl.fire();

            assert!(sch.next().now_or_never().is_some());
            assert!(sch.next().now_or_never().is_none());

            rt.sleep(Duration::from_millis(150)).await;

            assert!(sch.next().now_or_never().is_none());
        });
    }
}
