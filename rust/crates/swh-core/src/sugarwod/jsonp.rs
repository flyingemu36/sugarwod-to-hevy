// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Eamun Rahimi

use crate::error::{Result, SwhError};
use serde::de::DeserializeOwned;

/// Unwraps a JSONP response like `SugarWOD.workoutsFetched({...});` and deserializes the
/// inner JSON. Tries a direct parse first (the shape returned by a live HTTP call); if that
/// fails, retries after undoing one layer of `\"`/`\\` escaping — the shape the text takes when
/// it has been re-embedded inside a Lambda-proxy `body` string, as captured fixtures often are.
pub fn unwrap_jsonp<T: DeserializeOwned>(text: &str) -> Result<T> {
    let start = text
        .find('(')
        .ok_or_else(|| SwhError::SugarWodParse("no '(' found in JSONP response".into()))?;
    let end = text
        .rfind(')')
        .ok_or_else(|| SwhError::SugarWodParse("no ')' found in JSONP response".into()))?;
    if end <= start {
        return Err(SwhError::SugarWodParse(
            "malformed JSONP: ')' before '('".into(),
        ));
    }
    let inner = &text[start + 1..end];

    if let Ok(value) = serde_json::from_str::<T>(inner) {
        return Ok(value);
    }

    let unescaped = inner.replace("\\\"", "\"").replace("\\\\", "\\");
    serde_json::from_str::<T>(&unescaped)
        .map_err(|e| SwhError::SugarWodParse(format!("failed to parse JSONP body: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sugarwod::model::SugarWodResponse;

    #[test]
    fn unwraps_plain_jsonp() {
        let text = r#"SugarWOD.workoutsFetched({"success":true,"data":[]});"#;
        let parsed: SugarWodResponse = unwrap_jsonp(text).unwrap();
        assert!(parsed.success);
        assert!(parsed.data.is_empty());
    }

    #[test]
    fn unwraps_double_escaped_jsonp_from_legacy_fixture() {
        // Mirrors the shape captured when the old system re-embedded JSONP text inside a
        // Lambda-proxy `body` JSON string.
        let text = "SugarWOD.workoutsFetched({\\\"success\\\":true,\\\"data\\\":[]});";
        let parsed: SugarWodResponse = unwrap_jsonp(text).unwrap();
        assert!(parsed.success);
        assert!(parsed.data.is_empty());
    }

    #[test]
    fn unwraps_real_workout_payload() {
        let text = r#"SugarWOD.workoutsFetched({"success":true,"data":[{"id":"vFoCrHl7dr","track":"workout-of-the-day","title":"Performance + Fitness","description":"A.5:00 EMOM\n2 Push Press @ 65%","scheduledDateInteger":20260801,"scheduledDateDisplay":"Saturday, Aug 1, 2026","movements":[]}]});"#;
        let parsed: SugarWodResponse = unwrap_jsonp(text).unwrap();
        assert_eq!(parsed.data.len(), 1);
        assert_eq!(parsed.data[0].title, "Performance + Fitness");
        assert!(parsed.data[0].movements.is_empty());
    }
}
