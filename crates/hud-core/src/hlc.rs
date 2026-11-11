//! Hybrid logical clock (HLC): wall-clock milliseconds plus a counter, so events
//! stay ordered even when device clocks drift or two events share a millisecond.
//!
//! The text form is fixed-width hex, so plain string comparison (SQLite `ORDER BY`,
//! Postgres `text`) matches `Ord` on [`Hlc`].

use std::{
    fmt,
    str::FromStr,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::ids::DeviceId;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Hlc {
    wall_ms: u64,
    counter: u16,
    node: DeviceId,
}

impl Hlc {
    pub fn wall_ms(&self) -> u64 {
        self.wall_ms
    }
}

impl fmt::Display for Hlc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:013x}-{:04x}-{}",
            self.wall_ms, self.counter, self.node
        )
    }
}

#[derive(Debug, thiserror::Error)]
#[error("invalid HLC: {0}")]
pub struct ParseHlcError(String);

impl FromStr for Hlc {
    type Err = ParseHlcError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bad = || ParseHlcError(s.to_owned());
        let mut parts = s.splitn(3, '-');
        let wall_ms = u64::from_str_radix(parts.next().ok_or_else(bad)?, 16).map_err(|_| bad())?;
        let counter = u16::from_str_radix(parts.next().ok_or_else(bad)?, 16).map_err(|_| bad())?;
        let node = parts.next().ok_or_else(bad)?.parse().map_err(|_| bad())?;
        Ok(Self {
            wall_ms,
            counter,
            node,
        })
    }
}

impl Serialize for Hlc {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Hlc {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        String::deserialize(d)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

/// One device's clock. Methods take the wall time as an argument so tests can
/// control it; [`HlcClock::now`] and [`HlcClock::observe_now`] use the system clock.
#[derive(Debug)]
pub struct HlcClock {
    node: DeviceId,
    last: (u64, u16),
}

impl HlcClock {
    pub fn new(node: DeviceId) -> Self {
        Self { node, last: (0, 0) }
    }

    /// Restore a clock after restart so it never issues a timestamp at or below `last`.
    pub fn resume(node: DeviceId, last: Option<Hlc>) -> Self {
        Self {
            node,
            last: last.map_or((0, 0), |h| (h.wall_ms, h.counter)),
        }
    }

    /// Timestamp for a local event.
    pub fn tick(&mut self, wall_ms: u64) -> Hlc {
        let (l, c) = self.last;
        self.last = if wall_ms > l {
            (wall_ms, 0)
        } else {
            (l, c.saturating_add(1))
        };
        self.stamp()
    }

    /// Fold in a timestamp received from another device, then issue a new one.
    pub fn observe(&mut self, remote: &Hlc, wall_ms: u64) -> Hlc {
        let (l, c) = self.last;
        let (rl, rc) = (remote.wall_ms, remote.counter);
        let max = wall_ms.max(l).max(rl);
        let counter = if max == l && max == rl {
            c.max(rc).saturating_add(1)
        } else if max == l {
            c.saturating_add(1)
        } else if max == rl {
            rc.saturating_add(1)
        } else {
            0
        };
        self.last = (max, counter);
        self.stamp()
    }

    /// The most recent timestamp this clock issued, if any.
    pub fn last(&self) -> Option<Hlc> {
        (self.last != (0, 0)).then(|| self.stamp())
    }

    pub fn now(&mut self) -> Hlc {
        self.tick(system_ms())
    }

    pub fn observe_now(&mut self, remote: &Hlc) -> Hlc {
        self.observe(remote, system_ms())
    }

    fn stamp(&self) -> Hlc {
        Hlc {
            wall_ms: self.last.0,
            counter: self.last.1,
            node: self.node,
        }
    }
}

fn system_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_ticks_are_strictly_increasing_even_if_wall_clock_stalls_or_goes_back() {
        let mut c = HlcClock::new(DeviceId::new());
        let a = c.tick(1000);
        let b = c.tick(1000); // same millisecond
        let d = c.tick(900); // clock stepped backwards
        assert!(a < b && b < d);
    }

    #[test]
    fn observing_a_future_remote_timestamp_moves_us_past_it() {
        let mut a = HlcClock::new(DeviceId::new());
        let mut b = HlcClock::new(DeviceId::new());
        let far_ahead = a.tick(10_000);
        let after = b.observe(&far_ahead, 1_000);
        assert!(after > far_ahead);
        assert!(b.tick(1_001) > after);
    }

    #[test]
    fn text_form_sorts_like_ord_and_roundtrips() {
        let mut c = HlcClock::new(DeviceId::new());
        let hs: Vec<Hlc> = [5, 5, 70_000, 70_000, 70_001]
            .iter()
            .map(|&t| c.tick(t))
            .collect();
        for w in hs.windows(2) {
            assert!(w[0].to_string() < w[1].to_string());
        }
        for h in &hs {
            assert_eq!(&h.to_string().parse::<Hlc>().unwrap(), h);
        }
    }

    #[test]
    fn resume_never_goes_backwards() {
        let node = DeviceId::new();
        let last = HlcClock::new(node).tick(5_000);
        assert!(HlcClock::resume(node, Some(last)).tick(1_000) > last);
    }

    #[test]
    fn rejects_garbage() {
        assert!("nonsense".parse::<Hlc>().is_err());
    }
}
