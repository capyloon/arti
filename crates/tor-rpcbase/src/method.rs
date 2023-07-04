//! Method type for the RPC system.

use std::collections::HashSet;

use downcast_rs::Downcast;
use once_cell::sync::Lazy;

/// The parameters and method name associated with a given Request.
///
/// We use [`typetag`] here so that we define `Method`s in other crates.
///
/// See [`decl_method!`](crate::decl_method) for a template to declare one of these.
///
/// # Note
///
/// In order to comply with our spec, all Methods' data must be represented as a json
/// object.
//
// TODO RPC: Possible issue here is that, if this trait is public, anybody outside
// of Arti can use this trait to add new methods to the RPC engine. Should we
// care?
#[typetag::deserialize(tag = "method", content = "params")]
pub trait DynMethod: std::fmt::Debug + Send + Downcast {}
downcast_rs::impl_downcast!(DynMethod);

/// A typed method, used to ensure that all implementations of a method have the
/// same success and updates types.
///
/// Prefer to implement this trait, rather than `DynMethod`. (`DynMethod`
/// represents a type-erased method, with statically-unknown `Output` and
/// `Update` types.)
pub trait Method: DynMethod {
    /// A type returned by this method on success.
    type Output: serde::Serialize + Send + 'static;
    /// A type sent by this method on updates.
    ///
    /// If this method will never send updates, use the uninhabited
    /// [`NoUpdates`] type.
    type Update: serde::Serialize + Send + 'static;
}

/// An uninhabited type, used to indicate that a given method will never send
/// updates.
#[derive(serde::Serialize)]
#[allow(clippy::exhaustive_enums)]
pub enum NoUpdates {}

/// A method we're registering.
///
/// This struct's methods are public so it can be constructed from
/// `decl_method!`.
///
/// If you construct it yourself, you'll be in trouble.  But you already knew
/// that, since you're looking at a `doc(hidden)` thing.
#[doc(hidden)]
#[allow(clippy::exhaustive_structs)]
pub struct MethodInfo_ {
    /// The name of the method.
    pub method_name: &'static str,
}

inventory::collect!(MethodInfo_);

/// Declare that one or more space-separated types should be considered as RPC
/// methods.
///
/// # Example
///
/// ```
/// use tor_rpcbase as rpc;
///
/// #[derive(Debug, serde::Deserialize)]
/// struct Castigate {
///    severity: f64,
///    offenses: Vec<String>,
///    accomplice: Option<rpc::ObjectId>,
/// }
/// rpc::decl_method!{ "x-example:castigate" => Castigate}
///
/// impl rpc::Method for Castigate {
///     type Output = String;
///     type Update = rpc::NoUpdates;
/// }
/// ```
///
/// # Limitations
///
/// For now you'll need to import the `typetag` crate; unfortunately, it doesn't
/// yet behave well when used where it is not in scope as `typetag`.
#[macro_export]
macro_rules! decl_method {
    {$($name:expr => $id:ident),* $(,)?}
    =>
    {
        $(
            $crate::impl_const_type_id!{$id}
            #[typetag::deserialize(name = $name)]
            impl $crate::DynMethod for $id {}
            $crate::inventory::submit!{
                $crate::MethodInfo_ { method_name : $name }
            }
        )*
    }
}

/// Return true if `name` is the name of some method.
pub fn is_method_name(name: &str) -> bool {
    /// Lazy set of all method names.
    static METHOD_NAMES: Lazy<HashSet<&'static str>> = Lazy::new(|| iter_method_names().collect());
    METHOD_NAMES.contains(name)
}

/// Return an iterator that yields every registered method name.
///
/// Used (e.g.) to enforce syntactic requirements on method names.
pub fn iter_method_names() -> impl Iterator<Item = &'static str> {
    inventory::iter::<MethodInfo_>().map(|mi| mi.method_name)
}
