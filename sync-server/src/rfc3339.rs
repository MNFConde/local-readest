//! RFC3339 helpers matching the Go reference's timestamp semantics exactly:
//! field timestamps use second-precision RFC3339 UTC (`time.RFC3339`), the
//! books pull cursor uses RFC3339 with trailing-zero-trimmed nanos
//! (`RFC3339Nano`).

use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

pub fn now_unix_ms() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0),
    )
    .unwrap_or(0)
}

pub fn now_rfc3339() -> String {
    epoch_ms_to_rfc3339(now_unix_ms()).unwrap_or_else(|| "1970-01-01T00:00:00Z".into())
}

/// epoch ms → RFC3339 UTC, second precision (millis truncated), Go `RFC3339`.
pub fn epoch_ms_to_rfc3339(ms: i64) -> Option<String> {
    let secs = ms.div_euclid(1000);
    let dt = OffsetDateTime::from_unix_timestamp(secs).ok()?;
    dt.format(&Rfc3339).ok()
}

/// Optional-input variant used by transform.rs for nullable timestamp fields.
pub fn epoch_ms_to_rfc3339_opt(ms: Option<i64>) -> Option<String> {
    ms.and_then(epoch_ms_to_rfc3339)
}

/// epoch ms → RFC3339 with nanoseconds, trailing zeros trimmed (Go
/// `RFC3339Nano`); `ms <= 0` yields `""` (the reference's isoFromMs).
pub fn iso_from_ms(ms: i64) -> String {
    if ms <= 0 {
        return String::new();
    }
    let millis = (ms % 1000) as u32;
    // Build fraction manually: Go prints 3 digits for ms input then trims zeros.
    let base = epoch_ms_to_rfc3339(ms - millis as i64).unwrap_or_default();
    if millis == 0 {
        return base;
    }
    match base.split_once('T') {
        Some((date, rest)) => {
            let frac = format!("{millis:03}");
            let frac = frac.trim_end_matches('0');
            format!("{date}T{}.{}Z", rest.trim_end_matches('Z'), frac)
        }
        None => base,
    }
}

/// RFC3339 (with optional fraction) → epoch ms. Used to convert a merged
/// field timestamp back to the row's `updated_at` clock.
pub fn rfc3339_to_epoch_ms(s: &str) -> Option<i64> {
    let dt = OffsetDateTime::parse(s, &Rfc3339).ok()?;
    let ms = dt.unix_timestamp_nanos() / 1_000_000;
    i64::try_from(ms).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_conversion_roundtrips() {
        assert_eq!(epoch_ms_to_rfc3339(0), Some("1970-01-01T00:00:00Z".into()));
        assert_eq!(
            epoch_ms_to_rfc3339(1_700_000_000_123),
            Some("2023-11-14T22:13:20Z".into())
        );
        assert_eq!(iso_from_ms(0), "");
        assert_eq!(iso_from_ms(1_700_000_000_000), "2023-11-14T22:13:20Z");
        assert_eq!(iso_from_ms(1_700_000_000_120), "2023-11-14T22:13:20.12Z");
        assert_eq!(iso_from_ms(1_700_000_000_123), "2023-11-14T22:13:20.123Z");
        assert_eq!(
            rfc3339_to_epoch_ms("2023-11-14T22:13:20.123Z"),
            Some(1_700_000_000_123)
        );
        assert_eq!(
            rfc3339_to_epoch_ms("2023-11-14T22:13:20Z"),
            Some(1_700_000_000_000)
        );
    }
}
