//! `tor-persist`: Persistent data storage for use with Tor.
//!
//! This crate is part of
//! [Arti](https://gitlab.torproject.org/tpo/core/arti/), a project to
//! implement [Tor](https://www.torproject.org/) in Rust.
//!
//! For now, users should construct storage objects directly with (for
//! example) [`FsStateMgr::from_path_and_mistrust()`], but use them primarily via the
//! interfaces of the [`StateMgr`] trait.

// @@ begin lint list maintained by maint/add_warning @@
#![deny(missing_docs)]
#![warn(noop_method_call)]
#![deny(unreachable_pub)]
#![warn(clippy::all)]
#![deny(clippy::await_holding_lock)]
#![deny(clippy::cargo_common_metadata)]
#![deny(clippy::cast_lossless)]
#![deny(clippy::checked_conversions)]
#![warn(clippy::cognitive_complexity)]
#![deny(clippy::debug_assert_with_mut_call)]
#![deny(clippy::exhaustive_enums)]
#![deny(clippy::exhaustive_structs)]
#![deny(clippy::expl_impl_clone_on_copy)]
#![deny(clippy::fallible_impl_from)]
#![deny(clippy::implicit_clone)]
#![deny(clippy::large_stack_arrays)]
#![warn(clippy::manual_ok_or)]
#![deny(clippy::missing_docs_in_private_items)]
#![deny(clippy::missing_panics_doc)]
#![warn(clippy::needless_borrow)]
#![warn(clippy::needless_pass_by_value)]
#![warn(clippy::option_option)]
#![warn(clippy::rc_buffer)]
#![deny(clippy::ref_option_ref)]
#![warn(clippy::semicolon_if_nothing_returned)]
#![warn(clippy::trait_duplication_in_bounds)]
#![deny(clippy::unnecessary_wraps)]
#![warn(clippy::unseparated_literal_suffix)]
#![deny(clippy::unwrap_used)]
#![allow(clippy::let_unit_value)] // This can reasonably be done for explicitness
//! <!-- @@ end lint list maintained by maint/add_warning @@ -->

#[cfg(not(target_arch = "wasm32"))]
mod fs;
mod handle;
#[cfg(feature = "testing")]
mod testing;

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::sync::Arc;

/// Wrapper type for Results returned from this crate.
type Result<T> = std::result::Result<T, crate::Error>;

#[cfg(not(target_arch = "wasm32"))]
pub use fs::FsStateMgr;
pub use handle::{DynStorageHandle, StorageHandle};
pub use serde_json::Value as JsonValue;
#[cfg(feature = "testing")]
pub use testing::TestingStateMgr;

use tor_error::ErrorKind;

/// An object that can manage persistent state.
///
/// State is implemented as a simple key-value store, where the values
/// are objects that can be serialized and deserialized.
///
/// # Warnings
///
/// Current implementations may place additional limits on the types
/// of objects that can be stored.  This is not a great example of OO
/// design: eventually we should probably clarify that more.
pub trait StateMgr: Clone {
    /// Try to load the object with key `key` from the store.
    ///
    /// Return None if no such object exists.
    fn load<D>(&self, key: &str) -> Result<Option<D>>
    where
        D: DeserializeOwned;
    /// Try to save `val` with key `key` in the store.
    ///
    /// Replaces any previous value associated with `key`.
    fn store<S>(&self, key: &str, val: &S) -> Result<()>
    where
        S: Serialize;
    /// Return true if this is a read-write state manager.
    ///
    /// If it returns false, then attempts to `store` will fail with
    /// [`Error::NoLock`]
    fn can_store(&self) -> bool;

    /// Try to become a read-write state manager if possible, without
    /// blocking.
    ///
    /// This function will return an error only if something really
    /// unexpected went wrong.  It may return `Ok(_)` even if we don't
    /// acquire the lock: check the return value or call
    /// `[StateMgr::can_store()`] to see if the lock is held.
    fn try_lock(&self) -> Result<LockStatus>;

    /// Release any locks held and become a read-only state manager
    /// again. If no locks were held, do nothing.
    fn unlock(&self) -> Result<()>;

    /// Make a new [`StorageHandle`] to store values of particular type
    /// at a particular key.
    fn create_handle<T>(self, key: impl Into<String>) -> DynStorageHandle<T>
    where
        Self: Send + Sync + Sized + 'static,
        T: Serialize + DeserializeOwned + 'static,
    {
        Arc::new(handle::StorageHandleImpl::new(self, key.into()))
    }
}

/// A possible outcome from calling [`StateMgr::try_lock()`]
#[allow(clippy::exhaustive_enums)]
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum LockStatus {
    /// We didn't have the lock and were unable to acquire it.
    NoLock,
    /// We already held the lock, and didn't have anything to do.
    AlreadyHeld,
    /// We successfully acquired the lock for the first time.
    NewlyAcquired,
}

impl LockStatus {
    /// Return true if this status indicates that we hold the lock.
    pub fn held(&self) -> bool {
        !matches!(self, LockStatus::NoLock)
    }
}

/// An error manipulating persistent state.
//
// Such errors are "global" in the sense that it doesn't relate to any guard or any circuit
// or anything, so callers may use `#[from]` when they include it in their own error.
#[derive(thiserror::Error, Debug, Clone)]
#[non_exhaustive]
pub enum Error {
    /// An IO error occurred.
    #[error("IO error")]
    IoError(#[source] Arc<std::io::Error>),

    /// Permissions on a file or path were incorrect
    #[error("Invalid permissions on state file")]
    Permissions(#[from] fs_mistrust::Error),

    /// Tried to save without holding an exclusive lock.
    //
    // TODO This error seems to actually be sometimes used to make store a no-op.
    //      We should consider whether this is best handled as an error, but for now
    //      this seems adequate.
    #[error("Storage not locked")]
    NoLock,

    /// Problem when serializing JSON data.
    #[error("JSON serialization error")]
    Serialize(#[source] Arc<serde_json::Error>),

    /// Problem when deserializing JSON data.
    #[error("JSON serialization error")]
    Deserialize(#[source] Arc<serde_json::Error>),
}

impl tor_error::HasKind for Error {
    #[rustfmt::skip] // the tabular layout of the `match` makes this a lot clearer
    fn kind(&self) -> ErrorKind {
        use Error as E;
        use tor_error::ErrorKind as K;
        match self {
            E::IoError(..)     => K::PersistentStateAccessFailed,
            E::Permissions(e)  => if e.is_bad_permission() {
                K::FsPermissions
            } else {
                K::PersistentStateAccessFailed
            }
            E::NoLock          => K::BadApiUsage,
            E::Serialize(..)   => K::Internal,
            E::Deserialize(..) => K::PersistentStateCorrupted,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Error {
        Error::IoError(Arc::new(e))
    }
}

/// Error conversion for JSON errors; use only when loading
fn load_error(e: serde_json::Error) -> Error {
    Error::Deserialize(Arc::new(e))
}

/// Error conversion for JSON errors; use only when storing
fn store_error(e: serde_json::Error) -> Error {
    Error::Serialize(Arc::new(e))
}

/// A wrapper type for types whose representation may change in future versions of Arti.
///
/// This uses `#[serde(untagged)]` to attempt deserializing as a type `T` first, and falls back
/// to a generic JSON value representation if that fails.
#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Eq)]
#[serde(untagged)]
#[allow(clippy::exhaustive_enums)]
pub enum Futureproof<T> {
    /// A successfully-deserialized `T`.
    Understandable(T),
    /// A generic JSON value, representing a failure to deserialize a `T`.
    Unknown(JsonValue),
}

impl<T> Futureproof<T> {
    /// Convert the `Futureproof` into an `Option<T>`, throwing away an `Unknown` value.
    pub fn into_option(self) -> Option<T> {
        match self {
            Futureproof::Understandable(x) => Some(x),
            Futureproof::Unknown(_) => None,
        }
    }
}

impl<T> From<T> for Futureproof<T> {
    fn from(inner: T) -> Self {
        Self::Understandable(inner)
    }
}
