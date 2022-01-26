//! Entry points for use with Tokio runtimes.
use crate::impls::native_tls::NativeTlsProvider;
use crate::impls::tokio::TokioRuntimeHandle as Handle;

use crate::{CompoundRuntime, SpawnBlocking};
use std::io::{Error as IoError, ErrorKind, Result as IoResult};

#[cfg(feature = "rustls")]
use crate::impls::rustls::RustlsProvider;
use crate::impls::tokio::net::TcpStream;

/// A [`Runtime`] built around a Handle to a tokio runtime, and `native_tls`.
///
/// # Limitations
///
/// Note that Arti requires that the runtime should have working
/// implementations for Tokio's time, net, and io facilities, but we have
/// no good way to check that when creating this object.
#[derive(Clone)]
pub struct TokioNativeTlsRuntime {
    /// The actual [`CompoundRuntime`] that implements this.
    inner: HandleInner,
}

/// Implementation type for a TokioRuntimeHandle.
type HandleInner = CompoundRuntime<Handle, Handle, Handle, NativeTlsProvider<TcpStream>>;

/// A [`Runtime`] built around a Handle to a tokio runtime, and `rustls`.
#[derive(Clone)]
#[cfg(feature = "rustls")]
pub struct TokioRustlsRuntime {
    /// The actual [`CompoundRuntime`] that implements this.
    inner: RustlsHandleInner,
}

/// Implementation for a TokioRuntimeRustlsHandle
#[cfg(feature = "rustls")]
type RustlsHandleInner = CompoundRuntime<Handle, Handle, Handle, RustlsProvider<TcpStream>>;

crate::opaque::implement_opaque_runtime! {
    TokioNativeTlsRuntime { inner : HandleInner }
}

#[cfg(feature = "rustls")]
crate::opaque::implement_opaque_runtime! {
    TokioRustlsRuntime { inner : RustlsHandleInner }
}

impl From<tokio_crate::runtime::Handle> for TokioNativeTlsRuntime {
    fn from(h: tokio_crate::runtime::Handle) -> Self {
        let h = Handle::new(h);
        TokioNativeTlsRuntime {
            inner: CompoundRuntime::new(h.clone(), h.clone(), h, NativeTlsProvider::default()),
        }
    }
}

#[cfg(feature = "rustls")]
impl From<tokio_crate::runtime::Handle> for TokioRustlsRuntime {
    fn from(h: tokio_crate::runtime::Handle) -> Self {
        let h = Handle::new(h);
        TokioRustlsRuntime {
            inner: CompoundRuntime::new(h.clone(), h.clone(), h, RustlsProvider::default()),
        }
    }
}

impl TokioNativeTlsRuntime {
    /// Create a new [`TokioNativeTlsRuntime`].
    ///
    /// The return value will own the underlying Tokio runtime object, which
    /// will be dropped when the last copy of this handle is freed.
    ///
    /// If you want to use a currently running runtime instead, call
    /// [`TokioNativeTlsRuntime::current()`].
    pub fn create() -> IoResult<Self> {
        crate::impls::tokio::create_runtime().map(|r| TokioNativeTlsRuntime {
            inner: CompoundRuntime::new(r.clone(), r.clone(), r, NativeTlsProvider::default()),
        })
    }

    /// Return a [`TokioNativeTlsRuntime`] wrapping   the currently running
    /// Tokio runtime.
    ///
    /// # Usage note
    ///
    /// We should never call this from inside other Arti crates, or from library
    /// crates that want to support multiple runtimes!  This function is for
    /// Arti _users_ who want to wrap some existing Tokio runtime as a
    /// [`Runtime`].  It is not for library crates that want to work with
    /// multiple runtimes.
    ///
    /// Once you have a runtime returned by this function, you should just
    /// create more handles to it via [`Clone`].
    pub fn current() -> IoResult<Self> {
        Ok(current_handle()?.into())
    }
}

#[cfg(feature = "rustls")]
impl TokioRustlsRuntime {
    /// Create a new [`TokioRustlsRuntime`].
    ///
    /// The return value will own the underlying Tokio runtime object, which
    /// will be dropped when the last copy of this handle is freed.
    ///
    /// If you want to use a currently running runtime instead, call
    /// [`TokioRustlsRuntime::current()`].
    pub fn create() -> IoResult<Self> {
        crate::impls::tokio::create_runtime().map(|r| TokioRustlsRuntime {
            inner: CompoundRuntime::new(r.clone(), r.clone(), r, RustlsProvider::default()),
        })
    }

    /// Return a [`TokioRustlsRuntime`] wrapping the currently running
    /// Tokio runtime.
    ///
    /// # Usage note
    ///
    /// We should never call this from inside other Arti crates, or from library
    /// crates that want to support multiple runtimes!  This function is for
    /// Arti _users_ who want to wrap some existing Tokio runtime as a
    /// [`Runtime`].  It is not for library crates that want to work with
    /// multiple runtimes.
    ///
    /// Once you have a runtime returned by this function, you should just
    /// create more handles to it via [`Clone`].
    pub fn current() -> IoResult<Self> {
        Ok(current_handle()?.into())
    }
}

/// As `Handle::try_current()`, but return an IoError on failure.
fn current_handle() -> std::io::Result<tokio_crate::runtime::Handle> {
    tokio_crate::runtime::Handle::try_current().map_err(|e| IoError::new(ErrorKind::Other, e))
}

/// Run a test function using a freshly created tokio runtime.
///
/// # Panics
///
/// Panics if we can't create a tokio runtime.
pub fn test_with_runtime<P, F, O>(func: P) -> O
where
    P: FnOnce(TokioNativeTlsRuntime) -> F,
    F: futures::Future<Output = O>,
{
    let runtime = TokioNativeTlsRuntime::create().expect("Failed to create a tokio runtime");
    runtime.clone().block_on(func(runtime))
}
