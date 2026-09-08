// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Eamun Rahimi

//! Runtime configuration, read from environment variables that the CDK `ComputeStack` injects.
//!
//! Every accessor here is *required* — there are deliberately no fallback defaults. A default
//! affiliate id would silently sync a stranger's gym into your Hevy account; a default timezone
//! would silently fetch the wrong day's workout. Both are worse than a startup error, so a
//! missing variable fails the invocation loudly and says exactly which one is absent.

use crate::error::{Result, SwhError};
use chrono_tz::Tz;
use std::str::FromStr;

/// Secrets Manager id of the Hevy API key secret (`{"api_key": "..."}`).
pub const HEVY_SECRET_ID_VAR: &str = "HEVY_SECRET_ID";
/// SugarWOD affiliate slug identifying whose workouts to fetch.
pub const AFFILIATE_ID_VAR: &str = "SUGARWOD_AFFILIATE_ID";
/// IANA timezone name deciding what "today" means. Must match the schedule's timezone.
pub const LOCAL_TIMEZONE_VAR: &str = "LOCAL_TIMEZONE";
/// SSM parameter name holding the Discord webhook URL.
pub const DISCORD_WEBHOOK_PARAM_VAR: &str = "DISCORD_WEBHOOK_PARAM_NAME";

/// Reads a required environment variable, erroring by name if it is unset or empty.
pub fn required_env(name: &str) -> Result<String> {
    match std::env::var(name) {
        Ok(v) if !v.trim().is_empty() => Ok(v.trim().to_string()),
        _ => Err(SwhError::Config(format!(
            "missing required environment variable {name} (set by the CDK ComputeStack; \
             see cdk/lib/config.ts)"
        ))),
    }
}

pub fn hevy_secret_id() -> Result<String> {
    required_env(HEVY_SECRET_ID_VAR)
}

pub fn affiliate_id() -> Result<String> {
    required_env(AFFILIATE_ID_VAR)
}

/// The SSM parameter holding the Discord webhook URL, or `None` when notifications are off.
///
/// Unlike every other accessor here this one is optional, because Discord notification is a
/// convenience rather than part of syncing. It is **disabled by default**: the CDK
/// `ComputeStack` only injects this variable when the `discordEnabled` context flag is set, and
/// when it is off the execution role is not granted `ssm:GetParameter` either. An absent
/// variable therefore means "deliberately off", not "misconfigured", and must not warn.
pub fn discord_webhook_param() -> Option<String> {
    match std::env::var(DISCORD_WEBHOOK_PARAM_VAR) {
        Ok(v) if !v.trim().is_empty() => Some(v.trim().to_string()),
        _ => None,
    }
}

/// Resolves `LOCAL_TIMEZONE` to a `chrono_tz::Tz`. An unparseable name is an error rather than a
/// silent fall back to UTC — falling back would shift "today" by up to a day and quietly sync the
/// wrong workout.
pub fn local_timezone() -> Result<Tz> {
    let name = required_env(LOCAL_TIMEZONE_VAR)?;
    Tz::from_str(&name).map_err(|_| {
        SwhError::Config(format!(
            "{LOCAL_TIMEZONE_VAR}=\"{name}\" is not a valid IANA timezone name \
             (expected something like \"America/Los_Angeles\" or \"Europe/London\")"
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn required_env_names_the_missing_variable() {
        let err = required_env("SWH_TEST_DEFINITELY_UNSET_VAR").unwrap_err();
        assert!(err.to_string().contains("SWH_TEST_DEFINITELY_UNSET_VAR"));
    }

    #[test]
    fn timezone_parse_rejects_a_bogus_name() {
        assert!(Tz::from_str("Not/AZone").is_err());
    }

    #[test]
    fn timezone_parse_accepts_iana_names() {
        assert!(Tz::from_str("America/Los_Angeles").is_ok());
        assert!(Tz::from_str("Europe/London").is_ok());
        assert!(Tz::from_str("UTC").is_ok());
    }
}
