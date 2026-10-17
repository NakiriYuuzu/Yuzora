//! Wall-clock deadlines with a monotonic floor: clock rollback cannot extend grants.
use chrono::{DateTime, SecondsFormat, Utc};
use serde::{de::Error, Deserialize, Deserializer, Serialize, Serializer};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
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
    // Largest wall-clock lead over the monotonic estimate seen by any clone. A
    // corrected forward jump keeps the lead, so now() never rewinds into an
    // expired deadline and still advances at the monotonic rate.
    skew: Arc<Mutex<chrono::Duration>>,
}
impl Default for Clock {
    fn default() -> Self {
        Self {
            wall: Timestamp(Utc::now()),
            start: Instant::now(),
            system: true,
            skew: Arc::default(),
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
            skew: Arc::default(),
        }
    }
    pub fn now(&self) -> Timestamp {
        self.observe(self.system.then(Utc::now))
    }
    fn observe(&self, system: Option<DateTime<Utc>>) -> Timestamp {
        let monotonic = self.wall.after(self.start.elapsed());
        let mut skew = self.skew.lock().unwrap();
        if let Some(system) = system {
            *skew = (*skew).max(system - monotonic.0);
        }
        Timestamp(monotonic.0 + *skew)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn corrected_forward_jump_neither_rewinds_nor_freezes() {
        let clock = Clock::at(serde_json::from_str("\"2030-01-01T00:00:00Z\"").unwrap());
        let base = clock.now();
        let hour = Duration::from_secs(3600);
        let jumped = clock.observe(Some(base.after(hour).0));
        assert_eq!(jumped, base.after(hour));
        // A correction back to the true time must not revive expired deadlines.
        assert!(clock.clone().observe(Some(base.0)) >= jumped);
        // Nor may it stall: deadlines issued after the correction keep their length.
        tokio::time::advance(Duration::from_secs(10)).await;
        let later = clock.observe(Some(base.after(Duration::from_secs(10)).0));
        assert!(later >= jumped.after(Duration::from_secs(10)));
    }
}
