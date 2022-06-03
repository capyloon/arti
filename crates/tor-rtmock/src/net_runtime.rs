//! Declare MockNetRuntime.

// TODO(nickm): This is mostly copy-paste from MockSleepRuntime.  If possible,
// we should make it so that more code is more shared.

use crate::net::MockNetProvider;
use tor_rtcompat::{BlockOn, Runtime, SleepProvider, TcpProvider, TlsProvider, UdpProvider};

use crate::io::LocalStream;
use async_trait::async_trait;
use futures::task::{FutureObj, Spawn, SpawnError};
use futures::Future;
use std::io::Result as IoResult;
use std::net::SocketAddr;
use std::time::{Duration, Instant, SystemTime};

/// A wrapper Runtime that overrides the SleepProvider trait for the
/// underlying runtime.
#[derive(Clone, Debug)]
pub struct MockNetRuntime<R: Runtime> {
    /// The underlying runtime. Most calls get delegated here.
    runtime: R,
    /// A MockNetProvider.  Network-related calls get delegated here.
    net: MockNetProvider,
}

impl<R: Runtime> MockNetRuntime<R> {
    /// Create a new runtime that wraps `runtime`, but overrides
    /// its view of the network with a [`MockNetProvider`], `net`.
    pub fn new(runtime: R, net: MockNetProvider) -> Self {
        MockNetRuntime { runtime, net }
    }

    /// Return a reference to the underlying runtime.
    pub fn inner(&self) -> &R {
        &self.runtime
    }

    /// Return a reference to the [`MockNetProvider`]
    pub fn mock_net(&self) -> &MockNetProvider {
        &self.net
    }
}

impl<R: Runtime> Spawn for MockNetRuntime<R> {
    fn spawn_obj(&self, future: FutureObj<'static, ()>) -> Result<(), SpawnError> {
        self.runtime.spawn_obj(future)
    }
}

impl<R: Runtime> BlockOn for MockNetRuntime<R> {
    fn block_on<F: Future>(&self, future: F) -> F::Output {
        self.runtime.block_on(future)
    }
}

#[async_trait]
impl<R: Runtime> TcpProvider for MockNetRuntime<R> {
    type TcpStream = <MockNetProvider as TcpProvider>::TcpStream;
    type TcpListener = <MockNetProvider as TcpProvider>::TcpListener;

    async fn connect(&self, addr: &SocketAddr) -> IoResult<Self::TcpStream> {
        self.net.connect(addr).await
    }
    async fn listen(&self, addr: &SocketAddr) -> IoResult<Self::TcpListener> {
        self.net.listen(addr).await
    }
}

impl<R: Runtime> TlsProvider<LocalStream> for MockNetRuntime<R> {
    type Connector = <MockNetProvider as TlsProvider<LocalStream>>::Connector;
    type TlsStream = <MockNetProvider as TlsProvider<LocalStream>>::TlsStream;
    fn tls_connector(&self) -> Self::Connector {
        self.net.tls_connector()
    }
}

#[async_trait]
impl<R: Runtime> UdpProvider for MockNetRuntime<R> {
    type UdpSocket = R::UdpSocket;

    #[inline]
    async fn bind(&self, addr: &SocketAddr) -> IoResult<Self::UdpSocket> {
        // TODO this should probably get delegated to MockNetProvider instead
        self.runtime.bind(addr).await
    }
}

impl<R: Runtime> SleepProvider for MockNetRuntime<R> {
    type SleepFuture = R::SleepFuture;
    fn sleep(&self, dur: Duration) -> Self::SleepFuture {
        self.runtime.sleep(dur)
    }
    fn now(&self) -> Instant {
        self.runtime.now()
    }
    fn wallclock(&self) -> SystemTime {
        self.runtime.wallclock()
    }
}
