// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Eamun Rahimi

use super::{ParsedExerciseLine, SetSpec};
use regex::Regex;
use std::sync::LazyLock;

#[derive(Debug, Clone, Copy, PartialEq)]
enum LineMode {
    Default,
    Emom {
        rounds: u32,
    },
    AmrapEstimate {
        rounds: u32,
    },
    /// A fixed, explicitly-stated round/set count — "5 Rounds", "Every 1:30 x 8 Sets" — applied
    /// to the single-set movements that follow. Same replication behavior as `Emom`.
    Rounds {
        rounds: u32,
    },
}

/// Default estimated round count for a "N-Person Team Waterfall"/team-AMRAP section — the
/// user's own "generally 3-4, I true up after" with 3 as the conservative default (trivially
/// bumped to 4 by editing the synced routine in the Hevy app).
const AMRAP_ESTIMATE_DEFAULT_ROUNDS: u32 = 3;

static SKIP_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"(?i)^[A-Z]\s+For\s+Time:",
        r"(?i)^WOD$",
        r"(?i)Day\s+WOD$",
        r"(?i)^Dumbbells?:",
        r"(?i)^Barbell:",
        r"(?i)^Kettlebell:",
        r"(?i)^Weight:",
        r"(?i)^Time\s+Cap:",
        r"(?i)^Rest:",
        r"(?i)^Notes?:",
        r"(?i)^This\s+is",
        r"(?i)^You\s+can",
        r"(?i)^Its?\s+your",
        r"(?i)^Hyrox\s+SIM$",
        // Coaching annotation lines (e.g. "* building to a moderate-heavy load").
        r"^\*",
        // Interval/tempo section headers not carrying a usable count (e.g. "Every 90 sec") — the
        // counted forms are handled by EVERY_RE first; this only catches the leftovers as noise.
        r"(?i)^[A-Z]?\.?\s*Every\s+\d+",
        // A bare "Rest" interval (e.g. an EMOM's "Minute 5: Rest" after its label is stripped).
        r"(?i)^Rest$",
        // A quoted workout name on its own line (e.g. "Vaya Con Dios") — not an exercise.
        "^[\"\u{201c}][^\"\u{201d}]*[\"\u{201d}]$",
        // A bare set/rep prescription on its own line after a movement (e.g. "5/5 x 4 Sets @ 40%").
        // Dropped as noise rather than parsed into a junk exercise. (Attaching it back to the
        // preceding movement's set count is a deferred improvement.)
        r"(?i)^\d+(?:/\d+)?\s*[xX]\s*\d+\s*sets?\b",
        // A "N visits per station" / "(… visits per station …)" annotation on an alternating-
        // stations block — describes the format, not an exercise.
        r"(?i)visits?\s+per\s+station",
        // Gym announcements from holiday/closure posts/class times, not movements.
        r"(?i)^CLUB\s+HOURS\b",
        r"(?i)^CLASS\s+OFFERINGS?\b",
    ]
    .iter()
    .map(|p| Regex::new(p).unwrap())
    .collect()
});

static EMOM_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^[A-Z]?\.?\s*(\d+):00\s*EMOM$").unwrap());
/// A second real EMOM spelling: "EMOM X 18" / "B.EMOM x18" / "EMOM 18" / "EMOM X 35 MIN" (explicit
/// interval count, optionally suffixed with a "min" unit) instead of "18:00 EMOM". Captured so the
/// header is consumed rather than mistaken for an exercise line.
static EMOM_ALT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^[A-Z]?\.?\s*EMOM\s*[xX]?\s*(\d+)(?:\s*min(?:ute)?s?)?$").unwrap()
});
/// An interval header carrying a count — "Every 1:30 x 8 Sets", "A. Every 2:30 x 4 Sets",
/// "Every 3:00 x 3 Rounds", "Every 2 Min x 14 Min", "Every 2:30 x 15 Min". The interval is a
/// `mm:ss` (g1:g2) or `N min` (g3); the count is g4 with unit g5 (Sets/Rounds => that many rounds,
/// Min => total-duration form, rounds = total / interval). Drives replication of the movement(s)
/// that follow instead of being dropped as noise.
static EVERY_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^[A-Z]?\.?\s*Every\s+(?:(\d+):(\d+)|(\d+)\s*min(?:ute)?s?)\s*[xX]\s*(\d+)\s*(sets?|rounds?|min(?:ute)?s?)\b",
    )
    .unwrap()
});
/// An "Alternating Stations" marker on an `EVERY_RE` header — signals that its N sets are spread
/// across the "Station M:" stations that follow (per-station count = N / stations) rather than
/// being N sets of a single movement.
static ALTERNATING_STATIONS_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)stations?").unwrap());
/// A bare "5 Rounds" / "5 Rounds:" line — a fixed round count applied to the movements that
/// follow (N sets each), not itself an exercise. Distinct from the "30/30 X 7 Rounds" work/rest
/// header, which is a different shape intentionally dropped in `SKIP_PATTERNS`.
static ROUNDS_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^(\d+)\s+Rounds?:?$").unwrap());
/// A "N sets of:" round-count header — "3 sets of:", "4 Sets Of". Like `ROUNDS_RE`, a fixed count
/// (N sets each) applied to the movements that follow, not itself an exercise. An optional trailing
/// colon and section letter ("A. 3 sets of:") are tolerated.
static SETS_OF_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^[A-Z]?\.?\s*(\d+)\s+Sets?\s+Of\b\s*:?\s*$").unwrap());
/// A work/rest circuit header stating a round count — "30/30 X 7 Rounds", "40/20 x 4 Rounds".
/// The leading "work/rest" pair is not a rep scheme; the captured count drives per-station
/// replication of the movements that follow (whether "N." numbered or plain-listed).
static WORKREST_ROUNDS_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\d+/\d+\s*[xX]\s*(\d+)\s+Rounds?\b").unwrap());
/// A time-boxed AMRAP with an open-ended round count, estimated the same way as a team AMRAP.
/// Covers both spellings and an optional "A."/"B." section prefix: "35 Min AMRAP", "B. 18 Min
/// AMRAP", "20:00 AMRAP", "B. 20:00 AMRAP".
static AMRAP_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^[A-Z]?\.?\s*(?:\d+:\d+|\d+\s*min(?:ute)?s?)\s+AMRAP\b").unwrap()
});
/// An open-ended AMRAP header carrying no explicit time token — "AMRAP with time remaining",
/// "C. AMRAP in time remaining", "AMRAP remaining time". Estimated the same way as a team/timed
/// AMRAP (see `AMRAP_ESTIMATE_DEFAULT_ROUNDS`); the header itself is not an exercise.
static AMRAP_OPEN_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^[A-Z]?\.?\s*AMRAP\b.*\bremaining\b").unwrap());
/// A leading section-letter prefix on a movement line — "B. Deadlift", "C. Bike". Only used to
/// strip the label once the line is known to be a movement (real section *headers* like
/// "A. Every …" / "B. 20:00 AMRAP" are consumed by their own matchers before this point). Requires
/// a following non-digit so a numbered-list marker ("2. Air Squat") stays with `LIST_MARKER_RE`.
static SECTION_LETTER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Z]\.\s+(\D.*)$").unwrap());

/// Section-boundary noise: a "For Time - N Min Cap" header, an "if done ... optional" preamble,
/// or a "20/60 x Remaining time" optional-finisher line. Unlike `SKIP_PATTERNS` (which merely
/// drops a line), matching one of these also ends the current section — resetting the round mode
/// so a following bare movement isn't inherited into the prior section's replication.
static SECTION_NOISE_RE: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"(?i)^[A-Z]?\.?\s*For\s+Time\b",
        r"(?i)^if\s+done\b",
        r"(?i)remaining\s+time",
        // A "N Min to Complete" block header (e.g. "A2. 10 Min to Complete") — starts a new
        // section, so it ends the previous section's round mode.
        r"(?i)min\s+to\s+complete",
        // A "N Min to Work" block header (e.g. "A. 35 Min to Work") — same: a section boundary,
        // not an exercise. The round count comes from a following "N sets of:" line.
        r"(?i)min\s+to\s+work",
    ]
    .iter()
    .map(|p| Regex::new(p).unwrap())
    .collect()
});

/// Accumulates one enumerated-station circuit so its per-round set count can be applied once the
/// whole section is known. A single-pass replicate-on-emit can't work for these: the stations are
/// interval-labeled ("Min N-", "N.") — which suppresses the normal mode replication — and for an
/// EMOM the round count (`intervals / stations`, with the station total including a dropped "Rest"
/// minute) isn't known until the section ends. Covers two shapes:
/// - EMOM ("35:00 EMOM" / "EMOM X 35"): `explicit_rounds` is `None`, rounds = `intervals / stations`.
/// - work/rest ("30/30 X 7 Rounds"): `explicit_rounds` is `Some(7)`, used directly.
struct EmomAccum {
    /// Total interval count from an EMOM header (e.g. 35 for "EMOM X 35"). Unused when
    /// `explicit_rounds` is set.
    intervals: u32,
    /// A round count stated outright by the header ("... X 7 Rounds"), bypassing the
    /// `intervals / stations` computation.
    explicit_rounds: Option<u32>,
    /// Stations seen in the cycle so far, including a "Rest" minute that isn't itself emitted.
    station_count: u32,
    /// Indices into `lines_out` of the emitted (non-Rest) stations to replicate.
    emitted_idxs: Vec<usize>,
}

/// Finalizes a circuit section: replicates each emitted station's single set to the round count —
/// `explicit_rounds` when the header stated one, else `intervals / stations` (Fitness "EMOM X 35"
/// over 5 stations -> 7). No-op when the count resolves to 1 or less, or the section had no stations.
fn apply_emom_rounds(lines_out: &mut [ParsedExerciseLine], accum: &EmomAccum) {
    let rounds = match accum.explicit_rounds {
        Some(r) => r,
        None => {
            if accum.station_count == 0 || accum.intervals == 0 {
                return;
            }
            (accum.intervals / accum.station_count).max(1)
        }
    };
    if rounds <= 1 {
        return;
    }
    for &idx in &accum.emitted_idxs {
        let line = &mut lines_out[idx];
        if line.sets.len() == 1 {
            let base = line.sets[0].clone();
            line.sets = std::iter::repeat_n(base, rounds as usize).collect();
        }
    }
}
/// A leading interval/station label — "Minute 3:", "Min 3-", or an alternating "Odd:"/"Even:"
/// station. Stripped so the movement (and its rep prescription) is what gets parsed, and flags the
/// line as an enumerated EMOM station so it is NOT set-replicated across the whole interval (its
/// per-round count comes from the section finalize: intervals / stations, Odd+Even = 2 stations).
static INTERVAL_LABEL_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(?:Min(?:ute)?\s*\d+|Odd|Even|Station\s*\d+)\s*[:\-]\s*").unwrap()
});
/// A leading numbered-list marker — "1. DB Pullover", "2) Air Squat", "1.Back Squat". Captured
/// (rest must start with a non-digit, so decimals like "1.5" are left intact) and stripped so the
/// movement resolves. Like interval labels, these enumerate distinct stations — no replication.
static LIST_MARKER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\d+[.)]\s*(\D.*)$").unwrap());
/// A sloppier numbered-list marker fused to a numeric rep count — "2.20 Banded Tricept Pull Down"
/// = station 2, 20 reps. Only applied inside an EMOM cycle (where a leading "N." is unambiguously
/// a station index); outside one, a leading `\d+\.\d+` is left intact as a decimal load
/// ("1.5 Turkish Get-Up") — see `strip_interval_label`.
static LIST_MARKER_LOOSE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\d+[.)]\s*(\d.*)$").unwrap());
static REST_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^-\s*Rest\s+(\d+):(\d+)\s*-$").unwrap());
static BARE_REP_SCHEME_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\d+(?:-\d+)+)$").unwrap());
static TEAM_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)team").unwrap());
static WATERFALL_KEYWORD_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)waterfall|amrap").unwrap());

static DISTANCE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^(\d+)\s*m\s*(?:\(\d+ft\))?\s+(.+)").unwrap());
// Check for distance before the amount (e.g. "Run 200 m")
static DISTANCE_SUFFIX_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^(.+?)\s+(\d+)\s*m\s*(?:\(\d+ft\))?\s*$").unwrap());
static CAL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^(\d+)\s*/\s*(\d+)\s+Cal\s+(.+)$").unwrap());
/// A single-number (or rep-range) calorie target — "15 Cal Row", "10-15 Cal Row" — the men's/
/// women's-split form is `CAL_RE`. Same decision: flat 60s/0km, calorie number unused.
static CAL_SINGLE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\d+(?:-\d+)?\s*Cal\s+(.+)$").unwrap());
/// A leading rep range — "10-15 Burpees" (do 10-15 reps). The name is what follows; reps take the
/// low end of the range (the athlete trues up in Hevy). Distinct from `DASH_REP_SERIES_RE`, which
/// is a name *followed* by a set-by-set scheme ("Back Squat 12-10-8").
static RANGE_REPS_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\d+)-\d+\s+(\D.+)$").unwrap());
/// A leading ":SS" seconds duration — ":45 Plank Hold", ":60 KB Farmers Walk". Stripped so the name
/// resolves; the seconds become the set duration.
static LEADING_DURATION_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^:(\d+)\s+(\D.+)$").unwrap());
static XY_REPS_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\d+)\s*/\s*(\d+)\s+(.+)$").unwrap());
/// A per-side rep count: "6/ DB SL RDL" (6 each side). Distinct from `XY_REPS_RE` (men's/women's
/// "12/9") in that nothing follows the slash but the movement name. Logged as a single set of N.
static PER_SIDE_REPS_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\d+)\s*/\s+(\D.+)$").unwrap());
/// A per-side rep count with the side unit glued to the slash — "8/leg KB split RDL", "10/side",
/// "12/arm". The reps (g1) apply per side; the movement (g3) is what resolves. Distinct from
/// `PER_SIDE_REPS_RE` ("6/ DB SL RDL", space after the slash) and `XY_REPS_RE` ("12/9", two numbers).
static PER_SIDE_UNIT_REPS_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^(\d+)\s*/\s*(?:leg|side|arm|ea)\b\s+(\D.+)$").unwrap());
static REPS_PERCENT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^(\d+)\s+(.+?)\s*@\s*(\d+)\s*%$").unwrap());
static DASH_REP_SERIES_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(.+?)\s+(\d+(?:-\d+)+)$").unwrap());
static SET_BY_REP_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^(.+?)\s+(\d+)\s*[xX]\s*(\d+)$").unwrap());
static REPS_FIRST_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\d+)\s+(.+)$").unwrap());
static RPE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\s*@?\s*RPE\s*(\d+(?:\.\d+)?)\s*$").unwrap());
/// A trailing tempo prescription — "@ 3030", "@ 30X0", "@ 2020", "@ 20X1" (eccentric/pause/
/// concentric/pause counts, `X` = explosive). Not stored on the set; stripped so the movement name
/// resolves. Requires the `@` so a real trailing rep count isn't eaten.
static TEMPO_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\s*@\s*[0-9xX]{3,4}\s*$").unwrap());
static WEIGHT_ANNOTATION_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\((\d+(?:\.\d+)?)\s*/\s*(\d+(?:\.\d+)?)\)\s*(lb|kg)?").unwrap());

/// Parses a SugarWOD `description` into structured exercise lines. Mode-aware: tracks whether
/// the current section is a plain line-by-line list, an EMOM interval (enumerated stations are
/// replicated per round = `intervals / stations`), a fixed "N Rounds" / "Every … x N Sets" count,
/// or a team/AMRAP round estimate — see the module-level doc in `mod.rs` and the plan's
/// "Matching pipeline" section for the full rationale.
pub fn parse_description(description: &str) -> Vec<ParsedExerciseLine> {
    let mut mode = LineMode::Default;
    let mut pending_rep_scheme: Option<Vec<i64>> = None;
    let mut lines_out: Vec<ParsedExerciseLine> = Vec::new();
    // The currently-open enumerated-station EMOM section, if any. Finalized (per-round set counts
    // applied) whenever the section ends: a new header, a section-noise boundary, or end of input.
    let mut emom: Option<EmomAccum> = None;
    macro_rules! close_emom {
        () => {
            if let Some(acc) = emom.take() {
                apply_emom_rounds(&mut lines_out, &acc);
            }
        };
    }

    for raw_line in description.replace("\\n", "\n").lines() {
        let trimmed = raw_line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if let Some(caps) = EMOM_RE.captures(trimmed) {
            close_emom!();
            let minutes: u32 = caps[1].parse().unwrap_or(0);
            mode = LineMode::Emom { rounds: minutes };
            emom = Some(EmomAccum {
                intervals: minutes,
                explicit_rounds: None,
                station_count: 0,
                emitted_idxs: Vec::new(),
            });
            continue;
        }

        if let Some(caps) = EMOM_ALT_RE.captures(trimmed) {
            close_emom!();
            let rounds: u32 = caps[1].parse().unwrap_or(0);
            mode = LineMode::Emom { rounds };
            emom = Some(EmomAccum {
                intervals: rounds,
                explicit_rounds: None,
                station_count: 0,
                emitted_idxs: Vec::new(),
            });
            continue;
        }

        if TEAM_RE.is_match(trimmed) && WATERFALL_KEYWORD_RE.is_match(trimmed) {
            close_emom!();
            mode = LineMode::AmrapEstimate {
                rounds: AMRAP_ESTIMATE_DEFAULT_ROUNDS,
            };
            continue;
        }

        if let Some(caps) = EVERY_RE.captures(trimmed) {
            close_emom!();
            let count: u32 = caps[4].parse().unwrap_or(1);
            let unit = caps[5].to_lowercase();
            let rounds = if unit.starts_with("set") || unit.starts_with("round") {
                count
            } else {
                // Total-duration form ("Every 2:30 x 15 Min"): rounds = total / interval.
                let interval_secs = if let (Some(m), Some(s)) = (caps.get(1), caps.get(2)) {
                    m.as_str().parse::<u32>().unwrap_or(0) * 60
                        + s.as_str().parse::<u32>().unwrap_or(0)
                } else if let Some(m) = caps.get(3) {
                    m.as_str().parse::<u32>().unwrap_or(0) * 60
                } else {
                    0
                };
                match (count * 60).checked_div(interval_secs) {
                    // interval_secs == 0: no parseable interval, so the raw count is the best
                    // available round estimate.
                    None => count,
                    Some(intervals) => intervals.max(1),
                }
            };
            if ALTERNATING_STATIONS_RE.is_match(trimmed) {
                // "x N Sets Alternating Stations": N is the TOTAL set count spread across the
                // "Station M:" stations that follow. Open a circuit accum (intervals = N) so the
                // section finalize divides it per station — rounds = N / stations.
                mode = LineMode::Emom { rounds };
                emom = Some(EmomAccum {
                    intervals: rounds,
                    explicit_rounds: None,
                    station_count: 0,
                    emitted_idxs: Vec::new(),
                });
            } else {
                mode = LineMode::Rounds { rounds };
            }
            continue;
        }

        if let Some(caps) = ROUNDS_RE.captures(trimmed) {
            close_emom!();
            let rounds: u32 = caps[1].parse().unwrap_or(1);
            mode = LineMode::Rounds { rounds };
            continue;
        }

        // "N sets of:" — a fixed round count for the movements that follow (like a bare "N Rounds").
        if let Some(caps) = SETS_OF_RE.captures(trimmed) {
            close_emom!();
            let rounds: u32 = caps[1].parse().unwrap_or(1);
            mode = LineMode::Rounds { rounds };
            continue;
        }

        // "30/30 X 7 Rounds" — a work/rest circuit. The movements that follow (numbered "N."
        // stations or a plain list) are each performed for every round. Open a circuit accum with
        // the explicit count so numbered stations (which the mode replication skips) still expand,
        // and set the mode so any plain-listed movements expand too.
        if let Some(caps) = WORKREST_ROUNDS_RE.captures(trimmed) {
            close_emom!();
            let rounds: u32 = caps[1].parse().unwrap_or(1);
            mode = LineMode::Rounds { rounds };
            emom = Some(EmomAccum {
                intervals: 0,
                explicit_rounds: Some(rounds),
                station_count: 0,
                emitted_idxs: Vec::new(),
            });
            continue;
        }

        if AMRAP_RE.is_match(trimmed) || AMRAP_OPEN_RE.is_match(trimmed) {
            close_emom!();
            mode = LineMode::AmrapEstimate {
                rounds: AMRAP_ESTIMATE_DEFAULT_ROUNDS,
            };
            continue;
        }

        // A section-boundary noise line ends the current section: drop it and reset to a plain
        // list so a trailing bare movement isn't replicated by the section it followed.
        if SECTION_NOISE_RE.iter().any(|re| re.is_match(trimmed)) {
            close_emom!();
            mode = LineMode::Default;
            continue;
        }

        // A bare "B. Deadlift"-style movement (a section letter on a real movement — the lettered
        // *headers* were all consumed above). It starts a new lettered section, so end the prior
        // section's round mode, then strip the "B." so the movement resolves. Its own count comes
        // from its own header/scheme (e.g. a following "5-5-5-5-5"), not the previous section.
        let trimmed = if let Some(caps) = SECTION_LETTER_RE.captures(trimmed) {
            close_emom!();
            mode = LineMode::Default;
            caps.get(1).unwrap().as_str().trim()
        } else {
            trimmed
        };

        // Strip a leading "Minute N:" / "Min N-" station label before any further parsing.
        // Such lines enumerate distinct EMOM stations, so they must not trigger the
        // whole-section set replication below (tracked via `from_interval_label`).
        let (content, from_interval_label) = strip_interval_label(trimmed, emom.is_some());

        // A labeled line is one EMOM station in the open cycle — count it (including a "Rest"
        // minute that gets dropped just below) so the cycle length is known at section end.
        if from_interval_label {
            if let Some(acc) = emom.as_mut() {
                acc.station_count += 1;
            }
        }

        if let Some(caps) = REST_RE.captures(content) {
            let minutes: i64 = caps[1].parse().unwrap_or(0);
            let seconds: i64 = caps[2].parse().unwrap_or(0);
            if let Some(last) = lines_out.last_mut() {
                last.rest_seconds_after = Some(minutes * 60 + seconds);
            }
            continue;
        }

        if let Some(caps) = BARE_REP_SCHEME_RE.captures(content) {
            let scheme: Vec<i64> = caps[1].split('-').filter_map(|s| s.parse().ok()).collect();
            // A bare scheme after a movement ("B. Deadlift" \n "5-5-5-5-5") describes *that*
            // movement: attach it to the preceding bare single-set line. Otherwise it's a leading
            // header ("21-15-9" \n "Pull-ups") — keep it pending for the next bare line.
            match lines_out.last_mut() {
                Some(prev) if is_bare_sets(prev) => {
                    prev.sets = scheme
                        .into_iter()
                        .map(|r| SetSpec {
                            reps: Some(r),
                            ..Default::default()
                        })
                        .collect();
                }
                _ => pending_rep_scheme = Some(scheme),
            }
            continue;
        }

        if SKIP_PATTERNS.iter().any(|re| re.is_match(content)) {
            continue;
        }

        if let Some(mut line) = parse_exercise_line(content) {
            if is_bare_sets(&line) {
                if let Some(scheme) = pending_rep_scheme.take() {
                    line.sets = scheme
                        .into_iter()
                        .map(|r| SetSpec {
                            reps: Some(r),
                            ..Default::default()
                        })
                        .collect();
                }
            }

            if let LineMode::Emom { rounds }
            | LineMode::AmrapEstimate { rounds }
            | LineMode::Rounds { rounds } = mode
            {
                if !from_interval_label && line.sets.len() == 1 {
                    let base = line.sets[0].clone();
                    line.sets = std::iter::repeat_n(base, rounds.max(1) as usize).collect();
                }
            }

            // An emitted EMOM station: remember it so its per-round set count can be applied when
            // the cycle length is known (see `apply_emom_rounds`).
            let idx = lines_out.len();
            lines_out.push(line);
            if from_interval_label {
                if let Some(acc) = emom.as_mut() {
                    acc.emitted_idxs.push(idx);
                }
            }
        }
    }

    close_emom!();
    lines_out
}

/// True if the line's single set has no numeric structure at all (a "bare" fallback line,
/// e.g. a plain "Pull-ups" under a pending "21-15-9" header) — the only case a pending bare
/// rep-scheme header should be applied to.
fn is_bare_sets(line: &ParsedExerciseLine) -> bool {
    line.sets.len() == 1 && line.sets[0] == SetSpec::default()
}

/// Strips a leading interval/station label ("Minute 3:", "Min 3-", "Odd:") or numbered-list marker
/// if present, returning the remaining text and whether a label was removed. Inside an EMOM cycle
/// (`in_emom`), also strips a marker fused to its reps ("2.20 …"); outside one, a leading decimal is
/// preserved. See `INTERVAL_LABEL_RE` / `LIST_MARKER_RE` / `LIST_MARKER_LOOSE_RE`.
fn strip_interval_label(line: &str, in_emom: bool) -> (&str, bool) {
    if let Some(m) = INTERVAL_LABEL_RE.find(line) {
        return (line[m.end()..].trim(), true);
    }
    if let Some(caps) = LIST_MARKER_RE.captures(line) {
        return (caps.get(1).unwrap().as_str().trim(), true);
    }
    if in_emom {
        if let Some(caps) = LIST_MARKER_LOOSE_RE.captures(line) {
            return (caps.get(1).unwrap().as_str().trim(), true);
        }
    }
    (line, false)
}

fn parse_exercise_line(trimmed: &str) -> Option<ParsedExerciseLine> {
    let (text, rpe_note) = extract_rpe(trimmed);
    let (text, explicit_weight_lb) = extract_weight_annotation(&text);
    // Drop a trailing tempo code ("@ 3030") — informational, not stored, and would pollute the name.
    let text = TEMPO_RE.replace(&text, "").to_string();
    let text = text.trim().to_string();

    let mut set = SetSpec {
        explicit_weight_lb,
        ..Default::default()
    };

    if let Some(caps) = LEADING_DURATION_RE.captures(&text) {
        set.duration_seconds = caps[1].parse().ok();
        return Some(ParsedExerciseLine {
            name_text: caps[2].trim().to_string(),
            sets: vec![set],
            rpe_note,
            rest_seconds_after: None,
        });
    }

    if let Some(caps) = DISTANCE_RE.captures(&text) {
        set.distance_meters = caps[1].parse().ok();
        return Some(ParsedExerciseLine {
            name_text: caps[2].trim().to_string(),
            sets: vec![set],
            rpe_note,
            rest_seconds_after: None,
        });
    }

    if let Some(caps) = DISTANCE_SUFFIX_RE.captures(&text) {
        set.distance_meters = caps[2].parse().ok();
        return Some(ParsedExerciseLine {
            name_text: caps[1].trim().to_string(),
            sets: vec![set],
            rpe_note,
            rest_seconds_after: None,
        });
    }

    if let Some(caps) = CAL_RE.captures(&text) {
        // Decision: calorie-target intervals always log as a flat 60s/0km activity,
        // regardless of the calorie number (the men's/first value is intentionally unused).
        set.duration_seconds = Some(60);
        set.distance_meters = Some(0);
        return Some(ParsedExerciseLine {
            name_text: caps[3].trim().to_string(),
            sets: vec![set],
            rpe_note,
            rest_seconds_after: None,
        });
    }

    if let Some(caps) = CAL_SINGLE_RE.captures(&text) {
        // Single-number/rep-range calorie target ("15 Cal Row") — same flat 60s/0km as CAL_RE.
        set.duration_seconds = Some(60);
        set.distance_meters = Some(0);
        return Some(ParsedExerciseLine {
            name_text: caps[1].trim().to_string(),
            sets: vec![set],
            rpe_note,
            rest_seconds_after: None,
        });
    }

    if let Some(caps) = REPS_PERCENT_RE.captures(&text) {
        set.reps = caps[1].parse().ok();
        set.percent_1rm = caps[3].parse::<f64>().ok().map(|p| p / 100.0);
        return Some(ParsedExerciseLine {
            name_text: caps[2].trim().to_string(),
            sets: vec![set],
            rpe_note,
            rest_seconds_after: None,
        });
    }

    if let Some(caps) = PER_SIDE_UNIT_REPS_RE.captures(&text) {
        // "8/leg KB split RDL" — reps per side; keep the name clean so it resolves.
        set.reps = caps[1].parse().ok();
        return Some(ParsedExerciseLine {
            name_text: caps[2].trim().to_string(),
            sets: vec![set],
            rpe_note,
            rest_seconds_after: None,
        });
    }

    if let Some(caps) = XY_REPS_RE.captures(&text) {
        set.reps = caps[1].parse().ok();
        return Some(ParsedExerciseLine {
            name_text: caps[3].trim().to_string(),
            sets: vec![set],
            rpe_note,
            rest_seconds_after: None,
        });
    }

    if let Some(caps) = PER_SIDE_REPS_RE.captures(&text) {
        // "6/ DB SL RDL" — reps-per-side; the unilateral nuance is left for the user to adjust
        // in Hevy, but the name is kept clean so it can resolve.
        set.reps = caps[1].parse().ok();
        return Some(ParsedExerciseLine {
            name_text: caps[2].trim().to_string(),
            sets: vec![set],
            rpe_note,
            rest_seconds_after: None,
        });
    }

    if let Some(caps) = DASH_REP_SERIES_RE.captures(&text) {
        let name = caps[1].trim().to_string();
        let sets: Vec<SetSpec> = caps[2]
            .split('-')
            .filter_map(|s| s.parse::<i64>().ok())
            .map(|r| SetSpec {
                reps: Some(r),
                explicit_weight_lb,
                ..Default::default()
            })
            .collect();
        return Some(ParsedExerciseLine {
            name_text: name,
            sets,
            rpe_note,
            rest_seconds_after: None,
        });
    }

    if let Some(caps) = SET_BY_REP_RE.captures(&text) {
        let name = caps[1].trim().to_string();
        let num_sets: usize = caps[2].parse().unwrap_or(1);
        let reps: i64 = caps[3].parse().unwrap_or(0);
        let sets = std::iter::repeat_n(
            SetSpec {
                reps: Some(reps),
                explicit_weight_lb,
                ..Default::default()
            },
            num_sets.max(1),
        )
        .collect();
        return Some(ParsedExerciseLine {
            name_text: name,
            sets,
            rpe_note,
            rest_seconds_after: None,
        });
    }

    if let Some(caps) = RANGE_REPS_RE.captures(&text) {
        // "10-15 Burpees" — leading rep range; reps take the low end, name is the remainder.
        set.reps = caps[1].parse().ok();
        return Some(ParsedExerciseLine {
            name_text: caps[2].trim().to_string(),
            sets: vec![set],
            rpe_note,
            rest_seconds_after: None,
        });
    }

    if let Some(caps) = REPS_FIRST_RE.captures(&text) {
        set.reps = caps[1].parse().ok();
        return Some(ParsedExerciseLine {
            name_text: caps[2].trim().to_string(),
            sets: vec![set],
            rpe_note,
            rest_seconds_after: None,
        });
    }

    if text.is_empty() {
        return None;
    }

    // Bare fallback: no numeric structure at all. Still captured rather than dropped — a
    // movement with no parseable set/rep info is better surfaced as an unloaded exercise than
    // silently missing from the routine. May be filled in later by a pending rep-scheme header.
    Some(ParsedExerciseLine {
        name_text: text,
        sets: vec![set],
        rpe_note,
        rest_seconds_after: None,
    })
}

fn extract_rpe(line: &str) -> (String, Option<String>) {
    if let Some(caps) = RPE_RE.captures(line) {
        let whole = caps.get(0).unwrap();
        let note = format!("@ RPE {}", &caps[1]);
        (line[..whole.start()].to_string(), Some(note))
    } else {
        (line.to_string(), None)
    }
}

fn extract_weight_annotation(line: &str) -> (String, Option<f64>) {
    if let Some(caps) = WEIGHT_ANNOTATION_RE.captures(line) {
        let value_a: f64 = caps[1].parse().unwrap_or(0.0);
        let unit = caps.get(3).map(|m| m.as_str());
        let lb = if unit == Some("kg") {
            crate::units::kg_to_lb(value_a)
        } else {
            value_a
        };
        let whole = caps.get(0).unwrap();
        let cleaned = format!("{}{}", &line[..whole.start()], &line[whole.end()..]);
        (cleaned, Some(lb))
    } else {
        (line.to_string(), None)
    }
}

/// Parser tests, driven almost entirely by real WOD text captured from SugarWOD rather than
/// invented examples. That is deliberate: the whole difficulty of this parser is that coaches
/// type free-form prose, so the failures that matter are the ones nobody would think to invent.
///
/// **Vocabulary**, since the formats make no sense without it:
///
/// - **WOD** — "Workout of the Day", the whole day's programming for one *track*.
/// - **Track** — a named variant of the day's workout ("Fitness", "Performance", "HYROX").
///   A single day commonly has two or three; each becomes its own Hevy routine.
/// - **EMOM** — "Every Minute On the Minute": one station per minute, cycling. `35:00 EMOM`
///   over 5 stations means each station is performed 7 times (35 / 5), *not* 35 times. Getting
///   this division wrong is the single most common bug in here.
/// - **AMRAP** — "As Many Rounds As Possible" within a time window. The true round count depends
///   on how fast the athlete moves, so it cannot be derived — we estimate it
///   ([`AMRAP_ESTIMATE_DEFAULT_ROUNDS`]) and let the athlete adjust in Hevy.
/// - **"Every N x M"** — start a new set every N (minutes or mm:ss), for M sets or M total
///   minutes. M is sometimes a set count and sometimes a duration to divide; both spellings
///   appear in real programming.
/// - **Rx notation** — `12/9` is a men's/women's split (take the first number); `6/` means
///   "6 per side"; `@ 75%` is a percentage of one-rep max; `@ RPE 8` is a subjective effort
///   rating; `(95/65)` is an explicit load in pounds.
/// - **Cal** — a calorie target on a machine (row/bike/ski). Treated as a flat 60-second effort
///   rather than a rep count, since Hevy has no calorie field.
///
/// Movement names themselves (wall balls, toes to bar, KBS, singles) matter only as text to be
/// resolved against Hevy's catalog — the parser never needs to know what they are.
///
/// Most tests are named for the *spelling* they pin down, because the recurring problem is that
/// one idea has many spellings: `EMOM X 18`, `18:00 EMOM`, and `A. EMOM X 18 Min` are the same
/// instruction written three ways, and each one broke at some point.
#[cfg(test)]
mod tests {
    use super::*;

    /// Parses a description into exercise lines. Thin wrapper over [`parse_description`] purely
    /// to keep the test bodies short.
    fn only(desc: &str) -> Vec<ParsedExerciseLine> {
        parse_description(desc)
    }

    // Two EMOM blocks, kept verbatim because they pack more competing formats into a few lines
    // than anything else here. Shared as constants so the two tests below assert on the same
    // bytes.
    //
    // Why a leftover prefix matters more than it looks: a label left on the front of a name
    // ("minute 1 db reverse lunges") matches nothing in Hevy's catalog, so that movement is
    // dropped. Drop every movement in a workout and the routine is built with an empty
    // `exercises` array, which Hevy rejects with 400 "Array must contain at least 1 element(s)".
    // A prefix that survives parsing therefore fails the whole sync, not just one line.
    const EMOM_WITH_MINUTE_LABELS: &str = "35:00 EMOM\nMinute 1: DB Reverse Lunges \nMinute 2: Air Squat \nMinute 3: V-Ups\nMinute 4: 12/9 Cal Row \nMinute 5: Rest";
    const EVERY_THEN_EMOM_WITH_MIN_LABELS: &str = "A. Every 2:30 x 4 Sets\n10 Alt Back Rack Reverse Lunges\n* building to a moderate-heavy load\n\nB.EMOM X 18\nMin 1- 6/ DB SL RDL\nMin 2- 6 1 1/2 Goblet Squat\nMin 3- 12 DB Sumo Squat";

    /// Distance-based movements ("run 400 metres") carry a distance, not a rep count, and the
    /// distance can be written on either side of the movement name. Every spelling has to reduce
    /// to a bare name, because only the bare name matches Hevy's catalog.
    #[test]
    fn distance_is_stripped_from_the_name_in_every_spelling() {
        for (desc, name, meters, spelling) in [
            ("400m Run", "Run", 400, "distance first, no space"),
            (
                "1000 m run",
                "run",
                1000,
                "distance first, spaced and lowercase",
            ),
            (
                "50m (165ft) Burpee Broad Jumps",
                "Burpee Broad Jumps",
                50,
                "distance first with a feet conversion in brackets",
            ),
            ("Run 200 m", "Run", 200, "distance last"),
        ] {
            let lines = only(desc);
            assert_eq!(lines.len(), 1, "{spelling}: {desc:?} is one movement");
            assert_eq!(lines[0].name_text, name, "{spelling}: {desc:?}");
            assert_eq!(
                lines[0].sets,
                vec![SetSpec {
                    distance_meters: Some(meters),
                    ..Default::default()
                }],
                "{spelling}: {desc:?} should record only a distance"
            );
        }
    }

    /// "Every <interval> x <count>" starts a set on a fixed interval. The trailing number is
    /// sometimes a set count and sometimes a total duration to divide by the interval, and the
    /// two are only distinguishable by the unit — so all four spellings are pinned together.
    /// The header itself is never an exercise.
    #[test]
    fn every_interval_headers_drive_set_count() {
        for (desc, sets, why) in [
            (
                "A. Every 1:30 x 8 Sets\n3 Hang Power Clean @ 75%",
                8,
                "explicit set count",
            ),
            (
                "A. Every 2 Min x 14 Min\n5 Push Press @ 75%",
                7,
                "total 14 min / 2 min interval",
            ),
            (
                "A. Every 2:30 x 15 Min\n5 Back Squat @ 75%",
                6,
                "total 900s / 150s interval",
            ),
            (
                "B. Every 3:00 x 3 Rounds\n500m Row",
                3,
                "explicit round count",
            ),
        ] {
            let lines = only(desc);
            assert_eq!(
                lines.len(),
                1,
                "{why}: the header must not become an exercise"
            );
            assert_eq!(lines[0].sets.len(), sets, "{why}: {desc:?}");
        }
    }

    /// How many rounds an AMRAP yields depends on the athlete, so it cannot be derived from the
    /// text — every spelling of the header resolves to the same estimate, and none of them is an
    /// exercise. The athlete trues the count up in Hevy.
    #[test]
    fn amrap_headers_apply_the_round_estimate_in_every_spelling() {
        for (desc, names, spelling) in [
            (
                "35 Min AMRAP\n30 DB Alt Snatch\n200 m Run\n30 Burpees",
                vec!["DB Alt Snatch", "Run", "Burpees"],
                "\"N Min AMRAP\"",
            ),
            (
                "B. 18 Min AMRAP\n20 DB Rows\n20 DB Push Press",
                vec!["DB Rows", "DB Push Press"],
                "section-lettered \"B. N Min AMRAP\"",
            ),
            (
                "B. 20:00 AMRAP\n2 Wall Walks\n16 Wall Balls",
                vec!["Wall Walks", "Wall Balls"],
                "clock-style \"B. MM:SS AMRAP\"",
            ),
        ] {
            let lines = only(desc);
            let got: Vec<&str> = lines.iter().map(|l| l.name_text.as_str()).collect();
            assert_eq!(got, names, "{spelling}: header must not become an exercise");
            assert!(
                lines
                    .iter()
                    .all(|l| l.sets.len() == AMRAP_ESTIMATE_DEFAULT_ROUNDS as usize),
                "{spelling}: every movement gets the estimated round count"
            );
        }
    }

    /// A movement is nearly always preceded by its prescription, in one of several notations.
    /// All of them have to come off the front: the name is the only part that matches Hevy's
    /// catalog, so anything left glued to it makes the movement unresolvable and it silently
    /// disappears from the routine.
    #[test]
    fn leading_prescriptions_are_stripped_leaving_a_clean_name() {
        for (desc, name, reps, seconds, notation) in [
            (
                "40 Toes to Bar",
                "Toes to Bar",
                Some(40),
                None,
                "plain rep count",
            ),
            (
                "10-15 Burpees",
                "Burpees",
                Some(10),
                None,
                "rep range, take the low end",
            ),
            (
                "6/ DB SL RDL",
                "DB SL RDL",
                Some(6),
                None,
                "\"N/\" meaning N per side",
            ),
            (
                ":45 Plank Hold",
                "Plank Hold",
                None,
                Some(45),
                "\":SS\" hold duration",
            ),
        ] {
            let lines = only(desc);
            assert_eq!(lines.len(), 1, "{notation}: {desc:?}");
            assert_eq!(lines[0].name_text, name, "{notation}: {desc:?}");
            assert_eq!(lines[0].sets[0].reps, reps, "{notation}: reps for {desc:?}");
            assert_eq!(
                lines[0].sets[0].duration_seconds, seconds,
                "{notation}: duration for {desc:?}"
            );
        }
    }

    #[test]
    fn dash_rep_series() {
        // A descending ladder written after the movement: six sets of 12, 10, 8, 6, 4, 2 reps.
        // Each number is its own set, so the set count comes from the series length.
        let lines = only("Back Squat 12-10-8-6-4-2");
        assert_eq!(lines[0].name_text, "Back Squat");
        let reps: Vec<Option<i64>> = lines[0].sets.iter().map(|s| s.reps).collect();
        assert_eq!(
            reps,
            vec![Some(12), Some(10), Some(8), Some(6), Some(4), Some(2)]
        );
    }

    #[test]
    fn bare_rep_scheme_header_applies_to_next_bare_line() {
        // "21-15-9" is a classic CrossFit rep ladder written on its own line above the movement
        // it applies to. It is a header, not an exercise, and gives the following line 3 sets.
        let lines = only("21-15-9\nPull-ups");
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].name_text, "Pull-ups");
        let reps: Vec<Option<i64>> = lines[0].sets.iter().map(|s| s.reps).collect();
        assert_eq!(reps, vec![Some(21), Some(15), Some(9)]);
    }

    #[test]
    fn set_by_rep_notation() {
        // "5x5" is sets-by-reps: five sets of five, not one set of 25.
        let lines = only("Bench Press 5x5");
        assert_eq!(lines[0].name_text, "Bench Press");
        assert_eq!(lines[0].sets.len(), 5);
        assert!(lines[0].sets.iter().all(|s| s.reps == Some(5)));
    }

    #[test]
    fn weight_annotation_extracted() {
        // "(95/65)" is a prescribed load in pounds, men's/women's. Take the first number as the
        // explicit weight; it overrides any percentage-of-max calculation later in the pipeline.
        let lines = only("Back Squat (95/65)");
        assert_eq!(lines[0].name_text, "Back Squat");
        assert_eq!(lines[0].sets[0].explicit_weight_lb, Some(95.0));
    }

    #[test]
    fn rpe_suffix_becomes_note_not_numeric_field() {
        // RPE ("rate of perceived exertion") is how hard a set should feel, 1-10. It is a
        // judgement call, not a load, so it is preserved verbatim as a note for the athlete to
        // read rather than converted into a weight the parser would only be guessing at.
        let lines = only("Back Squat 5x5 @ RPE 8");
        assert_eq!(lines[0].name_text, "Back Squat");
        assert_eq!(lines[0].rpe_note.as_deref(), Some("@ RPE 8"));
        assert_eq!(lines[0].sets.len(), 5);
    }

    #[test]
    fn emom_header_replicates_sets_and_rest_marker_attaches_to_previous() {
        // Two back-to-back 5-minute EMOMs with a rest between them. Each EMOM header replicates
        // the movement under it to 5 sets, and the "-Rest 2:00-" marker belongs to the movement
        // *before* it, not the one after — rest is something you do after finishing a block.
        let lines =
            only("A.5:00 EMOM\n2 Push Press @ 65%\n-Rest 2:00-\n5:00 EMOM\n2 Push Jerks @ 65%");
        assert_eq!(lines.len(), 2);

        assert_eq!(lines[0].name_text, "Push Press");
        assert_eq!(lines[0].sets.len(), 5);
        assert!(lines[0]
            .sets
            .iter()
            .all(|s| s.reps == Some(2) && s.percent_1rm == Some(0.65)));
        assert_eq!(lines[0].rest_seconds_after, Some(120));

        assert_eq!(lines[1].name_text, "Push Jerks");
        assert_eq!(lines[1].sets.len(), 5);
    }

    #[test]
    fn team_waterfall_sets_amrap_estimate_mode_and_time_cap_is_noise() {
        // A "waterfall" is a team format where athletes start staggered. It changes who is
        // working when, but not what any one athlete does, so it is treated as an AMRAP: the
        // header lines are noise and the movements get the estimated round count.
        let lines = only("B. 3-Person Team Waterfall\n24:00 AMRAP\n12 Alt DB Hang Snatch");
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].name_text, "Alt DB Hang Snatch");
        assert_eq!(lines[0].sets.len(), 3);
        assert!(lines[0].sets.iter().all(|s| s.reps == Some(12)));
    }

    #[test]
    fn cal_target_is_flat_sixty_seconds_regardless_of_number() {
        // Calories on a machine are a work target, and Hevy has no calorie field. Rather than
        // mislabel them as reps, every calorie target becomes a flat 60-second effort — the
        // number is deliberately ignored, since 12 cal and 40 cal are both "row until done".
        let lines = only("B. 3-Person Team Waterfall\n24:00 AMRAP\n12/9 Cal Row\n12/9 Cal  Bike");
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].name_text, "Row");
        assert_eq!(lines[1].name_text, "Bike");
        for line in &lines {
            assert_eq!(line.sets.len(), 3);
            for s in &line.sets {
                assert_eq!(s.duration_seconds, Some(60));
                assert_eq!(s.distance_meters, Some(0));
            }
        }
    }

    #[test]
    fn order_by_gender() {
        // "12/9" is a men's/women's prescription, not a fraction or a range. Take the first
        // number; the athlete adjusts in Hevy if they want the other.
        let lines = only("B. 3-Person Team Waterfall\n24:00 AMRAP\n12/9 Push-Ups");
        assert_eq!(lines[0].name_text, "Push-Ups");
        assert_eq!(lines[0].sets.len(), 3);
        assert!(lines[0].sets.iter().all(|s| s.reps == Some(12)));
    }

    #[test]
    fn emom_minute_station_labels_stripped_and_replicated_per_round() {
        // Four formats in one block, all of which have to be handled for any of it to work:
        //   1. "Minute N:" labels come off the names, or nothing resolves.
        //   2. The "Rest" minute is not a movement, but still counts toward the cycle length.
        //   3. An embedded "12/9 Cal Row" reduces to "Row".
        //   4. Rounds = 35 minutes / 5 stations = 7 — not 35 (once per minute), and not 1
        //      (which is what you get if the header is ignored entirely).
        let lines = only(EMOM_WITH_MINUTE_LABELS);
        let names: Vec<&str> = lines.iter().map(|l| l.name_text.as_str()).collect();
        assert_eq!(
            names,
            vec!["DB Reverse Lunges", "Air Squat", "V-Ups", "Row"]
        );
        assert!(
            lines.iter().all(|l| l.sets.len() == 7),
            "35 min / 5 stations = 7 rounds each"
        );
        assert!(lines
            .iter()
            .all(|l| !l.name_text.to_lowercase().contains("minute")));
    }

    #[test]
    fn emom_x_n_header_and_min_dash_labels_parse() {
        // A section that switches format halfway through — an interval block, then an EMOM:
        //   - "B.EMOM X 18" is a header, consumed rather than emitted as an exercise.
        //   - "Min N-" labels come off, but the reps after them are kept.
        //   - "* building to a ..." is a coaching note, not a movement.
        //   - "A. Every 2:30 x 4 Sets" sets the round count (4) for the movement under it.
        //   - "EMOM X 18" over its 3 stations = 6 rounds each.
        let lines = only(EVERY_THEN_EMOM_WITH_MIN_LABELS);
        let names: Vec<&str> = lines.iter().map(|l| l.name_text.as_str()).collect();
        assert!(names.contains(&"Alt Back Rack Reverse Lunges"));
        assert!(names.contains(&"DB Sumo Squat"));
        for name in &names {
            let n = name.to_lowercase();
            assert!(
                !n.contains("emom") && !n.starts_with('*') && !n.starts_with("min "),
                "{name:?} still carries a header or interval label — the exact pollution that \
                 made this WOD unresolvable"
            );
        }
        let lunges = lines
            .iter()
            .find(|l| l.name_text == "Alt Back Rack Reverse Lunges")
            .unwrap();
        assert_eq!(lunges.sets.len(), 4, "\"Every 2:30 x 4 Sets\" -> 4 sets");
        let sumo = lines
            .iter()
            .find(|l| l.name_text == "DB Sumo Squat")
            .unwrap();
        assert_eq!(sumo.sets[0].reps, Some(12));
        // "EMOM X 18" over its 3-station cycle = 18 / 3 = 6 rounds each.
        assert_eq!(sumo.sets.len(), 6, "EMOM X 18 / 3 stations = 6 rounds");
    }

    #[test]
    fn bare_n_rounds_header_replicates_following_movements() {
        // A bare "N Rounds" line is a header that applies to every movement under it, not a
        // movement itself. Each of the three gets 5 sets; the header emits nothing.
        let lines = only("5 Rounds\n12 Deadlifts\n9 Hang Power Cleans\n6 Push Jerks");
        let names: Vec<&str> = lines.iter().map(|l| l.name_text.as_str()).collect();
        assert_eq!(names, vec!["Deadlifts", "Hang Power Cleans", "Push Jerks"]);
        assert!(
            lines.iter().all(|l| l.sets.len() == 5),
            "each movement gets 5 sets"
        );
        assert_eq!(lines[0].sets[0].reps, Some(12));
    }

    #[test]
    fn emom_x_n_enumerated_stations_replicate_per_round() {
        // The "Min N-" spelling of station labels, and the rule that Rest counts toward the
        // cycle length without being emitted: 35 minutes / 5 stations = 7 rounds for each of
        // the 4 real movements. Miss the Rest station and the division gives 8.
        let desc = "EMOM X 35\nMin 1- 20 Sit Ups\nMin 2- 75 Singles\nMin 3- 30 Plank Shoulder Taps\nMin 4- Max DB Snatch\nMin 5- Rest";
        let lines = only(desc);
        let names: Vec<&str> = lines.iter().map(|l| l.name_text.as_str()).collect();
        assert_eq!(
            names,
            vec!["Sit Ups", "Singles", "Plank Shoulder Taps", "Max DB Snatch"]
        );
        assert!(
            lines.iter().all(|l| l.sets.len() == 7),
            "35 min / 5 stations = 7 rounds each"
        );
        assert_eq!(lines[0].sets[0].reps, Some(20));
    }

    #[test]
    fn for_time_cap_header_is_noise_and_resets_prior_rounds_mode() {
        // "B.For Time - 12 Min Cap" is a section header, not an exercise, and it ends the
        // previous section's round mode so the next section starts clean.
        let lines = only(
            "A. Every 1:30 x 8 Sets\n3 Hang Power Clean @ 75%\n\nB.For Time - 12 Min Cap\n5 Rounds\n12 Deadlifts",
        );
        let names: Vec<&str> = lines.iter().map(|l| l.name_text.as_str()).collect();
        assert_eq!(names, vec!["Hang Power Clean", "Deadlifts"]);
        assert_eq!(lines[0].sets.len(), 8, "Part A: Every x 8 Sets");
        assert_eq!(lines[1].sets.len(), 5, "Part B: 5 Rounds");
    }

    #[test]
    fn optional_finisher_noise_lines_are_dropped() {
        // An optional finisher tacked onto the end of a rounds block. Two things to get right:
        // the "if done before optional:" and "20/60 x Remaining time" lines are noise, and the
        // movement after them must NOT inherit the earlier "5 Rounds" — the round mode ends
        // with its block, so the finisher is a single set.
        let desc = "5 Rounds\n12 Deadlifts\n\nif done before optional: \n20/60 x Remaining time \nBike Effort";
        let lines = only(desc);
        let names: Vec<&str> = lines.iter().map(|l| l.name_text.as_str()).collect();
        assert_eq!(names, vec!["Deadlifts", "Bike Effort"]);
        for junk in ["Rounds", "Remaining", "optional"] {
            assert!(
                !names.iter().any(|n| n.contains(junk)),
                "{junk:?} should be dropped as noise"
            );
        }
        let bike = lines.iter().find(|l| l.name_text == "Bike Effort").unwrap();
        assert_eq!(
            bike.sets.len(),
            1,
            "optional finisher is not round-replicated"
        );
    }

    #[test]
    fn gym_announcement_lines_are_not_exercises() {
        // Gyms post closure notices and class times through the same field as workouts. None of
        // it is a movement, so the whole input must yield nothing rather than junk exercises.
        let lines = only("CLUB HOURS: 7A-7PM\nCLASS OFFERINGS 9AM & 4PM");
        assert!(lines.is_empty());
    }

    #[test]
    fn numbered_list_stations_stripped_and_workrest_rounds_applied() {
        // Stations written as a numbered list. The "N." markers come off so the names resolve,
        // and "30/30 X 7 Rounds" (30s work / 30s rest, 7 times through) gives every station 7
        // sets — the rounds apply to each station, not split across them.
        let desc = "30/30 X 7 Rounds \n1. DB Pullover\n2. Medball V-Up\n3. Jump Rope\n4. DB Floor Press\n5. DB Curls";
        let lines = only(desc);
        let names: Vec<&str> = lines.iter().map(|l| l.name_text.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "DB Pullover",
                "Medball V-Up",
                "Jump Rope",
                "DB Floor Press",
                "DB Curls"
            ]
        );
        assert!(
            lines.iter().all(|l| l.sets.len() == 7),
            "\"30/30 X 7 Rounds\" -> 7 sets per station"
        );
    }

    #[test]
    fn workrest_rounds_header_applies_to_plain_movement_list() {
        // Same work/rest round count, but movements listed plainly (no "N." markers) — still 4
        // sets each, and the header itself is not emitted as an exercise.
        let lines = only("40/20 x 4 Rounds\nRow\nWall Balls");
        let names: Vec<&str> = lines.iter().map(|l| l.name_text.as_str()).collect();
        assert_eq!(names, vec!["Row", "Wall Balls"]);
        assert!(lines.iter().all(|l| l.sets.len() == 4));
    }

    #[test]
    fn decimal_leading_number_is_not_mistaken_for_a_list_marker() {
        // "1.5 Turkish Get-Up" must keep its "1.5" (a load/qualifier), not be split into "5 …".
        let lines = only("1.5 Turkish Get-Up");
        assert_eq!(lines[0].name_text, "1.5 Turkish Get-Up");
    }

    #[test]
    fn skip_lines_produce_no_exercises() {
        // Preamble, prose, and equipment lists are not movements. Emitting any of them would put
        // a junk entry in the athlete's Hevy routine, so the whole input must yield nothing.
        let lines = only("Hyrox SIM\nThis is the Hyrox Workout. You can do this solo, partner, relay, or half it all! Its your workout!\nA For Time:\nDumbbells: 2 x 50/35lb, 22.5/15kg");
        assert!(lines.is_empty());
    }

    #[test]
    fn full_hyrox_fixture_matches_expected_exercise_count() {
        // A full real workout end to end, as a backstop on the single-behaviour tests above:
        // the repeated runs must each survive as their own line rather than being deduplicated
        // or collapsed, and the prose header must not leak in as an exercise.
        let desc = "Hyrox SIM\nThis is the Hyrox Workout. You can do this solo, partner, relay, or half it all! Its your workout!\n\n1000 m Run\n1000 m SKI\n1000 m Run\n50 m Sled push\n1000 m Run\n50 m Sled pull\n1000 m Run\n80 m Burpee Broad jumps\n1000 m run\n1000 m Row\n1000 m run\n200 m Farmer carry\n1000 m run\n100 m sandbag lunges\n1000 m run\n100 Wall balls";
        let lines = only(desc);
        // 9 "1000 m run/Run" lines + SKI + Sled push + Sled pull + Burpee Broad jumps + Row +
        // Farmer carry + sandbag lunges + Wall balls = 16 total exercise lines.
        assert_eq!(lines.len(), 16);
        assert_eq!(lines.last().unwrap().name_text, "Wall balls");
        assert_eq!(lines.last().unwrap().sets[0].reps, Some(100));
    }

    #[test]
    fn emom_x_n_header_tolerates_trailing_min_unit() {
        // "EMOM X 35 MIN" must be recognized despite the trailing unit (5-station cycle -> 7).
        let desc = "EMOM X 35 MIN\nMin 1- 10 Push Up\nMin 2- 20 Sit Up\nMin 3- 50 Singles\nMin 4- 12 KBS\nMin 5- Rest";
        let lines = only(desc);
        let names: Vec<&str> = lines.iter().map(|l| l.name_text.as_str()).collect();
        assert_eq!(names, vec!["Push Up", "Sit Up", "Singles", "KBS"]);
        assert!(
            lines.iter().all(|l| l.sets.len() == 7),
            "35 / 5 stations = 7"
        );
    }

    #[test]
    fn emom_numbered_station_fused_to_reps_is_a_station_not_a_decimal() {
        // A typo'd station marker: "2.20 Banded Tricept Pull Down" is station 2 with 20 reps,
        // written without the space. Read as the decimal 2.20 instead, the line stops counting
        // as a station, the cycle collapses to 4, and the round count comes out wrong.
        let desc = "EMOM X 35 MIN\n1. 10 Push Up\n2.20 Banded Tricept Pull Down\n3. 50 Singles\n4. 12 AKBS\n5. Rest";
        let lines = only(desc);
        let names: Vec<&str> = lines.iter().map(|l| l.name_text.as_str()).collect();
        assert_eq!(
            names,
            vec!["Push Up", "Banded Tricept Pull Down", "Singles", "AKBS"]
        );
        assert!(
            lines.iter().all(|l| l.sets.len() == 7),
            "5-station cycle -> 7"
        );
        assert_eq!(lines[1].sets[0].reps, Some(20));
    }

    #[test]
    fn quoted_workout_name_is_not_an_exercise() {
        // Benchmark workouts have names, and coaches quote them ("Vaya Con Dios", "Fran").
        // A quoted line is a title, never a movement.
        let lines = only("B. 20:00 AMRAP\n\"Vaya Con Dios\"\n2 Wall Walks\n16 Wall Balls");
        let names: Vec<&str> = lines.iter().map(|l| l.name_text.as_str()).collect();
        assert_eq!(names, vec!["Wall Walks", "Wall Balls"]);
    }

    #[test]
    fn min_to_complete_and_trailing_set_prescription_are_noise() {
        // "A2. 10 Min to Complete" is a section header, and a bare "5/5 x 4 Sets @ 40%" prescription
        // line is dropped (not a junk exercise). The movement between them is kept.
        let lines =
            only("A2. 10 Min to Complete\nBack Rack Bulgarian Split Squats\n5/5 x 4 Sets @ 40%");
        let names: Vec<&str> = lines.iter().map(|l| l.name_text.as_str()).collect();
        assert_eq!(names, vec!["Back Rack Bulgarian Split Squats"]);
    }

    #[test]
    fn calorie_movement_without_split_resolves_name() {
        // "15 Cal Row" (no men's/women's split) -> "Row", flat 60s like other calorie targets.
        let lines = only("15 Cal Row");
        assert_eq!(lines[0].name_text, "Row");
        assert_eq!(lines[0].sets[0].duration_seconds, Some(60));
    }

    #[test]
    fn every_x_n_sets_alternating_stations_divides_across_stations() {
        // "Alternating Stations" inverts the usual rule: the 12 sets are spread ACROSS the 3
        // stations (4 visits each), not performed 12 times at each one. Also strips "Station N:"
        // labels and drops the "... visits per station" annotations, including the typo'd one.
        let desc = "Every 3:00 x 12 Sets Alternating Stations\n36:00 tota - 4 visits per station \nStation 1: 45/36 Cal Ski\nStation 2: 35/24 Cal Bike \nStation 3: 45/36 Cal Row\n(4 visits per station, 36:00 total)";
        let lines = only(desc);
        let names: Vec<&str> = lines.iter().map(|l| l.name_text.as_str()).collect();
        assert_eq!(names, vec!["Ski", "Bike", "Row"]);
        assert!(
            lines.iter().all(|l| l.sets.len() == 4),
            "12 sets / 3 stations = 4 each"
        );
    }

    #[test]
    fn odd_even_labels_form_a_two_station_emom_cycle() {
        // "A. EMOM X 18 Min" with alternating Odd/Even stations = a 2-station cycle -> 18 / 2 = 9
        // rounds. Leading "10-15" rep range stripped; "Cal Row" resolves to "Row".
        let desc = "A. EMOM X 18 Min\nOdd: 10-15 Burpees\nEven: 10-15 Cal Row";
        let lines = only(desc);
        let names: Vec<&str> = lines.iter().map(|l| l.name_text.as_str()).collect();
        assert_eq!(names, vec!["Burpees", "Row"]);
        assert!(
            lines.iter().all(|l| l.sets.len() == 9),
            "18 / 2 stations = 9"
        );
        assert_eq!(lines[0].sets[0].reps, Some(10));
    }

    #[test]
    fn n_sets_of_header_applies_round_count() {
        // "3 sets of:" is another spelling of a bare "N Rounds" header, and "35 Min to Work" is
        // a block header rather than a movement. Neither is an exercise; both movements under
        // them get 3 sets. ("@ 3030" is a tempo prescription and is not a movement either.)
        let desc = "A. 35 Min to Work \n3 sets of: \n8/leg KB split RDL @ 3030\n8/leg KB kickstand squats @ 3030";
        let lines = only(desc);
        let names: Vec<&str> = lines.iter().map(|l| l.name_text.as_str()).collect();
        assert_eq!(names, vec!["KB split RDL", "KB kickstand squats"]);
        assert!(
            lines.iter().all(|l| l.sets.len() == 3),
            "\"3 sets of:\" -> 3 sets each; no junk header exercises"
        );
    }

    #[test]
    fn bare_rep_scheme_after_movement_attaches_to_it() {
        // A rep scheme written on the line AFTER its movement, which is the opposite of the
        // usual order. It must attach backwards to the preceding movement rather than becoming
        // an exercise of its own, and the "B." section letter comes off the name.
        let lines = only("B. Deadlift\n5-5-5-5-5");
        assert_eq!(lines.len(), 1, "no junk scheme exercise");
        assert_eq!(lines[0].name_text, "Deadlift");
        let reps: Vec<Option<i64>> = lines[0].sets.iter().map(|s| s.reps).collect();
        assert_eq!(reps, vec![Some(5), Some(5), Some(5), Some(5), Some(5)]);
    }

    #[test]
    fn open_ended_amrap_with_time_remaining_estimates_rounds() {
        // An AMRAP with no time in it at all — "with time remaining" means "however long is left
        // in the session". There is nothing to parse a duration from, so it still has to be
        // recognised as an AMRAP header and fall back to the round estimate.
        let desc = "C. AMRAP with time remaining\nBike 20/15 Cals\n30 Walking lunges";
        let lines = only(desc);
        let names: Vec<&str> = lines.iter().map(|l| l.name_text.as_str()).collect();
        assert!(
            !names.iter().any(|n| n.to_lowercase().contains("amrap")),
            "header not an exercise"
        );
        assert!(
            lines
                .iter()
                .all(|l| l.sets.len() == AMRAP_ESTIMATE_DEFAULT_ROUNDS as usize),
            "open AMRAP -> estimated rounds each"
        );
    }
}
