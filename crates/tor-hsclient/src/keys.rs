//! Hidden service (onion service) client key management functionality

// TODO HS what layer should be responsible for finding and dispatching keys?
// I think it should be as high as possible, so keys should be passed into
// the hs connector for each connection.  Otherwise there would have to be an
// HsKeyProvider trait here, and error handling gets complicated.

use std::fmt::{self, Debug};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use tor_hscrypto::pk::{HsClientDescEncSecretKey, HsClientIntroAuthSecretKey};

/// Keys (if any) to use when connecting to a specific onion service.
///
/// Represents a possibly empty subset of the following keys:
///  * `KS_hsc_desc_enc`, [`HsClientDescEncSecretKey`]
///  * `KS_hsc_intro_auth`, [`HsClientIntroAuthSecretKey`]
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
    keys: Arc<ClientSecretKeyValues>,
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
    ks_hsc_desc_enc: Option<HsClientDescEncSecretKey>,

    /// Possibly, a key that is used to authenticate while introducing.
    ks_hsc_intro_auth: Option<HsClientIntroAuthSecretKey>,
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
    pub fn ks_hsc_intro_auth(&mut self, ks: HsClientIntroAuthSecretKey) -> &mut Self {
        self.ks_hsc_intro_auth = Some(ks);
        self
    }

    /// Convert these
    pub fn build(self) -> Result<HsClientSecretKeys, tor_config::ConfigError> {
        Ok(HsClientSecretKeys {
            keys: Arc::new(self),
        })
    }
}
