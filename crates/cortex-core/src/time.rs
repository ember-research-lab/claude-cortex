//! UTC timestamps in RFC3339 `Z` form for v3 ledgers.

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{de, Deserialize, Deserializer, Serialize, Serializer};

/// UTC datetime serialized as RFC3339 with `Z` suffix and microsecond precision.
/// Example: `2026-05-06T21:45:03.523577Z`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UtcTime(pub DateTime<Utc>);

impl UtcTime {
    pub fn now() -> Self {
        Self(Utc::now())
    }

    pub fn into_inner(self) -> DateTime<Utc> {
        self.0
    }

    pub fn as_str(&self) -> String {
        format_rfc3339_z(&self.0)
    }
}

impl From<DateTime<Utc>> for UtcTime {
    fn from(dt: DateTime<Utc>) -> Self {
        Self(dt)
    }
}

impl From<UtcTime> for DateTime<Utc> {
    fn from(t: UtcTime) -> Self {
        t.0
    }
}

/// Format a UTC datetime as RFC3339 with microsecond precision and `Z` suffix.
/// Uses chrono's [`SecondsFormat::Micros`] so output is always 6 digits past the dot.
pub fn format_rfc3339_z(dt: &DateTime<Utc>) -> String {
    dt.to_rfc3339_opts(SecondsFormat::Micros, true)
}

/// Parse RFC3339 / ISO-8601 datetime with optional `Z` or numeric offset.
pub fn parse_rfc3339(s: &str) -> Result<DateTime<Utc>, chrono::ParseError> {
    let dt = DateTime::parse_from_rfc3339(s)?;
    Ok(dt.with_timezone(&Utc))
}

impl Serialize for UtcTime {
    fn serialize<S: Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        ser.serialize_str(&self.as_str())
    }
}

impl<'de> Deserialize<'de> for UtcTime {
    fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        let s = String::deserialize(de)?;
        parse_rfc3339(&s).map(UtcTime).map_err(de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn round_trips_through_z_form() {
        let dt = Utc.with_ymd_and_hms(2026, 5, 6, 21, 45, 3).unwrap()
            + chrono::Duration::microseconds(523_577);
        let s = format_rfc3339_z(&dt);
        assert_eq!(s, "2026-05-06T21:45:03.523577Z");
        assert_eq!(parse_rfc3339(&s).unwrap(), dt);
    }
}
