//! Wall-clock deadlines with a monotonic floor: clock rollback cannot extend grants.
use chrono::{DateTime, SecondsFormat, Utc};
use serde::{de::Error, Deserialize, Deserializer, Serialize, Serializer};
use std::time::Duration;
use tokio::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Timestamp(pub DateTime<Utc>);
impl Timestamp {
    pub fn after(self, duration: Duration) -> Self {
        Self(self.0 + chrono::Duration::from_std(duration).expect("bounded duration"))
    }
}
// Chrono deliberately accepts more RFC3339 variants than the admin protocol.
fn utc_shape(value: &[u8]) -> bool {
    let core = value
        .strip_suffix(b"Z")
        .or_else(|| value.strip_suffix(b"+00:00"));
    let Some(core) = core else {
        return false;
    };
    if core.len() < 19 {
        return false;
    }
    for (i, byte) in core[..19].iter().enumerate() {
        let separator = match i {
            4 | 7 => Some(b'-'),
            10 => Some(b'T'),
            13 | 16 => Some(b':'),
            _ => None,
        };
        if separator.map_or(!byte.is_ascii_digit(), |expected| *byte != expected) {
            return false;
        }
    }
    core.len() == 19
        || ((21..=29).contains(&core.len())
            && core[19] == b'.'
            && core[20..].iter().all(u8::is_ascii_digit))
}
impl<'de> Deserialize<'de> for Timestamp {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let value = String::deserialize(d)?;
        if !utc_shape(value.as_bytes()) {
            return Err(D::Error::custom("Invalid UTC timestamp"));
        }
        let parsed = DateTime::parse_from_rfc3339(&value)
            .map_err(|_| D::Error::custom("Invalid UTC timestamp"))?;
        if parsed.offset().local_minus_utc() != 0
            || parsed.timestamp_subsec_nanos() >= 1_000_000_000
        {
            return Err(D::Error::custom("Invalid UTC timestamp"));
        }
        Ok(Self(parsed.with_timezone(&Utc)))
    }
}
impl Serialize for Timestamp {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0.to_rfc3339_opts(SecondsFormat::AutoSi, true))
    }
}

#[derive(Clone)]
pub struct Clock {
    wall: Timestamp,
    start: Instant,
    system: bool,
}
impl Default for Clock {
    fn default() -> Self {
        Self {
            wall: Timestamp(Utc::now()),
            start: Instant::now(),
            system: true,
        }
    }
}
impl Clock {
    /// Inject an anchored clock; Tokio's paused clock drives both deadlines and now().
    pub fn at(wall: Timestamp) -> Self {
        Self {
            wall,
            start: Instant::now(),
            system: false,
        }
    }
    pub fn now(&self) -> Timestamp {
        let monotonic = self.wall.after(self.start.elapsed());
        if self.system {
            monotonic.max(Timestamp(Utc::now()))
        } else {
            monotonic
        }
    }
    pub async fn until(&self, deadline: Timestamp) {
        loop {
            let remaining = (deadline.0 - self.now().0).to_std().unwrap_or_default();
            if remaining.is_zero() {
                return;
            }
            // Recheck wall-clock jumps as well as the monotonic deadline.
            tokio::time::sleep(remaining.min(Duration::from_secs(1))).await;
        }
    }
}
