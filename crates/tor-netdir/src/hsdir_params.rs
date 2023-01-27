//! Compute which time period and shared random value from a consensus to use at
//! any given time.
//!
//! This is, unfortunately, a bit complex.  It works as follows:
//!
//!   * The _current_ time period is the one that contains the valid-after time
//!     for the consensus...
//!      * but to compute the time period interval, you need to look at the
//!        consensus parameters,
//!      * and to compute the time period offset, you need to know the consensus
//!        voting interval.
//!
//!   * The SRV for any given time period is the one that that was the most
//!     recent at the _start_ of the time period...
//!      * but to know when an SRV was most recent, you need to read a timestamp
//!        from it that won't be there until proposal 342 is implemented...
//!      * and until then, you have to compute the start of the UTC day when the
//!        consensus became valid.
//!
//! This module could conceivably be part of `tor-netdoc`, but it seems better
//! to make it part of `tor-netdir`: this is where we put our complexity.
use std::time::{Duration, SystemTime};

use crate::{params::NetParameters, Error, Result};
use time::{OffsetDateTime, UtcOffset};
use tor_hscrypto::time::TimePeriod;
use tor_netdoc::doc::netstatus::{Lifetime, MdConsensus, SharedRandVal};

/// Parameters for generating and using an HsDir ring.
///
/// These parameters are derived from the shared random values and time
/// parameters in the consensus, and are used to determine the
/// position of each HsDir within the ring.
#[derive(Clone, Debug)]
pub(crate) struct HsRingParams {
    /// The time period for this ring.  It's used to ensure that blinded onion
    /// keys rotate in a _predictable_ way over time.
    pub(crate) time_period: TimePeriod,
    /// The SharedRandVal for this ring.  It's used to ensure that the position
    /// of each HsDir within the ring rotates _unpredictably_ over time.
    pub(crate) shared_rand: SharedRandVal,
}

/// By how many voting periods do we offset the beginning of our first time
/// period from the epoch?
///
/// We do this so that each of our time periods begins at a time when the SRV is
/// not rotating.
const VOTING_PERIODS_IN_OFFSET: u32 = 12;

/// How many voting periods make up an entire round of the shared random value
/// commit-and-reveal protocol?
///
/// We use this to compute an SRV lifetime if one of the SRV values is missing.
const VOTING_PERIODS_IN_SRV_ROUND: u32 = 24;

/// One day.
const ONE_DAY: Duration = Duration::new(86400, 0);

/// Compute the `HsRingParams` for the current time period, according to a given
/// consensus.
///
/// rend-spec-v3 section 2.2.1 et seq
///
/// Return the ring parameters for the current period (which clients use when
/// fetching onion service descriptors), along with a SmallVec of ring
/// parameters for any secondary periods that onion services should additionally
/// use when publishing their descriptors.
///
/// Note that "current" here is always relative to a given consensus, not the
/// current wall-clock time.
///
/// (This function's return type is a bit cumbersome; these parameters are
/// bundled together because it is efficient to compute them all at once.)
pub(crate) fn compute_ring_parameters(
    consensus: &MdConsensus,
    params: &NetParameters,
) -> Result<(HsRingParams, Vec<HsRingParams>)> {
    let srvs = extract_srvs(consensus)?;
    let tp_length: Duration = params.hsdir_timeperiod_length.try_into().map_err(|_| {
        Error::InvalidConsensus("Minutes in hsdir timeperiod could not be converted to a Duration")
    })?;
    let offset = voting_period(consensus.lifetime())? * VOTING_PERIODS_IN_OFFSET;
    let cur_period = TimePeriod::new(tp_length, consensus.lifetime().valid_after(), offset)
        .expect("Consensus valid-after did not fall in a time period");
    let cur_period_start = cur_period
        .range()
        .ok_or(Error::InvalidConsensus(
            "HsDir time period in consensus could not be represented as a SystemTime range.",
        ))?
        .start;

    let cur_srv =
        find_srv_for_time(&srvs[..], cur_period_start).unwrap_or_else(|| disaster_srv(cur_period));
    let main_ring = HsRingParams {
        time_period: cur_period,
        shared_rand: cur_srv,
    };

    // When computing secondary rings, we don't try so many fallback operations:
    // if they aren't available, they aren't available.
    let mut other_rings = Vec::new();
    for period in [cur_period.prev(), cur_period.next()].iter().flatten() {
        if let Some(period_range) = period.range() {
            if let Some(srv) = find_srv_for_time(&srvs[..], period_range.start) {
                other_rings.push(HsRingParams {
                    time_period: *period,
                    shared_rand: srv,
                });
            }
        }
    }

    Ok((main_ring, other_rings))
}

/// Compute the "Disaster SRV" for a given time period.
///
/// This SRV is used if the authorities do not list any shared random value for
/// that time period, but we need to compute an HsDir ring for it anyway.
fn disaster_srv(period: TimePeriod) -> SharedRandVal {
    use digest::Digest;
    let mut d = tor_llcrypto::d::Sha3_256::new();
    d.update(b"shared-random-disaster");
    d.update((period.length_in_sec() / 60).to_be_bytes());
    d.update(period.interval_num().to_be_bytes());

    let v: [u8; 32] = d.finalize().into();
    v.into()
}

/// Helper type: A `SharedRandVal`, and the time range over which it is the most
/// recent.
type SrvInfo = (SharedRandVal, std::ops::Range<SystemTime>);

/// Given a list of SrvInfo, return the SharedRandVal (if any) that is the most
/// recent SRV at `when`.
fn find_srv_for_time(info: &[SrvInfo], when: SystemTime) -> Option<SharedRandVal> {
    info.iter()
        .find(|(_, range)| range.contains(&when))
        .map(|(srv, _)| *srv)
}

/// Return every SRV from a consensus, along with a duration over which it is
/// most recent SRV.
fn extract_srvs(consensus: &MdConsensus) -> Result<Vec<SrvInfo>> {
    let mut v = Vec::new();
    let consensus_ts = consensus.lifetime().valid_after();
    let srv_interval = srv_interval(consensus)?;

    if let Some(cur) = consensus.shared_rand_cur() {
        let ts_begin = cur
            .timestamp()
            .unwrap_or_else(|| start_of_day_containing(consensus_ts));
        let ts_end = ts_begin + srv_interval;
        v.push((*cur.value(), ts_begin..ts_end));
    }
    if let Some(prev) = consensus.shared_rand_prev() {
        let ts_begin = prev
            .timestamp()
            .unwrap_or_else(|| start_of_day_containing(consensus_ts) - ONE_DAY);
        let ts_end = ts_begin + srv_interval;
        v.push((*prev.value(), ts_begin..ts_end));
    }

    Ok(v)
}

/// Return the length of time for which a single SRV value is valid.
fn srv_interval(consensus: &MdConsensus) -> Result<Duration> {
    // What we _want_ to do, ideally, is is to learn the duration from the
    // difference between the declared time for the previous value and the
    // declared time for the current one.
    //
    // (This assumes that proposal 342 is implemented.)
    if let (Some(cur), Some(prev)) = (consensus.shared_rand_cur(), consensus.shared_rand_prev()) {
        if let (Some(cur_ts), Some(prev_ts)) = (cur.timestamp(), prev.timestamp()) {
            if let Ok(d) = cur_ts.duration_since(prev_ts) {
                return Ok(d);
            }
        }
    }

    // But if one of those values is missing, or if it has no timestamp, we have
    // to fall back to admitting that we know the schedule for the voting
    // algorithm.
    voting_period(consensus.lifetime()).map(|d| d * VOTING_PERIODS_IN_SRV_ROUND)
}

/// Return the length of the voting period in the consensus.
///
/// (The "voting period" is the length of time between between one consensus and the next.)
fn voting_period(lifetime: &Lifetime) -> Result<Duration> {
    // TODO hs: consider moving this function to be a method of Lifetime.
    let valid_after = lifetime.valid_after();
    let fresh_until = lifetime.fresh_until();
    fresh_until
        .duration_since(valid_after)
        .map_err(|_| Error::InvalidConsensus("Mis-formed lifetime"))
}

/// Return a time at the start of the UTC day containing `t`.
fn start_of_day_containing(t: SystemTime) -> SystemTime {
    OffsetDateTime::from(t)
        .to_offset(UtcOffset::UTC)
        .replace_time(time::macros::time!(00:00))
        .into()
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
    use hex_literal::hex;
    use tor_netdoc::doc::netstatus::{ConsensusBuilder, MdConsensusRouterStatus};

    /// Helper: parse an rfc3339 time.
    ///
    /// # Panics
    ///
    /// Panics if the time is invalid.
    fn t(s: &str) -> SystemTime {
        humantime::parse_rfc3339(s).unwrap()
    }
    /// Helper: parse a duration.
    ///
    /// # Panics
    ///
    /// Panics if the time is invalid.
    fn d(s: &str) -> Duration {
        humantime::parse_duration(s).unwrap()
    }

    fn example_lifetime() -> Lifetime {
        Lifetime::new(
            t("1985-10-25T07:00:00Z"),
            t("1985-10-25T08:00:00Z"),
            t("1985-10-25T10:00:00Z"),
        )
        .unwrap()
    }

    const SRV1: [u8; 32] = *b"next saturday night were sending";
    const SRV2: [u8; 32] = *b"you......... back to the future!";

    fn example_consensus_builder() -> ConsensusBuilder<MdConsensusRouterStatus> {
        let mut bld = MdConsensus::builder();

        bld.consensus_method(34)
            .lifetime(example_lifetime())
            .param("bwweightscale", 1)
            .param("hsdir_interval", 1440)
            .weights("".parse().unwrap())
            .shared_rand_prev(7, SRV1.into(), None)
            .shared_rand_cur(7, SRV2.into(), None);

        bld
    }

    #[test]
    fn start_of_day() {
        assert_eq!(
            start_of_day_containing(t("1985-10-25T07:00:00Z")),
            t("1985-10-25T00:00:00Z")
        );
        assert_eq!(
            start_of_day_containing(t("1985-10-25T00:00:00Z")),
            t("1985-10-25T00:00:00Z")
        );
        assert_eq!(
            start_of_day_containing(t("1985-10-25T23:59:59.999Z")),
            t("1985-10-25T00:00:00Z")
        );
    }

    #[test]
    fn vote_period() {
        assert_eq!(voting_period(&example_lifetime()).unwrap(), d("1 hour"));

        let lt2 = Lifetime::new(
            t("1985-10-25T07:00:00Z"),
            t("1985-10-25T07:22:00Z"),
            t("1985-10-25T07:59:00Z"),
        )
        .unwrap();

        assert_eq!(voting_period(&lt2).unwrap(), d("22 min"));
    }

    #[test]
    fn srv_period() {
        // In a basic consensus with no SRV timestamps, we'll assume 24 voting periods.
        let consensus = example_consensus_builder().testing_consensus().unwrap();
        assert_eq!(srv_interval(&consensus).unwrap(), d("1 day"));

        // If there are timestamps, we look at the difference between them.
        let consensus = example_consensus_builder()
            .shared_rand_prev(7, SRV1.into(), Some(t("1985-10-25T00:00:00Z")))
            .shared_rand_cur(7, SRV2.into(), Some(t("1985-10-25T06:00:05Z")))
            .testing_consensus()
            .unwrap();
        assert_eq!(srv_interval(&consensus).unwrap(), d("6 hours 5 sec"));

        // Note that if the timestamps are in reversed order, we fall back to 24 hours.
        let consensus = example_consensus_builder()
            .shared_rand_cur(7, SRV1.into(), Some(t("1985-10-25T00:00:00Z")))
            .shared_rand_prev(7, SRV2.into(), Some(t("1985-10-25T06:00:05Z")))
            .testing_consensus()
            .unwrap();
        assert_eq!(srv_interval(&consensus).unwrap(), d("1 day"));
    }

    #[test]
    fn srvs_extract_and_find() {
        let consensus = example_consensus_builder().testing_consensus().unwrap();
        let srvs = extract_srvs(&consensus).unwrap();
        assert_eq!(
            srvs,
            vec![
                // Since no timestamps are given in the example, the current srv
                // is valid from midnight to midnight...
                (
                    SRV2.into(),
                    t("1985-10-25T00:00:00Z")..t("1985-10-26T00:00:00Z")
                ),
                // ...and the previous SRV is valid midnight-to-midnight on the
                // previous day.
                (
                    SRV1.into(),
                    t("1985-10-24T00:00:00Z")..t("1985-10-25T00:00:00Z")
                )
            ]
        );

        // Now try with explicit timestamps on the SRVs.
        let consensus = example_consensus_builder()
            .shared_rand_prev(7, SRV1.into(), Some(t("1985-10-25T00:00:00Z")))
            .shared_rand_cur(7, SRV2.into(), Some(t("1985-10-25T06:00:05Z")))
            .testing_consensus()
            .unwrap();
        let srvs = extract_srvs(&consensus).unwrap();
        assert_eq!(
            srvs,
            vec![
                (
                    SRV2.into(),
                    t("1985-10-25T06:00:05Z")..t("1985-10-25T12:00:10Z")
                ),
                (
                    SRV1.into(),
                    t("1985-10-25T00:00:00Z")..t("1985-10-25T06:00:05Z")
                )
            ]
        );

        // See if we can look up SRVs in that period.
        assert_eq!(None, find_srv_for_time(&srvs, t("1985-10-24T23:59:00Z")));
        assert_eq!(
            Some(SRV1.into()),
            find_srv_for_time(&srvs, t("1985-10-25T00:00:00Z"))
        );
        assert_eq!(
            Some(SRV1.into()),
            find_srv_for_time(&srvs, t("1985-10-25T03:59:00Z"))
        );
        assert_eq!(
            Some(SRV1.into()),
            find_srv_for_time(&srvs, t("1985-10-25T00:00:00Z"))
        );
        assert_eq!(
            Some(SRV2.into()),
            find_srv_for_time(&srvs, t("1985-10-25T06:00:05Z"))
        );
        assert_eq!(
            Some(SRV2.into()),
            find_srv_for_time(&srvs, t("1985-10-25T12:00:00Z"))
        );
        assert_eq!(None, find_srv_for_time(&srvs, t("1985-10-25T12:00:30Z")));
    }

    #[test]
    fn disaster() {
        use digest::Digest;
        use tor_llcrypto::d::Sha3_256;
        let period = TimePeriod::new(d("1 day"), t("1970-01-02T17:33:00Z"), d("12 hours")).unwrap();
        assert_eq!(period.length_in_sec(), 86400);
        assert_eq!(period.interval_num(), 1);

        let dsrv = disaster_srv(period);
        assert_eq!(
            dsrv.as_ref(),
            &hex!("F8A4948707653837FA44ABB5BBC75A12F6F101E7F8FAF699B9715F4965D3507D")
        );
        assert_eq!(
            &dsrv.as_ref()[..],
            &Sha3_256::digest(b"shared-random-disaster\0\0\0\0\0\0\x05\xA0\0\0\0\0\0\0\0\x01")[..]
        );
    }

    #[test]
    fn ring_params_simple() {
        // Compute ring parameters in a legacy environment, where the time
        // period and the SRV lifetime are one day long, and they are offset by
        // 12 hours.
        let consensus = example_consensus_builder().testing_consensus().unwrap();
        let netparams = NetParameters::from_map(consensus.params());
        let (cur, secondary) = compute_ring_parameters(&consensus, &netparams).unwrap();

        assert_eq!(
            cur.time_period,
            TimePeriod::new(d("1 day"), t("1985-10-25T07:00:00Z"), d("12 hours")).unwrap()
        );
        // We use the "previous" SRV since the start of this time period was 12:00 on the 24th.
        assert_eq!(cur.shared_rand.as_ref(), &SRV1);

        // Our secondary SRV will be the one that starts when we move into the
        // next time period.
        assert_eq!(secondary.len(), 1);
        assert_eq!(
            secondary[0].time_period,
            TimePeriod::new(d("1 day"), t("1985-10-25T12:00:00Z"), d("12 hours")).unwrap(),
        );
        assert_eq!(secondary[0].shared_rand.as_ref(), &SRV2);
    }

    #[test]
    fn ring_params_tricky() {
        // In this case we give the SRVs timestamps and we choose an odd hsdir_interval.
        let consensus = example_consensus_builder()
            .shared_rand_prev(7, SRV1.into(), Some(t("1985-10-25T00:00:00Z")))
            .shared_rand_cur(7, SRV2.into(), Some(t("1985-10-25T05:00:00Z")))
            .param("hsdir_interval", 120) // 2 hours
            .testing_consensus()
            .unwrap();
        let netparams = NetParameters::from_map(consensus.params());
        let (cur, secondary) = compute_ring_parameters(&consensus, &netparams).unwrap();

        assert_eq!(
            cur.time_period,
            TimePeriod::new(d("2 hours"), t("1985-10-25T07:00:00Z"), d("12 hours")).unwrap()
        );
        assert_eq!(cur.shared_rand.as_ref(), &SRV2);

        assert_eq!(secondary.len(), 2);
        assert_eq!(
            secondary[0].time_period,
            TimePeriod::new(d("2 hours"), t("1985-10-25T05:00:00Z"), d("12 hours")).unwrap()
        );
        assert_eq!(secondary[0].shared_rand.as_ref(), &SRV1);
        assert_eq!(
            secondary[1].time_period,
            TimePeriod::new(d("2 hours"), t("1985-10-25T09:00:00Z"), d("12 hours")).unwrap()
        );
        assert_eq!(secondary[1].shared_rand.as_ref(), &SRV2);
    }
}
