//! Define a [`CompoundRuntime`] part that can be built from several component
//! pieces.

use std::{net::SocketAddr, sync::Arc, time::Duration};

use crate::traits::*;
use async_trait::async_trait;
use educe::Educe;
use futures::{future::FutureObj, task::Spawn};
use std::io::Result as IoResult;
use std::time::{Instant, SystemTime};

/// A runtime made of several parts, each of which implements one trait-group.
///
/// The `SpawnR` component should implements [`Spawn`] and [`BlockOn`];
/// the `SleepR` component should implement [`SleepProvider`]; the `TcpR`
/// component should implement [`TcpProvider`]; and the `TlsR` component should
/// implement [`TlsProvider`].
///
/// You can use this structure to create new runtimes in two ways: either by
/// overriding a single part of an existing runtime, or by building an entirely
/// new runtime from pieces.
#[derive(Educe)]
#[educe(Clone)] // #[derive(Clone)] wrongly infers Clone bounds on the generic parameters
pub struct CompoundRuntime<SpawnR, SleepR, TcpR, TlsR, UdpR> {
    /// The actual collection of Runtime objects.
    ///
    /// We wrap this in an Arc rather than requiring that each item implement
    /// Clone, though we could change our minds later on.
    inner: Arc<Inner<SpawnR, SleepR, TcpR, TlsR, UdpR>>,
}

/// A collection of objects implementing that traits that make up a [`Runtime`]
struct Inner<SpawnR, SleepR, TcpR, TlsR, UdpR> {
    /// A `Spawn` and `BlockOn` implementation.
    spawn: SpawnR,
    /// A `SleepProvider` implementation.
    sleep: SleepR,
    /// A `TcpProvider` implementation
    tcp: TcpR,
    /// A `TcpProvider<TcpR::TcpStream>` implementation.
    tls: TlsR,
    /// A `UdpProvider` implementation
    udp: UdpR,
}

impl<SpawnR, SleepR, TcpR, TlsR, UdpR> CompoundRuntime<SpawnR, SleepR, TcpR, TlsR, UdpR> {
    /// Construct a new CompoundRuntime from its components.
    pub fn new(spawn: SpawnR, sleep: SleepR, tcp: TcpR, tls: TlsR, udp: UdpR) -> Self {
        CompoundRuntime {
            inner: Arc::new(Inner {
                spawn,
                sleep,
                tcp,
                tls,
                udp,
            }),
        }
    }
}

impl<SpawnR, SleepR, TcpR, TlsR, UdpR> Spawn for CompoundRuntime<SpawnR, SleepR, TcpR, TlsR, UdpR>
where
    SpawnR: Spawn,
{
    #[inline]
    fn spawn_obj(&self, future: FutureObj<'static, ()>) -> Result<(), futures::task::SpawnError> {
        self.inner.spawn.spawn_obj(future)
    }
}

impl<SpawnR, SleepR, TcpR, TlsR, UdpR> BlockOn for CompoundRuntime<SpawnR, SleepR, TcpR, TlsR, UdpR>
where
    SpawnR: BlockOn,
    SleepR: Clone + Send + Sync + 'static,
    TcpR: Clone + Send + Sync + 'static,
    TlsR: Clone + Send + Sync + 'static,
    UdpR: Clone + Send + Sync + 'static,
{
    #[inline]
    fn block_on<F: futures::Future>(&self, future: F) -> F::Output {
        self.inner.spawn.block_on(future)
    }
}

impl<SpawnR, SleepR, TcpR, TlsR, UdpR> SleepProvider
    for CompoundRuntime<SpawnR, SleepR, TcpR, TlsR, UdpR>
where
    SleepR: SleepProvider,
    SpawnR: Clone + Send + Sync + 'static,
    TcpR: Clone + Send + Sync + 'static,
    TlsR: Clone + Send + Sync + 'static,
    UdpR: Clone + Send + Sync + 'static,
{
    type SleepFuture = SleepR::SleepFuture;

    #[inline]
    fn sleep(&self, duration: Duration) -> Self::SleepFuture {
        self.inner.sleep.sleep(duration)
    }

    #[inline]
    fn now(&self) -> Instant {
        self.inner.sleep.now()
    }

    #[inline]
    fn wallclock(&self) -> SystemTime {
        self.inner.sleep.wallclock()
    }
}

#[async_trait]
impl<SpawnR, SleepR, TcpR, TlsR, UdpR> TcpProvider
    for CompoundRuntime<SpawnR, SleepR, TcpR, TlsR, UdpR>
where
    TcpR: TcpProvider,
    SpawnR: Send + Sync + 'static,
    SleepR: Send + Sync + 'static,
    TcpR: Send + Sync + 'static,
    TlsR: Send + Sync + 'static,
    UdpR: Send + Sync + 'static,
{
    type TcpStream = TcpR::TcpStream;

    type TcpListener = TcpR::TcpListener;

    #[inline]
    async fn connect(&self, addr: &SocketAddr) -> IoResult<Self::TcpStream> {
        self.inner.tcp.connect(addr).await
    }

    #[inline]
    async fn listen(&self, addr: &SocketAddr) -> IoResult<Self::TcpListener> {
        self.inner.tcp.listen(addr).await
    }
}

impl<SpawnR, SleepR, TcpR, TlsR, UdpR, S> TlsProvider<S>
    for CompoundRuntime<SpawnR, SleepR, TcpR, TlsR, UdpR>
where
    TcpR: TcpProvider,
    TlsR: TlsProvider<S>,
    SleepR: Clone + Send + Sync + 'static,
    SpawnR: Clone + Send + Sync + 'static,
    UdpR: Clone + Send + Sync + 'static,
{
    type Connector = TlsR::Connector;
    type TlsStream = TlsR::TlsStream;

    #[inline]
    fn tls_connector(&self) -> Self::Connector {
        self.inner.tls.tls_connector()
    }
}

impl<SpawnR, SleepR, TcpR, TlsR, UdpR> std::fmt::Debug
    for CompoundRuntime<SpawnR, SleepR, TcpR, TlsR, UdpR>
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompoundRuntime").finish_non_exhaustive()
    }
}

#[async_trait]
impl<SpawnR, SleepR, TcpR, TlsR, UdpR> UdpProvider
    for CompoundRuntime<SpawnR, SleepR, TcpR, TlsR, UdpR>
where
    UdpR: UdpProvider,
    SpawnR: Send + Sync + 'static,
    SleepR: Send + Sync + 'static,
    TcpR: Send + Sync + 'static,
    TlsR: Send + Sync + 'static,
    UdpR: Send + Sync + 'static,
{
    type UdpSocket = UdpR::UdpSocket;

    #[inline]
    async fn bind(&self, addr: &SocketAddr) -> IoResult<Self::UdpSocket> {
        self.inner.udp.bind(addr).await
    }
}
