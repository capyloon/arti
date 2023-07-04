//! Hidden service (onion service) client key management functionality

// TODO HS what layer should be responsible for finding and dispatching keys?
// I think it should be as high as possible, so keys should be passed into
// the hs connector for each connection.  Otherwise there would have to be an
// HsKeyProvider trait here, and error handling gets complicated.

use std::fmt::{self, Debug};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use tor_hscrypto::pk::{HsClientDescEncSecretKey, HsClientIntroAuthKeypair, HsId};
use tor_keymgr::{ArtiPath, ArtiPathComponent, CTorPath, KeySpecifier};

/// Keys (if any) to use when connecting to a specific onion service.
///
/// Represents a possibly empty subset of the following keys:
///  * `KS_hsc_desc_enc`, [`HsClientDescEncSecretKey`]
///  * `KS_hsc_intro_auth`, [`HsClientIntroAuthKeypair`]
///
/// `HsClientSecretKeys` is constructed with a `Builder`:
/// use `ClientSecretKeysBuilder::default()`,
/// optionally call setters, and then call `build()`.
///
/// For client connections to share circuits and streams,
/// call `build` only once.
/// Different calls to `build` yield `HsClientSecretKeys` values
/// which won't share HS circuits, streams, or authentication.
///
/// Conversely, `Clone`s of an `HsClientSecretKeys` *can* share circuits.
//
/// All [empty](HsClientSecretKeys::is_empty) `HsClientSecretKeys`
/// (for example, from [`:none()`](HsClientSecretKeys::none))
/// *can* share circuits.
//
// TODO HS some way to read these from files or something!
//
// TODO HS: some of our APIs take Option<HsClientSecretKeys>.
// But HsClientSecretKeys is can be empty, so we should remove the `Option`.
#[derive(Clone, Default)]
pub struct HsClientSecretKeys {
    /// The actual keys
    ///
    /// This is compared and hashed by the Arc pointer value.
    /// We don't want to implement key comparison by comparing secret key values.
    pub(crate) keys: Arc<ClientSecretKeyValues>,
}

impl Debug for HsClientSecretKeys {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        // TODO derive this?
        let mut d = f.debug_tuple("HsClientSecretKeys");
        d.field(&Arc::as_ptr(&self.keys));
        self.keys
            .ks_hsc_desc_enc
            .as_ref()
            .map(|_| d.field(&"<desc_enc>"));
        self.keys
            .ks_hsc_intro_auth
            .as_ref()
            .map(|_| d.field(&"<intro_uath>"));
        d.finish()
    }
}

impl PartialEq for HsClientSecretKeys {
    fn eq(&self, other: &Self) -> bool {
        self.is_empty() && other.is_empty() || Arc::ptr_eq(&self.keys, &other.keys)
    }
}
impl Eq for HsClientSecretKeys {}
impl Hash for HsClientSecretKeys {
    fn hash<H: Hasher>(&self, state: &mut H) {
        Arc::as_ptr(&self.keys).hash(state);
    }
}

impl HsClientSecretKeys {
    /// Create a new `HsClientSecretKeys`, for making unauthenticated connections
    ///
    /// Creates a `HsClientSecretKeys` which has no actual keys,
    /// so will make connections to hidden services
    /// without any Tor-protocol-level client authentication.
    pub fn none() -> Self {
        Self::default()
    }

    /// Tests whether this `HsClientSecretKeys` actually contains any keys
    pub fn is_empty(&self) -> bool {
        // TODO derive this.  For now, we deconstruct it to prove we check all the fields.
        let ClientSecretKeyValues {
            ks_hsc_desc_enc,
            ks_hsc_intro_auth,
        } = &*self.keys;
        ks_hsc_desc_enc.is_none() && ks_hsc_intro_auth.is_none()
    }
}

/// Client secret key values
///
/// Skip the whole builder pattern derivation, etc. - the types are just the same
type ClientSecretKeyValues = HsClientSecretKeysBuilder;

/// Builder for `HsClientSecretKeys`
#[derive(Default, Debug)]
pub struct HsClientSecretKeysBuilder {
    /// Possibly, a key that is used to decrypt a descriptor.
    pub(crate) ks_hsc_desc_enc: Option<HsClientDescEncSecretKey>,

    /// Possibly, a key that is used to authenticate while introducing.
    pub(crate) ks_hsc_intro_auth: Option<HsClientIntroAuthKeypair>,
}

// TODO derive these setters
//
// TODO HS is this what we want for an API?  We need *some* API.
// This is a bit like config but we probably don't want to
// feed secret key material through config-rs, etc.
impl HsClientSecretKeysBuilder {
    /// Provide a descriptor decryption key
    pub fn ks_hsc_desc_enc(&mut self, ks: HsClientDescEncSecretKey) -> &mut Self {
        self.ks_hsc_desc_enc = Some(ks);
        self
    }
    /// Provide an introduction authentication key
    pub fn ks_hsc_intro_auth(&mut self, ks: HsClientIntroAuthKeypair) -> &mut Self {
        self.ks_hsc_intro_auth = Some(ks);
        self
    }

    /// Convert these
    pub fn build(self) -> Result<HsClientSecretKeys, tor_config::ConfigBuildError> {
        Ok(HsClientSecretKeys {
            keys: Arc::new(self),
        })
    }
}

/// An HS client identifier.
///
/// Distinguishes different "clients" or "users" of this Arti instance,
/// so that they can have different sets of HS client authentication keys.
#[derive(Clone, Debug, derive_more::Display, derive_more::Into, derive_more::AsRef)]
pub struct HsClientSpecifier(ArtiPathComponent);

impl HsClientSpecifier {
    /// Create a new [`HsClientSpecifier`].
    ///
    /// The `inner` string **must** be a valid [`ArtiPathComponent`].
    pub fn new(inner: String) -> Result<Self, tor_keymgr::Error> {
        ArtiPathComponent::new(inner).map(Self)
    }
}

/// An identifier for a particular instance of an HS client key.
pub struct HsClientSecretKeySpecifier {
    /// The client associated with this key.
    client_id: HsClientSpecifier,
    /// The hidden service this authorization key is for.
    hs_id: HsId,
    /// The role of the key.
    role: HsClientKeyRole,
}

/// The role of an HS client key.
#[derive(Debug, Clone, Copy, PartialEq, derive_more::Display)]
#[non_exhaustive]
pub enum HsClientKeyRole {
    /// A key for deriving keys for decrypting HS descriptors (KP_hsc_desc_enc).
    #[display(fmt = "KP_hsc_desc_enc")]
    DescEnc,
    /// A key for computing INTRODUCE1 signatures (KP_hsc_intro_auth).
    #[display(fmt = "KP_hsc_intro_auth")]
    IntroAuth,
}

impl HsClientSecretKeySpecifier {
    /// Create a new [`HsClientSecretKeySpecifier`].
    pub fn new(client_id: HsClientSpecifier, hs_id: HsId, role: HsClientKeyRole) -> Self {
        Self {
            client_id,
            hs_id,
            role,
        }
    }
}

impl KeySpecifier for HsClientSecretKeySpecifier {
    fn arti_path(&self) -> tor_keymgr::Result<ArtiPath> {
        ArtiPath::new(format!(
            "client/{}/{}/{}",
            self.client_id, self.hs_id, self.role
        ))
    }

    fn ctor_path(&self) -> Option<CTorPath> {
        todo!()
    }
}
