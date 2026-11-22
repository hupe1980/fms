//! The calendar the Festlegung is written in.

use jiff::Timestamp;
use jiff::civil::Date;
use jiff::tz::TimeZone;

/// Europe/Berlin — days, months and the publication period are local German time.
#[must_use]
pub fn berlin() -> TimeZone {
    TimeZone::get("Europe/Berlin").unwrap_or(TimeZone::UTC)
}

/// The local date of an instant.
#[must_use]
pub fn local_date(t: Timestamp) -> Date {
    t.to_zoned(berlin()).date()
}
