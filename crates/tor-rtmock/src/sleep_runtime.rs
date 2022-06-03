//! Declare MockSleepRuntime.

use crate::time::MockSleepProvider;
use tor_rtcompat::{BlockOn, Runtime, SleepProvider, TcpProvider, TlsProvider, UdpProvider};

use async_trait::async_trait;
use futures::task::{FutureObj, Spawn, SpawnError};
use futures::Future;
use pin_project::pin_project;
use std::io::Result as IoResult;
use std::net::SocketAddr;
use std::time::{Duration, Instant, SystemTime};
use tracing::trace;

/// A wrapper Runtime that overrides the SleepProvider trait for the
/// underlying runtime.
#[derive(Clone, Debug)]
pub struct MockSleepRuntime<R: Runtime> {
    /// The underlying runtime. Most calls get delegated here.
    runtime: R,
    /// A MockSleepProvider.  Time-related calls get delegated here.
    sleep: MockSleepProvider,
}

impl<R: Runtime> MockSleepRuntime<R> {
    /// Create a new runtime that wraps `runtime`, but overrides
    /// its view of time with a [`MockSleepProvider`].
    pub fn new(runtime: R) -> Self {
        let sleep = MockSleepProvider::new(SystemTime::now());
        MockSleepRuntime { runtime, sleep }
    }

    /// Return a reference to the underlying runtime.
    pub fn inner(&self) -> &R {
        &self.runtime
    }

    /// Return a reference to the [`MockSleepProvider`]
    pub fn mock_sleep(&self) -> &MockSleepProvider {
        &self.sleep
    }

    /// See [`MockSleepProvider::advance()`]
    pub async fn advance(&self, dur: Duration) {
        self.sleep.advance(dur).await;
    }
    /// See [`MockSleepProvider::jump_to()`]
    pub fn jump_to(&self, new_wallclock: SystemTime) {
        self.sleep.jump_to(new_wallclock);
    }
    /// Run a future under mock time, advancing time forward where necessary until it completes.
    /// Users of this function should read the whole of this documentation before using!
    ///
    /// The returned future will run `fut`, expecting it to create `Sleeping` futures (as returned
    /// by `MockSleepProvider::sleep()` and similar functions). When all such created futures have
    /// been polled (indicating the future is waiting on them), time will be advanced in order that
    /// the first (or only) of said futures returns `Ready`. This process then repeats until `fut`
    /// returns `Ready` itself (as in, the returned wrapper future will wait for all created
    /// `Sleeping` futures to be polled, and advance time again).
    ///
    /// **Note:** The above described algorithm interacts poorly with futures that spawn
    /// asynchronous background tasks, or otherwise expect work to complete in the background
    /// before time is advanced. These futures will need to make use of the
    /// `SleepProvider::block_advance` (and similar) APIs in order to prevent time advancing while
    /// said tasks complete; see the documentation for those APIs for more detail.
    ///
    /// # Panics
    ///
    /// Panics if another `WaitFor` future is already running. (If two ran simultaneously, they
    /// would both try and advance the same mock time clock, which would be bad.)
    pub fn wait_for<F: futures::Future>(&self, fut: F) -> WaitFor<F> {
        assert!(
            !self.sleep.has_waitfor_waker(),
            "attempted to call MockSleepRuntime::wait_for while another WaitFor is active"
        );
        WaitFor {
            sleep: self.sleep.clone(),
            fut,
        }
    }
}

impl<R: Runtime> Spawn for MockSleepRuntime<R> {
    fn spawn_obj(&self, future: FutureObj<'static, ()>) -> Result<(), SpawnError> {
        self.runtime.spawn_obj(future)
    }
}

impl<R: Runtime> BlockOn for MockSleepRuntime<R> {
    fn block_on<F: Future>(&self, future: F) -> F::Output {
        self.runtime.block_on(future)
    }
}

#[async_trait]
impl<R: Runtime> TcpProvider for MockSleepRuntime<R> {
    type TcpStream = R::TcpStream;
    type TcpListener = R::TcpListener;

    async fn connect(&self, addr: &SocketAddr) -> IoResult<Self::TcpStream> {
        self.runtime.connect(addr).await
    }
    async fn listen(&self, addr: &SocketAddr) -> IoResult<Self::TcpListener> {
        self.runtime.listen(addr).await
    }
}

impl<R: Runtime> TlsProvider<R::TcpStream> for MockSleepRuntime<R> {
    type Connector = R::Connector;
    type TlsStream = R::TlsStream;
    fn tls_connector(&self) -> Self::Connector {
        self.runtime.tls_connector()
    }
}

#[async_trait]
impl<R: Runtime> UdpProvider for MockSleepRuntime<R> {
    type UdpSocket = R::UdpSocket;

    async fn bind(&self, addr: &SocketAddr) -> IoResult<Self::UdpSocket> {
        self.runtime.bind(addr).await
    }
}

impl<R: Runtime> SleepProvider for MockSleepRuntime<R> {
    type SleepFuture = crate::time::Sleeping;
    fn sleep(&self, dur: Duration) -> Self::SleepFuture {
        self.sleep.sleep(dur)
    }
    fn now(&self) -> Instant {
        self.sleep.now()
    }
    fn wallclock(&self) -> SystemTime {
        self.sleep.wallclock()
    }
    fn block_advance<T: Into<String>>(&self, reason: T) {
        self.sleep.block_advance(reason);
    }
    fn release_advance<T: Into<String>>(&self, reason: T) {
        self.sleep.release_advance(reason);
    }
    fn allow_one_advance(&self, dur: Duration) {
        self.sleep.allow_one_advance(dur);
    }
}

/// A future that advances time until another future is ready to complete.
#[pin_project]
pub struct WaitFor<F> {
    /// A reference to the sleep provider that's simulating time for us.
    #[pin]
    sleep: MockSleepProvider,
    /// The future that we're waiting for.
    #[pin]
    fut: F,
}

use std::pin::Pin;
use std::task::{Context, Poll};

impl<F: Future> Future for WaitFor<F> {
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        trace!("waitfor poll");
        let mut this = self.project();
        this.sleep.register_waitfor_waker(cx.waker().clone());

        if let Poll::Ready(r) = this.fut.poll(cx) {
            trace!("waitfor done!");
            this.sleep.clear_waitfor_waker();
            return Poll::Ready(r);
        }
        trace!("waitfor poll complete");

        if this.sleep.should_advance() {
            if let Some(duration) = this.sleep.time_until_next_timeout() {
                trace!("Advancing by {:?}", duration);
                this.sleep.advance_noyield(duration);
            } else {
                // If we get here, something's probably wedged and the test isn't going to complete
                // anyway: we were expecting to advance in order to make progress, but we can't.
                // If we don't panic, the test will just run forever, which is really annoying, so
                // just panic and fail quickly.
                panic!("WaitFor told to advance, but didn't have any duration to advance by");
            }
        } else {
            trace!("waiting for sleepers to advance");
        }
        Poll::Pending
    }
}
