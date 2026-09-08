// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Eamun Rahimi

use chrono::Utc;
use chrono_tz::Tz;

/// Today's date in `tz`, formatted as SugarWOD's `YYYYMMDD`.
///
/// The timezone is a parameter rather than a constant because "today" is the single most
/// consequential input to the daily sync: the job fires in the early hours of the local morning,
/// which is already the *next* UTC day in the Americas. Resolving this against the wrong zone
/// fetches the wrong day's workout. It comes from `LOCAL_TIMEZONE` (see
/// [`crate::config::local_timezone`]) and must match the timezone the EventBridge schedule
/// fires in.
pub fn today_yyyymmdd_in(tz: Tz) -> String {
    Utc::now().with_timezone(&tz).format("%Y%m%d").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono_tz::America::Los_Angeles;
    use chrono_tz::UTC;

    #[test]
    fn formats_as_eight_digit_yyyymmdd() {
        let s = today_yyyymmdd_in(Los_Angeles);
        assert_eq!(s.len(), 8);
        assert!(s.chars().all(|c| c.is_ascii_digit()));
    }

    #[test]
    fn zone_choice_actually_changes_the_answer() {
        // Both are valid "today"s. The point is that the zone is load-bearing — UTC is never
        // behind Pacific, and is ahead of it for most of each Pacific day. That's exactly why
        // this is a parameter rather than a hardcoded constant.
        let pacific: i64 = today_yyyymmdd_in(Los_Angeles).parse().unwrap();
        let utc: i64 = today_yyyymmdd_in(UTC).parse().unwrap();
        assert!(
            utc >= pacific,
            "UTC ({utc}) should never be behind Pacific ({pacific})"
        );
    }
}
