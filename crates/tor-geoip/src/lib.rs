//! A crate for performing GeoIP lookups using the Tor GeoIP database.

// @@ begin lint list maintained by maint/add_warning @@
#![cfg_attr(not(ci_arti_stable), allow(renamed_and_removed_lints))]
#![cfg_attr(not(ci_arti_nightly), allow(unknown_lints))]
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
#![deny(clippy::print_stderr)]
#![deny(clippy::print_stdout)]
#![warn(clippy::rc_buffer)]
#![deny(clippy::ref_option_ref)]
#![warn(clippy::semicolon_if_nothing_returned)]
#![warn(clippy::trait_duplication_in_bounds)]
#![deny(clippy::unnecessary_wraps)]
#![warn(clippy::unseparated_literal_suffix)]
#![deny(clippy::unwrap_used)]
#![allow(clippy::let_unit_value)] // This can reasonably be done for explicitness
#![allow(clippy::uninlined_format_args)]
#![allow(clippy::significant_drop_in_scrutinee)] // arti/-/merge_requests/588/#note_2812945
#![allow(clippy::result_large_err)] // temporary workaround for arti#587
//! <!-- @@ end lint list maintained by maint/add_warning @@ -->

pub use crate::err::Error;
use once_cell::sync::OnceCell;
use rangemap::RangeInclusiveMap;
use std::fmt::{Debug, Display, Formatter};
use std::net::{IpAddr, Ipv6Addr};
use std::num::NonZeroU32;
use std::str::FromStr;
use std::sync::Arc;

mod err;

/// An embedded copy of the latest geoip v4 database at the time of compilation.
///
/// FIXME(eta): This does use a few megabytes of binary size, which is less than ideal.
///             It would be better to parse it at compile time or something.
#[cfg(feature = "embedded-db")]
static EMBEDDED_DB_V4: &str = include_str!("../data/geoip");

/// An embedded copy of the latest geoip v6 database at the time of compilation.
#[cfg(feature = "embedded-db")]
static EMBEDDED_DB_V6: &str = include_str!("../data/geoip6");

/// A parsed copy of the embedded database.
#[cfg(feature = "embedded-db")]
static EMBEDDED_DB_PARSED: OnceCell<Arc<GeoipDb>> = OnceCell::new();

/// A two-letter country code.
///
/// Specifically, this type represents a purported "ISO 3166-1 alpha-2" country
/// code, such as "IT" for Italy or "UY" for Uruguay.
///
/// It does not include the sentinel value `??` that we use to represent
/// "country unknown"; if you need that, use [`OptionCc`]. Other than that, we
/// do not check whether the country code represents a real country: we only
/// ensure that it is a pair of printing ASCII characters.
///
/// Note that the geoip databases included with Arti will only include real
/// countries; we do not include the pseudo-countries `A1` through `An` for
/// "anonymous proxies", since doing so would mean putting nearly all Tor relays
/// into one of those countries.
#[derive(Copy, Clone, Eq, PartialEq)]
pub struct CountryCode {
    /// The underlying value (two printable ASCII characters, stored uppercase).
    ///
    /// The special value `??` is excluded, since it is not a country; use
    /// `OptionCc` instead if you need to represent that.
    inner: [u8; 2],
}

impl CountryCode {
    /// Make a new `CountryCode`.
    fn new(cc_orig: &str) -> Result<Self, Error> {
        let cc = cc_orig.to_ascii_uppercase();

        let cc: [u8; 2] = cc
            .as_bytes()
            .try_into()
            .map_err(|_| Error::BadCountryCode(cc))?;

        if !cc.iter().all(|b| b.is_ascii() && !b.is_ascii_control()) {
            return Err(Error::BadCountryCode(cc_orig.to_owned()));
        }

        if &cc == b"??" {
            return Err(Error::NowhereNotSupported);
        }

        Ok(Self { inner: cc })
    }

    /// Get the actual country code.
    ///
    /// This just calls `.as_ref()`.
    pub fn get(&self) -> &str {
        self.as_ref()
    }
}

impl Display for CountryCode {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_ref())
    }
}

impl Debug for CountryCode {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "CountryCode(\"{}\")", self.as_ref())
    }
}

impl AsRef<str> for CountryCode {
    fn as_ref(&self) -> &str {
        // This shouldn't ever panic, since we shouldn't feed bad country codes in.
        std::str::from_utf8(&self.inner).expect("invalid country code in CountryCode")
    }
}

impl FromStr for CountryCode {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        CountryCode::new(s)
    }
}

/// Wrapper for an `Option<`[`CountryCode`]`>` that encodes `None` as `??`.
///
/// Used so that we can implement foreign traits.
#[derive(
    Copy, Clone, Debug, Eq, PartialEq, derive_more::Into, derive_more::From, derive_more::AsRef,
)]
#[allow(clippy::exhaustive_structs)]
pub struct OptionCc(pub Option<CountryCode>);

impl FromStr for OptionCc {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match CountryCode::new(s) {
            Err(Error::NowhereNotSupported) => Ok(None.into()),
            Err(e) => Err(e),
            Ok(cc) => Ok(Some(cc).into()),
        }
    }
}

impl Display for OptionCc {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            Some(cc) => write!(f, "{}", cc),
            None => write!(f, "??"),
        }
    }
}

/// A country code / ASN definition.
///
/// Type lifted from `geoip-db-tool` in the C-tor source.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
struct NetDefn {
    /// The country code.
    ///
    /// We translate the value "??" into None.
    cc: Option<CountryCode>,
    /// The ASN, if we have one.
    asn: Option<NonZeroU32>,
}

impl NetDefn {
    /// Make a new `NetDefn`.
    fn new(cc: &str, asn: Option<u32>) -> Result<Self, Error> {
        let asn = asn
            .map(|x| x.try_into())
            .transpose()
            .map_err(|_| Error::BadFormat("got an ASN with value 0"))?;

        let cc = cc.parse::<OptionCc>()?.into();

        Ok(Self { cc, asn })
    }

    /// Return the country code.
    fn country_code(&self) -> Option<&CountryCode> {
        self.cc.as_ref()
    }

    /// Return the ASN, if there is one.
    fn asn(&self) -> Option<u32> {
        self.asn.as_ref().map(|x| x.get())
    }
}

/// A database of IP addresses to country codes.
pub struct GeoipDb {
    /// The IPv4 subset of the database, with v4 addresses stored as 32-bit integers.
    map_v4: RangeInclusiveMap<u32, NetDefn>,
    /// The IPv6 subset of the database, with v6 addresses stored as 128-bit integers.
    map_v6: RangeInclusiveMap<u128, NetDefn>,
}

impl GeoipDb {
    /// Make a new `GeoipDb` using a compiled-in copy of the GeoIP database.
    ///
    /// The returned instance of the database is shared with `Arc` across all invocations of this
    /// function in the same program.
    #[cfg(feature = "embedded-db")]
    pub fn new_embedded() -> Arc<Self> {
        Arc::clone(EMBEDDED_DB_PARSED.get_or_init(|| {
            Arc::new(
                // It's reasonable to assume the one we embedded is fine -- we'll test it in CI, etc.
                Self::new_from_legacy_format(EMBEDDED_DB_V4, EMBEDDED_DB_V6)
                    .expect("failed to parse embedded geoip database"),
            )
        }))
    }

    /// Make a new `GeoipDb` using provided copies of the v4 and v6 database, in Tor legacy format.
    pub fn new_from_legacy_format(db_v4: &str, db_v6: &str) -> Result<Self, Error> {
        let mut ret = GeoipDb {
            map_v4: Default::default(),
            map_v6: Default::default(),
        };

        for line in db_v4.lines() {
            if line.starts_with('#') {
                continue;
            }
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let mut split = line.split(',');
            let from = split
                .next()
                .ok_or(Error::BadFormat("empty line somehow?"))?
                .parse::<u32>()?;
            let to = split
                .next()
                .ok_or(Error::BadFormat("line with insufficient commas"))?
                .parse::<u32>()?;
            let cc = split
                .next()
                .ok_or(Error::BadFormat("line with insufficient commas"))?;
            let asn = split.next().map(|x| x.parse::<u32>()).transpose()?;

            let defn = NetDefn::new(cc, asn)?;

            ret.map_v4.insert(from..=to, defn);
        }

        // This is slightly copypasta, but probably less readable to merge into one thing.
        for line in db_v6.lines() {
            if line.starts_with('#') {
                continue;
            }
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let mut split = line.split(',');
            let from = split
                .next()
                .ok_or(Error::BadFormat("empty line somehow?"))?
                .parse::<Ipv6Addr>()?;
            let to = split
                .next()
                .ok_or(Error::BadFormat("line with insufficient commas"))?
                .parse::<Ipv6Addr>()?;
            let cc = split
                .next()
                .ok_or(Error::BadFormat("line with insufficient commas"))?;
            let asn = split.next().map(|x| x.parse::<u32>()).transpose()?;

            let defn = NetDefn::new(cc, asn)?;

            ret.map_v6.insert(from.into()..=to.into(), defn);
        }

        Ok(ret)
    }

    /// Get the `NetDefn` for an IP address.
    fn lookup_defn(&self, ip: IpAddr) -> Option<&NetDefn> {
        match ip {
            IpAddr::V4(v4) => self.map_v4.get(&v4.into()),
            IpAddr::V6(v6) => self.map_v6.get(&v6.into()),
        }
    }

    /// Get a 2-letter country code for the given IP address, if this data is available.
    pub fn lookup_country_code(&self, ip: IpAddr) -> Option<&CountryCode> {
        self.lookup_defn(ip).and_then(|x| x.country_code())
    }

    /// Return the ASN the IP address is in, if this data is available.
    pub fn lookup_asn(&self, ip: IpAddr) -> Option<u32> {
        self.lookup_defn(ip)?.asn()
    }
}

#[cfg(test)]
mod test {
    // @@ begin test lint list maintained by maint/add_warning @@
    #![allow(clippy::bool_assert_comparison)]
    #![allow(clippy::clone_on_copy)]
    #![allow(clippy::dbg_macro)]
    #![allow(clippy::print_stderr)]
    #![allow(clippy::print_stdout)]
    #![allow(clippy::single_char_pattern)]
    #![allow(clippy::unwrap_used)]
    #![allow(clippy::unchecked_duration_subtraction)]
    //! <!-- @@ end test lint list maintained by maint/add_warning @@ -->

    use super::*;
    use std::net::Ipv4Addr;

    // NOTE(eta): this test takes a whole 1.6 seconds in *non-release* mode
    #[test]
    #[cfg(feature = "embedded-db")]
    fn embedded_db() {
        let db = GeoipDb::new_embedded();

        assert_eq!(
            db.lookup_country_code(Ipv4Addr::new(8, 8, 8, 8).into())
                .map(|x| x.as_ref()),
            Some("US")
        );

        assert_eq!(
            db.lookup_country_code("2001:4860:4860::8888".parse().unwrap())
                .map(|x| x.as_ref()),
            Some("US")
        );
    }

    #[test]
    fn basic_lookups() {
        let src_v4 = r#"
        16909056,16909311,GB
        "#;
        let src_v6 = r#"
        fe80::,fe81::,US
        dead:beef::,dead:ffff::,??
        "#;
        let db = GeoipDb::new_from_legacy_format(src_v4, src_v6).unwrap();

        assert_eq!(
            db.lookup_country_code(Ipv4Addr::new(1, 2, 3, 4).into())
                .map(|x| x.as_ref()),
            Some("GB")
        );

        assert_eq!(
            db.lookup_country_code(Ipv4Addr::new(1, 1, 1, 1).into()),
            None
        );

        assert_eq!(
            db.lookup_country_code("fe80::dead:beef".parse().unwrap())
                .map(|x| x.as_ref()),
            Some("US")
        );

        assert_eq!(
            db.lookup_country_code("fe81::dead:beef".parse().unwrap()),
            None
        );
        assert_eq!(
            db.lookup_country_code("dead:beef::1".parse().unwrap()),
            None
        );
    }

    #[test]
    fn cc_parse() -> Result<(), Error> {
        // real countries.
        assert_eq!(CountryCode::from_str("us")?, CountryCode::from_str("US")?);
        assert_eq!(CountryCode::from_str("UY")?, CountryCode::from_str("UY")?);

        // not real as of this writing, but still representable.
        assert_eq!(CountryCode::from_str("A7")?, CountryCode::from_str("a7")?);
        assert_eq!(CountryCode::from_str("xz")?, CountryCode::from_str("xz")?);

        // Can't convert to two bytes.
        assert!(matches!(
            CountryCode::from_str("z"),
            Err(Error::BadCountryCode(_))
        ));
        assert!(matches!(
            CountryCode::from_str("🐻‍❄️"),
            Err(Error::BadCountryCode(_))
        ));
        assert!(matches!(
            CountryCode::from_str("Sheboygan"),
            Err(Error::BadCountryCode(_))
        ));

        // Can convert to two bytes, but still not printable ascii
        assert!(matches!(
            CountryCode::from_str("\r\n"),
            Err(Error::BadCountryCode(_))
        ));
        assert!(matches!(
            CountryCode::from_str("\0\0"),
            Err(Error::BadCountryCode(_))
        ));
        assert!(matches!(
            CountryCode::from_str("¡"),
            Err(Error::BadCountryCode(_))
        ));

        // Not a country.
        assert!(matches!(
            CountryCode::from_str("??"),
            Err(Error::NowhereNotSupported)
        ));

        Ok(())
    }

    #[test]
    fn opt_cc_parse() -> Result<(), Error> {
        assert_eq!(
            CountryCode::from_str("br")?,
            OptionCc::from_str("BR")?.0.unwrap()
        );
        assert!(OptionCc::from_str("??")?.0.is_none());

        Ok(())
    }
}
