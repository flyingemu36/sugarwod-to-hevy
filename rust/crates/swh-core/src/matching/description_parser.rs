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
        // "Time Cap: 35:00", and a bare "Time Cap" on its own line above the cap value.
        r"(?i)^Time\s+Cap\b",
        r"(?i)^Rest:",
        r"(?i)^Notes?:",
        r"(?i)^This\s+is",
        r"(?i)^You\s+can",
        r"(?i)^Its?\s+your",
        r"(?i)^Hyrox\s+SIM$",
        // Coaching annotation lines (e.g. "* building to a moderate-heavy load").
        r"^\*",
        // A separator line with no letters or digits at all ("-", "+", "—", "//") — a visual
        // break between parts of a WOD, never a movement.
        r"^[^\p{L}\p{N}]+$",
        // Interval/tempo section headers not carrying a usable count (e.g. "Every 90 sec") — the
        // counted forms are handled by EVERY_RE first; this only catches the leftovers as noise.
        r"(?i)^[A-Z]?\.?\s*Every\s+\d+",
        // A "Rest" line — a bare "Rest" interval (an EMOM's "Minute 5: Rest" after its label is
        // stripped), or one whose duration `REST_RE` could not read ("Rest b/t rounds"). The
        // timed forms are attached to the preceding movement before this point.
        r"(?i)^Rest\b",
        // A quoted workout name on its own line (e.g. "Vaya Con Dios") — not an exercise.
        "^[\"\u{201c}][^\"\u{201d}]*[\"\u{201d}]$",
        // A bare set/rep prescription on its own line after a movement (e.g. "5/5 x 4 Sets @ 40%").
        // Dropped as noise rather than parsed into a junk exercise. (Attaching it back to the
        // preceding movement's set count is a deferred improvement.)
        r"(?i)^\d+(?:/\d+)?\s*[xX]\s*\d+\s*sets?\b",
        // A "N visits per station" / "(… visits per station …)" annotation on an alternating-
        // stations block — describes the format, not an exercise.
        r"(?i)visits?\s+per\s+station",
        // Gym announcements from holiday/closure posts/class times, not movements. A gym posts
        // these to the WOD track on event days, so a whole "track" can consist of nothing else.
        r"(?i)^CLUB\s+HOURS\b",
        r"(?i)^CLASS(?:ES)?\s+(?:OFFERINGS?|CLOSED)\b",
        r"(?i)^NO\s+CLASS(?:ES)?\b",
        r"(?i)\bROOM\s+(?:CLOSED|PREP)\b",
        r"(?i)^COME\s+(?:CHEER|JOIN|WATCH|SUPPORT)\b",
        // A bare clock time or time range on its own line ("6:30-12:40pm", "9AM - 5PM") — an
        // event schedule, not a duration prescription.
        r"(?i)^\d{1,2}(?::\d{2})?\s*(?:am|pm)?\s*-\s*\d{1,2}(?::\d{2})?\s*(?:am|pm)\b",
        // A "N Min Cap" time cap on its own line. The bare "Time Cap:" spelling is handled above;
        // this is the "38 Min Cap" form, whose leading number would otherwise be read as a rep
        // count and leave "Min Cap" behind as the movement name.
        r"(?i)^\d+\s*Min(?:ute)?s?\s+Cap\b",
        r"(?i)^Min(?:ute)?s?\s+Cap\b",
        // A "10 Min Coach choice core work" block — a free-text section the coach fills in on the
        // day, so there is no movement to resolve.
        r"(?i)coach'?s?\s+choice",
        // A "Directly Into..." / "Straight Into:" transition between two halves of a workout.
        r"(?i)^(?:directly|straight|right)\s+into\b",
        // "Chipper" names a format — one long list worked straight through — not a movement. The
        // movements it refers to are the lines that follow it.
        r"(?i)^chipper\b",
        // Coaching prose under a lift: "Suggested loading and jumps for 3RM for the day.",
        // "Percentages are based off a 1RM Back Squat.", "Choice on Loads and Height. Challenging
        // and Unbroken".
        r"(?i)^suggested\b",
        r"(?i)^percentages?\s+(?:are|is|based)\b",
        r"(?i)^choice\s+(?:on|of)\b",
        // An equipment spec line, "Box Height: 24/20in" — the `Dumbbells:` sibling for boxes.
        r"(?i)^(?:box\s+)?height\s*:",
    ]
    .iter()
    .map(|p| Regex::new(p).unwrap())
    .collect()
});

/// An optional section label in front of a header: "A.", "B ", "A .", and the sub-section forms
/// "A2." / "A 2.". A digit is accepted only after a letter and before a dot, so a header that starts
/// with its own number ("12:00 EMOM") never has that number read as a label.
///
/// Every header regex shares this one prefix. They used to each carry their own `[A-Z]?\.?\s*`,
/// which missed "A2.Every 2:00 x 3 Sets", "A 2. 7:00 EMOM" and "A . EMOM x 15:00 Min" — each then
/// fell through to be parsed as an exercise *and* left its section's movements without a round count.
const SECTION_PREFIX: &str = r"(?:[A-Z](?:\s*\d+\s*\.|\s*\.)?\s*)?";

/// Builds a header regex: case-insensitive, anchored, behind the shared `SECTION_PREFIX`.
fn header_regex(body: &str) -> Regex {
    Regex::new(&format!("(?i)^{SECTION_PREFIX}{body}")).unwrap()
}

static EMOM_RE: LazyLock<Regex> = LazyLock::new(|| header_regex(r"(\d+):00\s*EMOM$"));
/// A second real EMOM spelling: "EMOM X 18" / "B.EMOM x18" / "EMOM 18" / "EMOM X 35 MIN" (explicit
/// interval count, optionally suffixed with a "min" unit) instead of "18:00 EMOM". Captured so the
/// header is consumed rather than mistaken for an exercise line.
/// "EMOM x 15:00 Min" spells the count as a clock time; the ":00" is accepted and ignored.
static EMOM_ALT_RE: LazyLock<Regex> =
    LazyLock::new(|| header_regex(r"EMOM\s*[xX]?\s*(\d+)(?::00)?(?:\s*min(?:ute)?s?)?$"));
/// An interval header carrying a count — "Every 1:30 x 8 Sets", "A. Every 2:30 x 4 Sets",
/// "Every 3:00 x 3 Rounds", "Every 2 Min x 14 Min", "Every 2:30 x 15 Min". The interval is a
/// `mm:ss` (g1:g2) or `N min` (g3); the count is g4 with unit g5 (Sets/Rounds => that many rounds,
/// Min => total-duration form, rounds = total / interval). Drives replication of the movement(s)
/// that follow instead of being dropped as noise. Tolerates a unit after a clock interval
/// ("Every 2:00 Min x 12 Min") and the "E very" typo seen in real programming (2026-10-08).
static EVERY_RE: LazyLock<Regex> = LazyLock::new(|| {
    header_regex(
        r"E\s?very\s+(?:(\d+):(\d+)(?:\s*min(?:ute)?s?)?|(\d+)\s*min(?:ute)?s?)\s*[xX]\s*(\d+)\s*(sets?|rounds?|min(?:ute)?s?)\b",
    )
});
/// An "Alternating Stations" marker on an `EVERY_RE` header — signals that its N sets are spread
/// across the "Station M:" stations that follow (per-station count = N / stations) rather than
/// being N sets of a single movement.
static ALTERNATING_STATIONS_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)stations?").unwrap());
/// A bare "5 Rounds" / "5 Rounds:" / "3 Sets" / "4 Sets:" line — a fixed round count applied to
/// the movements that follow (N sets each), not itself an exercise. Distinct from the "30/30 X 7
/// Rounds" work/rest header (`WORKREST_ROUNDS_RE`).
static ROUNDS_RE: LazyLock<Regex> =
    LazyLock::new(|| header_regex(r"(\d+)\s+(?:Rounds?|Sets?)\s*:?$"));
/// A round count carried inside a longer header: "5 Rounds- 15 Min Cap", "For Time · 4 Rounds -
/// Cap 12 Min", "35:00: (Teams of 2) · 4 Rounds". Checked before `SECTION_NOISE_RE`, which would
/// otherwise take "For Time …" as a plain boundary and leave the movements at one set.
static HEADER_ROUNDS_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"(?i)^{SECTION_PREFIX}(\d+)\s+Rounds?\s*[-–:·]|·\s*(\d+)\s+Rounds?\b"
    ))
    .unwrap()
});
/// A bare "Station 1" / "Station 2:" line heading a group of movements in an alternating-stations
/// block. The movements under it belong to that one station rather than each being a station.
static STATION_HEADER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^Station\s*\d+\s*:?$").unwrap());
/// A "N sets of:" round-count header — "3 sets of:", "4 Sets Of". Like `ROUNDS_RE`, a fixed count
/// (N sets each) applied to the movements that follow, not itself an exercise. An optional trailing
/// colon and section letter ("A. 3 sets of:") are tolerated. A range ("B. 4-5 sets of:") takes its
/// low end, as rep ranges do.
static SETS_OF_RE: LazyLock<Regex> =
    LazyLock::new(|| header_regex(r"(\d+)(?:\s*-\s*\d+)?\s+Sets?\s+Of\b\s*:?\s*$"));
/// A work/rest circuit header stating a round count — "30/30 X 7 Rounds", "40/20 x 4 Rounds".
/// The leading "work/rest" pair is not a rep scheme; the captured count drives per-station
/// replication of the movements that follow (whether "N." numbered or plain-listed).
///
/// Behind the shared section prefix: "A. 30/30 X 5 rounds at same station" and "C. 40/20 x 8
/// Rounds" used to slip past as exercise lines, which both reported junk *and* collapsed every
/// station in the section to a single set while the sync reported success.
static WORKREST_ROUNDS_RE: LazyLock<Regex> =
    LazyLock::new(|| header_regex(r"\d+/\d+\s*[xX]\s*(\d+)\s+Rounds?\b"));
/// A time-boxed AMRAP with an open-ended round count, estimated the same way as a team AMRAP.
/// Covers both spellings and an optional "A."/"B." section prefix: "35 Min AMRAP", "B. 18 Min
/// AMRAP", "20:00 AMRAP", "B. 20:00 AMRAP".
static AMRAP_RE: LazyLock<Regex> =
    LazyLock::new(|| header_regex(r"(?:\d+:\d+|\d+\s*min(?:ute)?s?)\s+AMRAP\b"));
/// An open-ended AMRAP header carrying no explicit time token — "AMRAP with time remaining",
/// "C. AMRAP in time remaining", "AMRAP remaining time". Estimated the same way as a team/timed
/// AMRAP (see `AMRAP_ESTIMATE_DEFAULT_ROUNDS`); the header itself is not an exercise.
static AMRAP_OPEN_RE: LazyLock<Regex> = LazyLock::new(|| header_regex(r"AMRAP\b.*\bremaining\b"));
/// A leading section-letter prefix on a movement line — "B. Deadlift", "C. Bike". Only used to
/// strip the label once the line is known to be a movement (real section *headers* like
/// "A. Every …" / "B. 20:00 AMRAP" are consumed by their own matchers before this point). Requires
/// a following non-digit so a numbered-list marker ("2. Air Squat") stays with `LIST_MARKER_RE`.
static SECTION_LETTER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Z]\.\s*(\D.*)$").unwrap());

/// Section-boundary noise: a "For Time - N Min Cap" header, an "if done ... optional" preamble,
/// or a "20/60 x Remaining time" optional-finisher line. Unlike `SKIP_PATTERNS` (which merely
/// drops a line), matching one of these also ends the current section — resetting the round mode
/// so a following bare movement isn't inherited into the prior section's replication.
static SECTION_NOISE_RE: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"(?i)^[A-Z]?\.?\s*For\s+Time\b",
        // "A. 35 Min for Quality" / "B. For Quality with time remaining" — the "For Quality"
        // sibling of "For Time". Unanchored because the phrase only ever appears in a header.
        r"(?i)\bfor\s+quality\b",
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
/// A leading partner label in a two-person workout — "P1- 200 m Run", "P2: Chipper".
///
/// Deliberately *not* part of `INTERVAL_LABEL_RE`, even though it looks alike: an interval/station
/// label means "one slot in a rotating cycle", so it suppresses the section's set replication and
/// gets its round count from the cycle length instead. Partners work *simultaneously*, so P1's
/// movement is performed for the whole section just like an unlabeled one — it wants the ordinary
/// replication, and routing it through the station machinery would collapse it to a single set.
static PARTNER_LABEL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^P\s*\d+\s*[-:]\s*").unwrap());

///
/// A minute label may name several minutes — "Min 2 and 3- 400 m run", "Min 5 & 6:" — in which case
/// the station occupies that many slots of the cycle (see `strip_interval_label`).
static INTERVAL_LABEL_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^(?:(Min(?:ute)?s?\s*\d+(?:\s*(?:and|&|,)\s*\d+)*)|Odd|Even|Station\s*\d+)\s*[:\-]\s*",
    )
    .unwrap()
});
static DIGITS_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\d+").unwrap());
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
/// A timed rest attached to the movement above it: "-Rest 2:00-", "Rest 1:00", "Rest 2:00 between
/// sets", "Rest 1:00 b/t sets". Anything after the time is commentary. Left unmatched, the trailing
/// forms were reported as exercises — and "b/t" was split as a couplet into "Rest 1:00 b" and "t sets".
static REST_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^-?\s*Rest\s+(\d+):(\d+)\b.*$").unwrap());
/// A bare rep ladder on its own line — "21-15-9", "20,18,16,14,12,10", and the open-ended
/// "20,18,16..." (continue down the ladder). Both separators occur in real programming; accepting
/// only "-" left the comma spelling to be read as an exercise name.
static BARE_REP_SCHEME_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\d+(?:\s*[-,]\s*\d+)+)\s*(?:\.{2,}|…)?$").unwrap());
/// A rep prescription on its own line, belonging to the movement(s) above it — "5 Reps @ 50%",
/// "1-2 Reps @ 90%", "3 Reps 65-75%", "1 Rep @ 75%", "8/8 Reps @ 40-45% of 1RM Back Squats".
/// A range or per-side pair takes its first number; a percentage range its low end. A percentage
/// "of" some other lift (g3) is dropped: it is relative to a different max than this movement's,
/// so applying it would prescribe the wrong load.
static BARE_REPS_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^(\d+)(?:\s*[-/]\s*\d+)?\s*Reps?\b\s*(?:@\s*)?(?:(\d+)\s*(?:-\s*\d+\s*)?%\s*\+?)?\s*(of\b.*)?$",
    )
    .unwrap()
});
/// A complex's rep line — "1+1+1 @ 60-65%" under "High Hang Clean + Hang Clean + Low Hang Clean":
/// one rep count per movement of the complex, in order, at a shared percentage.
static COMPLEX_REPS_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(\d+(?:\s*\+\s*\d+)+)\s*(?:@\s*(\d+)\s*(?:-\s*\d+\s*)?%\s*\+?)?$").unwrap()
});
/// An explicit per-set prescription listed under a movement — "Set 1: 8 Reps @ 60-65%",
/// "Set 4: 6 Reps @ 75%+", "Set 6: Max Reps @ 85%". A range takes its low end and a trailing "+"
/// is dropped, matching `REPS_PERCENT_RE`. "Max Reps" yields no rep count (there is no number to
/// prescribe), only the percentage.
/// A "/" with digits on both sides — "20/15 Cals", "50/35 Push-Ups", "1000/900m Row",
/// "45/25 X 5 Rounds". This is the Rx/scaled (or work/rest) spelling, never a couplet separator,
/// and it is the single most confusable form: "Bike 20/15 Cals" otherwise splits into "Bike 20"
/// and "15 Cals", both of which look superficially like movement names.
static NUMERIC_SLASH_PAIR_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\d\s*/\s*\d").unwrap());

/// A "w/" ("with") or "b/t" ("between") on a line, neither of which is a couplet separator —
/// "DB Lunges w/ KB", "Rest b/t rounds".
static W_SLASH_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\bw\s*/|\bb\s*/\s*t\b").unwrap());
/// A " + " joining the movements of a combo or complex — "25 Sit ups + 50 DUs", "8 Chest to Bar
/// Pull-Ups + 12 Pull-Ups", "High Hang Clean + Hang Clean + Low Hang Clean". Spaces are required,
/// so a complex's rep line ("1+1+1") is never split.
static PLUS_SEPARATOR_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+\+\s+").unwrap());

/// A part that is *nothing but* a per-side/per-limb qualifier — the tail of "10/side" or "8/leg"
/// once the line has been split on "/". Matching the whole part rather than just its start is
/// load-bearing: "Side plank" and "Arm Circles" are real movements that begin with one of these
/// words, and a prefix match silently refused to split any couplet containing one.
static PER_SIDE_WORD_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^(?:side|leg|arm|ea|each|hand|round)s?$").unwrap());

/// The "#1: 7 reps" spelling (2026-10-08) is the same block.
static SET_PRESCRIPTION_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^(?:Set\s*|#\s*)\d+\s*:\s*(?:(\d+)|Max)\s*Reps?\b(?:\s*@\s*(\d+)\s*(?:-\s*\d+\s*)?%\s*\+?)?\s*$",
    )
    .unwrap()
});

static TEAM_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)team").unwrap());
static WATERFALL_KEYWORD_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)waterfall|amrap").unwrap());

/// An Rx/scaled distance pair written as one value — "500/450m Row", "1000/900m Ski". Takes the
/// first (Rx) distance, matching how `XY_REPS_RE` takes the first of a rep pair. `XY_REPS_RE`
/// cannot cover this: the unit is fused to the second number ("450m"), so there is no whitespace
/// where it expects the movement name to start, and the whole line fell through to the name.
static RX_DISTANCE_PAIR_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^(\d+)\s*/\s*\d+\s*m(?:eters?)?\.?\s+(\D.+)$").unwrap());

/// A parenthesised equipment dimension — "Box Jumps (24/20in)", "(24in)", "(60cm)". Unlike a
/// men's/women's *load* it prescribes nothing Hevy can store, but left in place it stays glued to
/// the movement name and stops it resolving. Stripped as noise, like a tempo code.
static DIMENSION_ANNOTATION_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)\s*\(\s*\d+(?:\.\d+)?\s*(?:/\s*\d+(?:\.\d+)?\s*)?(?:in|inch(?:es)?|cm|ft|"|')\s*\)"#,
    )
    .unwrap()
});

/// A leading distance — "400 m Run", "200m Run", and a range "100-200 m run" (its low end).
static DISTANCE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^(\d+)(?:\s*-\s*\d+)?\s*m\s*(?:\(\d+ft\))?\s+(.+)").unwrap());
/// A leading distance in feet — "200ft Farmers Carry", "50ft Sled Push", "50/50ft (15/15m) Single
/// Arm Kettlebell Overhead Carry" — converted to meters. A metric equivalent in parentheses is
/// dropped, and a per-side pair takes its first number.
static FEET_DISTANCE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^(\d+)(?:\s*/\s*\d+)?\s*(?:ft|feet|')\.?\s*(?:\(\s*\d+(?:\s*/\s*\d+)?\s*m\s*\)\s*)?(\D.+)$",
    )
    .unwrap()
});
/// Distance *after* the movement (e.g. "Run 200 m"). Accepts the unit written as "m", "m.",
/// "meter" or "meters" — coaches use all four, and an unmatched spelling leaves the number glued
/// to the name, which resolves to nothing.
static DISTANCE_SUFFIX_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(.+?)\s+(\d+)\s*m(?:eters?)?\.?\s*(?:\(\d+ft\))?\s*$").unwrap()
});
/// "Cal" is also spelled out — "15/12 Calorie Ski", "30/24 Calorie Row" (2026-10-14, 10-22). The
/// short form alone used to read those as 15 *reps* of a calorie-named movement.
static CAL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^(\d+)\s*/\s*(\d+)\s+Cal(?:orie)?s?\s+(.+)$").unwrap());
/// A single-number (or rep-range) calorie target — "15 Cal Row", "10-15 Cal Row" — the men's/
/// women's-split form is `CAL_RE`. Same decision: flat 60s/0km, calorie number unused.
/// A calorie target written *after* the movement — "Bike 20/15 Cals", "Row 15 Cal". The leading
/// forms are `CAL_RE` and `CAL_SINGLE_RE`; this is the same instruction with the words swapped,
/// and it gets the same flat 60s treatment. Without it the target stays on the name ("bike 20 15
/// cals") and the movement is dropped.
static CAL_SUFFIX_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^(\D.*?)\s+\d+(?:\s*[/-]\s*\d+)?\s*Cals?\.?\s*$").unwrap());
static CAL_SINGLE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\d+(?:-\d+)?\s*Cal(?:orie)?s?\s+(.+)$").unwrap());
/// A leading rep range — "10-15 Burpees" (do 10-15 reps). The name is what follows; reps take the
/// low end of the range (the athlete trues up in Hevy). Distinct from `DASH_REP_SERIES_RE`, which
/// is a name *followed* by a set-by-set scheme ("Back Squat 12-10-8").
static RANGE_REPS_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\d+)\s*-\s*\d+\s+(\D.+)$").unwrap());
/// A leading ":SS" seconds duration — ":45 Plank Hold", ":60 KB Farmers Walk". Stripped so the name
/// resolves; the seconds become the set duration.
/// A per-side pair (":15/:15 Copenhagen Plank") takes its first duration.
static LEADING_DURATION_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^:(\d+)(?:\s*/\s*:\d+)?\s+(\D.+)$").unwrap());
/// A "Take 15:00 to Establish a 3RM" / "Build to a heavy 1RM" lead-in on a lift. The movement is
/// what follows; the lead-in is format, and left in place it kept "Deadlift" from resolving.
static ESTABLISH_PREFIX_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^(?:take\s+[\d:]+\s*(?:min(?:ute)?s?\s+)?to\s+)?(?:establish|find|build\s+(?:up\s+)?to)\s+(?:an?\s+)?(?:heavy\s+)?\d+\s*RM\s+(?:of\s+|for\s+)?",
    )
    .unwrap()
});
/// A trailing loading instruction — "Push Press for load:", "50ft Sled Push, For Load".
static FOR_LOAD_SUFFIX_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)[\s,]*\bfor\s+load\s*:?\s*$").unwrap());
/// A trailing parenthetical of plain words — "High Box Jumps (Choose a Challenging Height)". Three
/// or more words, so an equipment tag like "(DB)" or "(Dumbbell)" is never touched.
static PROSE_PAREN_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\s*\((?:[A-Za-z]+\s+){2,}[A-Za-z]+\)\s*$").unwrap());
/// Punctuation left dangling at the end of a name once annotations are stripped — the " /" of
/// "Single DB Step-Overs (50/35lb) /(24/20')".
static TRAILING_PUNCT_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[\s/,:;\-]+$").unwrap());
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
/// "5 Push Press @ 70%" and the range form "5 Push Press @ 70-75%". A range takes its low end
/// (g3), matching how `RANGE_REPS_RE` treats a rep range — the athlete trues up in Hevy, and the
/// bottom of a prescribed range is the safe end to start from. The range spelling is common in
/// real programming and was previously unmatched, which dropped the %1RM *and* left "@ 70-75%"
/// glued to the movement name so it resolved to nothing.
static REPS_PERCENT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^(\d+)\s+(.+?)\s*@\s*(\d+)\s*(?:-\s*\d+\s*)?%$").unwrap());
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
/// An Rx load written as a men's/women's pair in parentheses. The unit is optional and may sit
/// either *inside* the parens ("(95/65lbs)", "(110/70lb)") or *outside* them ("(95/65)lb"), in
/// singular or plural — coaches write all of these, and real programming overwhelmingly puts it
/// inside. An earlier version accepted only the outside form, so a real-world "(95/65lbs)" both
/// lost its load *and* kept the annotation glued to the movement name, which then resolved to
/// nothing: one regex gap silently cost the weight *and* the exercise.
static WEIGHT_ANNOTATION_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\(\s*(\d+(?:\.\d+)?)\s*/\s*(\d+(?:\.\d+)?)\s*(lbs?|kgs?)?\s*\)\s*(lbs?|kgs?)?")
        .unwrap()
});

/// Parses a SugarWOD `description` into structured exercise lines. Mode-aware: tracks whether
/// the current section is a plain line-by-line list, an EMOM interval (enumerated stations are
/// replicated per round = `intervals / stations`), a fixed "N Rounds" / "Every … x N Sets" count,
/// or a team/AMRAP round estimate — see the module-level doc in `mod.rs` and the plan's
/// "Matching pipeline" section for the full rationale.
pub fn parse_description(description: &str) -> Vec<ParsedExerciseLine> {
    let mut mode = LineMode::Default;
    let mut pending_rep_scheme: Option<Vec<i64>> = None;
    // Index into `lines_out` of the movement an open "Set N:" block is currently filling, so the
    // second and later lines of the block append instead of each resetting it. No explicit reset
    // is needed: the block is only continued while its target is still the most recent line, so
    // any intervening movement or header naturally ends it.
    let mut set_block: Option<usize> = None;
    let mut lines_out: Vec<ParsedExerciseLine> = Vec::new();
    // The currently-open enumerated-station EMOM section, if any. Finalized (per-round set counts
    // applied) whenever the section ends: a new header, a section-noise boundary, or end of input.
    let mut emom: Option<EmomAccum> = None;
    // A section header has been seen since the last movement was emitted. A rep line ("1 Rep @
    // 75%") after a header starts *more* sets of the movement above it, rather than restating it.
    let mut header_since_movement = false;
    // Inside a bare "Station N" group: the movements that follow belong to that one station.
    let mut station_block = false;
    // The movements emitted from the most recent movement line — several for a couplet or a
    // complex — which a following rep line ("5 Reps @ 50%", "1+1+1 @ 60%") describes.
    let mut last_group: Vec<usize> = Vec::new();
    // An open run of rep lines under `last_group`; see `apply_reps_line`.
    let mut reps_block: Option<RepsBlock> = None;
    macro_rules! close_emom {
        () => {
            if let Some(acc) = emom.take() {
                apply_emom_rounds(&mut lines_out, &acc);
            }
            // A ladder header ("21-15-9") and a station group both belong to their section.
            pending_rep_scheme = None;
            station_block = false;
            header_since_movement = true;
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

        if let Some(caps) = HEADER_ROUNDS_RE.captures(trimmed) {
            close_emom!();
            let rounds: u32 = caps
                .get(1)
                .or_else(|| caps.get(2))
                .and_then(|m| m.as_str().parse().ok())
                .unwrap_or(1);
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

        // Strip a leading partner label ("P1- 200 m Run"). Unlike the section letter this starts
        // no new section and unlike a station label it implies no rotation, so neither the mode nor
        // the open circuit is touched — only the prefix is removed.
        let trimmed = match PARTNER_LABEL_RE.find(trimmed) {
            Some(m) => trimmed[m.end()..].trim(),
            None => trimmed,
        };

        // A bare "Station 1" heads a group of movements that together make up one station of an
        // alternating-stations block: count the station once here, and treat everything under it
        // as that station's (no whole-section replication) until the section ends.
        if STATION_HEADER_RE.is_match(trimmed) {
            if let Some(acc) = emom.as_mut() {
                acc.station_count += 1;
                station_block = true;
            }
            continue;
        }

        // Strip a leading "Minute N:" / "Min N-" station label before any further parsing.
        // Such lines enumerate distinct EMOM stations, so they must not trigger the
        // whole-section set replication below (tracked via `from_interval_label`).
        let (content, label_slots) = strip_interval_label(trimmed, emom.is_some());

        // A labeled line is one EMOM station in the open cycle — count it (including a "Rest"
        // minute that gets dropped just below) so the cycle length is known at section end. A
        // station spanning several minutes ("Min 2 and 3-") fills that many slots of the cycle.
        if label_slots > 0 {
            if let Some(acc) = emom.as_mut() {
                acc.station_count += label_slots;
            }
        }
        let from_interval_label = label_slots > 0 || (station_block && emom.is_some());

        if let Some(caps) = REST_RE.captures(content) {
            let minutes: i64 = caps[1].parse().unwrap_or(0);
            let seconds: i64 = caps[2].parse().unwrap_or(0);
            if let Some(last) = lines_out.last_mut() {
                last.rest_seconds_after = Some(minutes * 60 + seconds);
            }
            continue;
        }

        // An explicit per-set prescription belonging to the movement above it. Each line adds one
        // set to that movement and *replaces* whatever the section's round mode replicated onto it:
        // the coach enumerated the sets outright, so both their count and their per-set percentages
        // win over a count inferred from the section header.
        //
        // Untreated, these lines each became a junk exercise name *and* left the movement with a
        // flat, mode-replicated set list carrying no percentage — so every ramp silently collapsed
        // to `load_calc`'s unlabeled-load default of 50% of max, with no rep targets. That is a
        // worse failure than an unmapped warning, because the sync reports success: a real
        // "Push Press, 8 reps @ 60-65% ... 6 reps @ 75%+" off a 175 lb max synced as six flat sets
        // of 90 lb with no reps, roughly two thirds of the prescribed top-end load.
        if let Some(caps) = SET_PRESCRIPTION_RE.captures(content) {
            if let Some(target) = lines_out.len().checked_sub(1) {
                let spec = SetSpec {
                    reps: caps.get(1).and_then(|m| m.as_str().parse().ok()),
                    percent_1rm: caps
                        .get(2)
                        .and_then(|m| m.as_str().parse::<f64>().ok())
                        .map(|p| p / 100.0),
                    // A load stated on the movement line itself ("Deadlifts (225/155lb)") still
                    // applies when the per-set line names only reps and a percentage.
                    explicit_weight_lb: lines_out[target]
                        .sets
                        .first()
                        .and_then(|s| s.explicit_weight_lb),
                    ..Default::default()
                };
                if set_block == Some(target) {
                    lines_out[target].sets.push(spec);
                } else {
                    lines_out[target].sets = vec![spec];
                    set_block = Some(target);
                }
            }
            continue;
        }

        if let Some(reps_line) = parse_reps_line(content) {
            if !last_group.is_empty() {
                let rounds = if header_since_movement {
                    Some(mode_rounds(mode))
                } else {
                    None
                };
                apply_reps_line(
                    &mut lines_out,
                    &last_group,
                    &reps_line,
                    rounds,
                    &mut reps_block,
                );
                header_since_movement = false;
            }
            continue;
        }

        if let Some(caps) = BARE_REP_SCHEME_RE.captures(content) {
            let scheme: Vec<i64> = caps[1]
                .split(['-', ','])
                .filter_map(|s| s.trim().parse().ok())
                .collect();
            // A bare scheme after a movement ("B. Deadlift" \n "5-5-5-5-5") describes *that*
            // movement: attach it to the preceding bare single-set line. Otherwise it's a leading
            // header ("21-15-9" \n "Pull-ups") — keep it pending for the section's bare lines.
            match lines_out.last_mut() {
                Some(prev) if is_bare_sets(prev) && !header_since_movement => {
                    let weight = prev.sets[0].explicit_weight_lb;
                    prev.sets = ladder_sets(&scheme, weight);
                }
                _ => pending_rep_scheme = Some(scheme),
            }
            continue;
        }

        if SKIP_PATTERNS.iter().any(|re| re.is_match(content)) {
            continue;
        }

        // A couplet station ("Push ups/ TRX supinated row") holds two movements that are each
        // their own Hevy exercise but share one station slot: the station is counted once above,
        // while both halves are emitted and both are replicated per round.
        let mut group = Vec::new();
        for part in split_couplet(content) {
            let Some(mut line) = parse_exercise_line(part) else {
                continue;
            };
            // A leading ladder ("20,18,16..." above "DB push press" and "TRX low row") is the rep
            // scheme for every bare movement in its section, not only the first one. It used to be
            // consumed by the first, leaving the rest at one undifferentiated set per round.
            if is_bare_sets(&line) {
                if let Some(scheme) = &pending_rep_scheme {
                    let weight = line.sets[0].explicit_weight_lb;
                    line.sets = ladder_sets(scheme, weight);
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
            group.push(idx);
            if from_interval_label {
                if let Some(acc) = emom.as_mut() {
                    acc.emitted_idxs.push(idx);
                }
            }
        }
        if !group.is_empty() {
            last_group = group;
            header_since_movement = false;
        }
    }

    if let Some(acc) = emom.take() {
        apply_emom_rounds(&mut lines_out, &acc);
    }
    lines_out
}

/// True if the line's single set prescribes no reps, distance, duration or percentage (a "bare"
/// fallback line, e.g. a plain "Pull-ups" under a pending "21-15-9" header) — the only case a
/// bare rep-scheme header should be applied to. A stated load ("Thrusters (95/65)") still counts
/// as bare: it says how heavy, not how many, and is carried onto every set of the ladder.
fn is_bare_sets(line: &ParsedExerciseLine) -> bool {
    line.sets.len() == 1
        && SetSpec {
            explicit_weight_lb: None,
            ..line.sets[0].clone()
        } == SetSpec::default()
}

/// One set per rung of a rep ladder, each carrying the movement's stated load.
fn ladder_sets(scheme: &[i64], explicit_weight_lb: Option<f64>) -> Vec<SetSpec> {
    scheme
        .iter()
        .map(|&r| SetSpec {
            reps: Some(r),
            explicit_weight_lb,
            ..Default::default()
        })
        .collect()
}

/// The round count a section's mode replicates single-set movements to.
fn mode_rounds(mode: LineMode) -> u32 {
    match mode {
        LineMode::Emom { rounds }
        | LineMode::AmrapEstimate { rounds }
        | LineMode::Rounds { rounds } => rounds.max(1),
        LineMode::Default => 1,
    }
}

/// A rep line with no movement name of its own (see `BARE_REPS_RE` / `COMPLEX_REPS_RE`).
#[derive(Debug, Clone, PartialEq)]
struct RepsLine {
    /// One entry applies to every movement it describes; several (a complex's "1+1+1") apply to
    /// those movements in order.
    reps: Vec<Option<i64>>,
    percent_1rm: Option<f64>,
}

fn parse_reps_line(content: &str) -> Option<RepsLine> {
    let percent = |m: Option<regex::Match>| {
        m.and_then(|m| m.as_str().parse::<f64>().ok())
            .map(|p| p / 100.0)
    };
    if let Some(caps) = BARE_REPS_RE.captures(content) {
        return Some(RepsLine {
            reps: vec![caps[1].parse().ok()],
            // "@ 40-45% of 1RM Back Squats": a percentage of a different lift's max.
            percent_1rm: if caps.get(3).is_some() {
                None
            } else {
                percent(caps.get(2))
            },
        });
    }
    let caps = COMPLEX_REPS_RE.captures(content)?;
    Some(RepsLine {
        reps: caps[1].split('+').map(|r| r.trim().parse().ok()).collect(),
        percent_1rm: percent(caps.get(2)),
    })
}

/// A run of consecutive rep lines under the same movement group.
struct RepsBlock {
    group: Vec<usize>,
    /// The first line's per-movement sets, kept so a second line can turn the block into a list.
    first: Vec<SetSpec>,
    lines: usize,
}

/// Applies a rep line to the movements of the line above it (`group`).
///
/// - **One line** restates the prescription of every set the movement already has: "Bench Press"
///   under a 12:00 EMOM, then "3 Reps 65-75%", is twelve sets of 3 at 65%.
/// - **Several lines in a row** list the sets outright — the 2026-10-09 "5 Reps @ 50%" / "4 Reps @
///   60%" / … / "1-2 Reps @ 90%" ramp is six sets, replacing any count from the section header.
/// - **After a new header** (`rounds` is `Some`), the line is *more* sets: "2 Snatch @ 70%" under
///   one 7:00 EMOM, then "A 2. 7:00 EMOM" / "1 Rep @ 75%", adds seven singles at 75%.
///
/// Every set keeps the movement's stated load. Untreated, each of these lines became a junk
/// exercise and left the movement with no reps or percentage at all.
fn apply_reps_line(
    lines_out: &mut [ParsedExerciseLine],
    group: &[usize],
    reps_line: &RepsLine,
    rounds: Option<u32>,
    block: &mut Option<RepsBlock>,
) {
    let spec_for = |i: usize, line: &ParsedExerciseLine| SetSpec {
        reps: reps_line
            .reps
            .get(i)
            .or(reps_line.reps.first())
            .copied()
            .flatten(),
        percent_1rm: reps_line.percent_1rm,
        explicit_weight_lb: line.sets.first().and_then(|s| s.explicit_weight_lb),
        ..Default::default()
    };

    if let Some(rounds) = rounds {
        for (i, &idx) in group.iter().enumerate() {
            let spec = spec_for(i, &lines_out[idx]);
            lines_out[idx]
                .sets
                .extend(std::iter::repeat_n(spec, rounds as usize));
        }
        *block = None;
        return;
    }

    match block {
        Some(b) if b.group == group => {
            for (i, &idx) in group.iter().enumerate() {
                let spec = spec_for(i, &lines_out[idx]);
                if b.lines == 1 {
                    lines_out[idx].sets = vec![b.first[i].clone()];
                }
                lines_out[idx].sets.push(spec);
            }
            b.lines += 1;
        }
        _ => {
            let mut first = Vec::new();
            for (i, &idx) in group.iter().enumerate() {
                let spec = spec_for(i, &lines_out[idx]);
                for set in &mut lines_out[idx].sets {
                    set.reps = spec.reps.or(set.reps);
                    set.percent_1rm = spec.percent_1rm.or(set.percent_1rm);
                }
                first.push(spec);
            }
            *block = Some(RepsBlock {
                group: group.to_vec(),
                first,
                lines: 1,
            });
        }
    }
}

/// Strips a leading interval/station label ("Minute 3:", "Min 3-", "Odd:") or numbered-list marker
/// if present, returning the remaining text and whether a label was removed. Inside an EMOM cycle
/// (`in_emom`), also strips a marker fused to its reps ("2.20 …"); outside one, a leading decimal is
/// preserved. See `INTERVAL_LABEL_RE` / `LIST_MARKER_RE` / `LIST_MARKER_LOOSE_RE`.
///
/// The count returned is how many slots of the cycle the station fills: 0 for no label, the number
/// of minutes named for a multi-minute label ("Min 2 and 3-" is 2), else 1.
fn strip_interval_label(line: &str, in_emom: bool) -> (&str, u32) {
    if let Some(caps) = INTERVAL_LABEL_RE.captures(line) {
        let slots = caps
            .get(1)
            .map(|m| DIGITS_RE.find_iter(m.as_str()).count() as u32)
            .unwrap_or(1)
            .max(1);
        return (line[caps.get(0).unwrap().end()..].trim(), slots);
    }
    if let Some(caps) = LIST_MARKER_RE.captures(line) {
        return (caps.get(1).unwrap().as_str().trim(), 1);
    }
    if in_emom {
        if let Some(caps) = LIST_MARKER_LOOSE_RE.captures(line) {
            return (caps.get(1).unwrap().as_str().trim(), 1);
        }
    }
    (line, 0)
}

/// Splits a couplet/triplet station line into its separate movements — "Push ups/ TRX supinated
/// row" becomes `["Push ups", "TRX supinated row"]`. Returns the line unsplit as a single element
/// for everything else, which is the overwhelming majority of lines.
///
/// "/" is badly overloaded in this data: Rx/scaled reps ("50/35 Push-Ups") and distances
/// ("1000/900m Row"), work/rest intervals ("45/25 X 5 Rounds"), per-side counts ("10/side"), and
/// "w/" for "with". Splitting one of those would shred an ordinary line into junk, so the test is
/// deliberately strict. A "/" flanked by digits is never a separator, which rules out every
/// Rx/scaled and work/rest form outright — including "Bike 20/15 Cals", whose two halves would
/// otherwise both pass a naive name test. Beyond that, *every* part must read like a movement name:
/// at least two letters and not a bare per-side qualifier. Lines carrying parentheses are left alone
/// too, so an "(American/Russian)"-style annotation is never treated as a separator.
///
/// Whole tracks depended on this: a gym that programs "1. Push ups/ TRX supinated row" put three
/// days' Fitness and HYROX workouts through as a single unresolvable name each, syncing nothing.
fn split_couplet(content: &str) -> Vec<&str> {
    // A " + " combo is checked first and wins outright: its parts are whole movement lines,
    // each with its own rep count ("25 Sit ups + 50 DUs").
    if PLUS_SEPARATOR_RE.is_match(content) {
        let parts: Vec<&str> = PLUS_SEPARATOR_RE.split(content).map(str::trim).collect();
        if parts
            .iter()
            .all(|p| p.chars().filter(|c| c.is_alphabetic()).count() >= 2)
        {
            return parts;
        }
    }

    if !content.contains('/')
        || content.contains('(')
        || content.contains(')')
        || NUMERIC_SLASH_PAIR_RE.is_match(content)
        || W_SLASH_RE.is_match(content)
    {
        return vec![content];
    }

    let parts: Vec<&str> = content.split('/').map(str::trim).collect();
    let looks_like_movement = |p: &&str| {
        p.chars().filter(|c| c.is_alphabetic()).count() >= 2 && !PER_SIDE_WORD_RE.is_match(p)
    };
    if (2..=3).contains(&parts.len()) && parts.iter().all(looks_like_movement) {
        parts
    } else {
        vec![content]
    }
}

fn parse_exercise_line(trimmed: &str) -> Option<ParsedExerciseLine> {
    let (text, rpe_note) = extract_rpe(trimmed);
    // Strip an equipment dimension before the load annotation, so "(24/20in)" is never mistaken
    // for a men's/women's weight pair.
    let text = DIMENSION_ANNOTATION_RE.replace(&text, "").to_string();
    let (text, explicit_weight_lb) = extract_weight_annotation(&text);
    // Drop a trailing tempo code ("@ 3030") — informational, not stored, and would pollute the name.
    let text = TEMPO_RE.replace(&text, "").to_string();
    // Format wording around the movement: a "Take 15:00 to Establish a 3RM" lead-in, a trailing
    // "for load:" / "(Choose a Challenging Height)", and punctuation left dangling by any of the
    // strips above.
    let text = ESTABLISH_PREFIX_RE.replace(text.trim(), "").to_string();
    let text = FOR_LOAD_SUFFIX_RE.replace(&text, "").to_string();
    let text = PROSE_PAREN_RE.replace(&text, "").to_string();
    let text = TRAILING_PUNCT_RE.replace(text.trim(), "").to_string();
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

    if let Some(caps) = RX_DISTANCE_PAIR_RE.captures(&text) {
        set.distance_meters = caps[1].parse().ok();
        return Some(ParsedExerciseLine {
            name_text: caps[2].trim().to_string(),
            sets: vec![set],
            rpe_note,
            rest_seconds_after: None,
        });
    }

    if let Some(caps) = FEET_DISTANCE_RE.captures(&text) {
        set.distance_meters = caps[1]
            .parse::<f64>()
            .ok()
            .map(|ft| (ft * 0.3048).round() as i64);
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

    if let Some(caps) = CAL_SUFFIX_RE.captures(&text) {
        // Trailing calorie target ("Bike 20/15 Cals") — same flat 60s/0km as the leading forms.
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
        // The unit may be inside the parens (group 3) or outside them (group 4); whichever is
        // present wins. Matched case-insensitively and possibly plural ("lbs", "KG"), so compare
        // on a lowercased prefix rather than an exact "kg".
        let unit = caps
            .get(3)
            .or_else(|| caps.get(4))
            .map(|m| m.as_str().to_lowercase());
        let lb = if unit.as_deref().is_some_and(|u| u.starts_with("kg")) {
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

    /// Thin alias for [`parse_description`], purely to keep the test bodies short.
    fn parse(desc: &str) -> Vec<ParsedExerciseLine> {
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

    /// A line of nothing but punctuation is a visual separator between parts of a WOD, never a
    /// movement. Both descriptions are verbatim from the two days it crashed the sync (2026-10-15
    /// HYROX, 2026-10-21 Performance): the separator survived as an exercise named "-" / "+",
    /// whose empty normalized key DynamoDB then rejected.
    #[test]
    fn punctuation_only_separator_lines_are_noise() {
        let hyrox = parse(
            "30 Min AMRAP \nFor Time:\n21/15 Cal Bike \n21 Line Facing Burpees\n15/12 Cal Bike\n15 Line Facing Burpees\n9/7 Cal Bike\n9 Line Facing Burpees\n-\n800m Run\n-\n21/15 Cal Row\n21 Lateral Burpees over the Rower\n15/12 Cal Row\n15 Lateral Burpees over the Rower\n9/7 Cal Row\n9 Lateral Burpees over the Rower\n-\n800m Run",
        );
        let perf = parse(
            "A. Every 4:00 x 5 Sets\n5 Front Squats @ 65-70%\n+\n2/2 Turkish Get-Ups\n\nB. EMOM x 12 Min \n1. 8 Heavy DB Box Step Overs \n2. 3/ Pistol Squat \n3. 8/5 Cal Bike",
        );
        for line in hyrox.iter().chain(perf.iter()) {
            assert!(
                line.name_text.chars().any(char::is_alphanumeric),
                "separator parsed as an exercise: {:?}",
                line.name_text
            );
        }
        // Dropping the "+" must not cost the superset its round count.
        let tgu = perf
            .iter()
            .find(|l| l.name_text == "Turkish Get-Ups")
            .expect("Turkish Get-Ups parsed");
        assert_eq!(tgu.sets.len(), 5);
        for sep in ["-", "+", "--", "—", "* * *", "//"] {
            assert!(parse(sep).is_empty(), "{sep:?} parsed as an exercise");
        }
    }

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
            (
                "Run 200 meters",
                "Run",
                200,
                "distance last, unit spelled out",
            ),
            (
                "Row 500 Meter",
                "Row",
                500,
                "distance last, singular and capitalised",
            ),
        ] {
            let lines = parse(desc);
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
            let lines = parse(desc);
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
            let lines = parse(desc);
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
            let lines = parse(desc);
            assert_eq!(lines.len(), 1, "{notation}: {desc:?}");
            assert_eq!(lines[0].name_text, name, "{notation}: {desc:?}");
            assert_eq!(lines[0].sets[0].reps, reps, "{notation}: reps for {desc:?}");
            assert_eq!(
                lines[0].sets[0].duration_seconds, seconds,
                "{notation}: duration for {desc:?}"
            );
        }
    }

    /// Calorie targets appear on either side of the movement: "15 Cal Row" and "Bike 20/15 Cals".
    /// Both are a work target rather than reps, and Hevy has no calorie field, so both become the
    /// same flat 60-second effort — and either way the number must come off the name, or nothing
    /// resolves against the catalog.
    #[test]
    fn calorie_targets_reduce_to_a_bare_name_on_either_side() {
        for (desc, name, spelling) in [
            ("15 Cal Row", "Row", "leading, single number"),
            ("10-15 Cal Row", "Row", "leading, rep range"),
            ("12/9 Cal Row", "Row", "leading, split"),
            ("Bike 20/15 Cals", "Bike", "trailing, split"),
            ("Row 15 Cal", "Row", "trailing, single number"),
        ] {
            let lines = parse(desc);
            assert_eq!(lines.len(), 1, "{spelling}: {desc:?}");
            assert_eq!(lines[0].name_text, name, "{spelling}: {desc:?}");
            assert_eq!(
                lines[0].sets[0].duration_seconds,
                Some(60),
                "{spelling}: calorie targets are a flat 60s effort"
            );
        }
    }

    #[test]
    fn dash_rep_series() {
        // A descending ladder written after the movement: six sets of 12, 10, 8, 6, 4, 2 reps.
        // Each number is its own set, so the set count comes from the series length.
        let lines = parse("Back Squat 12-10-8-6-4-2");
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
        let lines = parse("21-15-9\nPull-ups");
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].name_text, "Pull-ups");
        let reps: Vec<Option<i64>> = lines[0].sets.iter().map(|s| s.reps).collect();
        assert_eq!(reps, vec![Some(21), Some(15), Some(9)]);
    }

    #[test]
    fn set_by_rep_notation() {
        // "5x5" is sets-by-reps: five sets of five, not one set of 25.
        let lines = parse("Bench Press 5x5");
        assert_eq!(lines[0].name_text, "Bench Press");
        assert_eq!(lines[0].sets.len(), 5);
        assert!(lines[0].sets.iter().all(|s| s.reps == Some(5)));
    }

    #[test]
    fn weight_annotation_extracted() {
        // "(95/65)" is a prescribed load in pounds, men's/women's. Take the first number as the
        // explicit weight; it overrides any percentage-of-max calculation later in the pipeline.
        let lines = parse("Back Squat (95/65)");
        assert_eq!(lines[0].name_text, "Back Squat");
        assert_eq!(lines[0].sets[0].explicit_weight_lb, Some(95.0));
    }

    #[test]
    fn weight_annotation_with_the_unit_inside_the_parens() {
        // Real 2026-09-10..12 programming puts the unit *inside* the parens, singular or plural.
        // The regex used to accept only "(95/65)lb", so every one of these kept the annotation on
        // the name ("S2OH (95/65lbs)") and resolved to nothing — losing the load and the movement.
        for (line, name, lb) in [
            ("15 S2OH (95/65lbs)", "S2OH", 95.0),
            ("11 DB Power Cleans (110/70lb)", "DB Power Cleans", 110.0),
            ("11 Wall Balls (20/14lb)", "Wall Balls", 20.0),
            (
                "DB Kettlebell Thrusters (50/35lbs) ",
                "DB Kettlebell Thrusters",
                50.0,
            ),
        ] {
            let lines = parse(line);
            assert_eq!(lines[0].name_text, name, "name for {line:?}");
            assert_eq!(
                lines[0].sets[0].explicit_weight_lb,
                Some(lb),
                "load for {line:?}"
            );
        }
    }

    #[test]
    fn weight_annotation_unit_outside_the_parens_and_kg_conversion_still_work() {
        let lines = parse("Wall Balls (20/14)lb");
        assert_eq!(lines[0].name_text, "Wall Balls");
        assert_eq!(lines[0].sets[0].explicit_weight_lb, Some(20.0));

        // Inside-the-parens kg, any case, still converts rather than being read as pounds.
        let lines = parse("Sandbag Clean (60/40KG)");
        assert_eq!(lines[0].name_text, "Sandbag Clean");
        let lb = lines[0].sets[0].explicit_weight_lb.unwrap();
        assert!((lb - crate::units::kg_to_lb(60.0)).abs() < 1e-9, "got {lb}");
    }

    #[test]
    fn percent_range_takes_the_low_end() {
        // Real 2026-09-12 / 09-10 spelling. Previously unmatched: "@ 70-75%" stayed on the name
        // and both the %1RM and the movement were lost, while the single-number "@ 70%" worked.
        let lines = parse("5 Push Press @ 70-75% ");
        assert_eq!(lines[0].name_text, "Push Press");
        assert_eq!(lines[0].sets[0].reps, Some(5));
        assert_eq!(lines[0].sets[0].percent_1rm, Some(0.70));

        let lines = parse("3 Front Squats @ 75 - 80%");
        assert_eq!(lines[0].name_text, "Front Squats");
        assert_eq!(lines[0].sets[0].percent_1rm, Some(0.75));
    }

    #[test]
    fn rpe_suffix_becomes_note_not_numeric_field() {
        // RPE ("rate of perceived exertion") is how hard a set should feel, 1-10. It is a
        // judgement call, not a load, so it is preserved verbatim as a note for the athlete to
        // read rather than converted into a weight the parser would only be guessing at.
        let lines = parse("Back Squat 5x5 @ RPE 8");
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
            parse("A.5:00 EMOM\n2 Push Press @ 65%\n-Rest 2:00-\n5:00 EMOM\n2 Push Jerks @ 65%");
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
        let lines = parse("B. 3-Person Team Waterfall\n24:00 AMRAP\n12 Alt DB Hang Snatch");
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
        let lines = parse("B. 3-Person Team Waterfall\n24:00 AMRAP\n12/9 Cal Row\n12/9 Cal  Bike");
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
        let lines = parse("B. 3-Person Team Waterfall\n24:00 AMRAP\n12/9 Push-Ups");
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
        let lines = parse(EMOM_WITH_MINUTE_LABELS);
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
        let lines = parse(EVERY_THEN_EMOM_WITH_MIN_LABELS);
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
        let lines = parse("5 Rounds\n12 Deadlifts\n9 Hang Power Cleans\n6 Push Jerks");
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
        let lines = parse(desc);
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
        let lines = parse(
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
        let lines = parse(desc);
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
        let lines = parse("CLUB HOURS: 7A-7PM\nCLASS OFFERINGS 9AM & 4PM");
        assert!(lines.is_empty());
    }

    #[test]
    fn numbered_list_stations_stripped_and_workrest_rounds_applied() {
        // Stations written as a numbered list. The "N." markers come off so the names resolve,
        // and "30/30 X 7 Rounds" (30s work / 30s rest, 7 times through) gives every station 7
        // sets — the rounds apply to each station, not split across them.
        let desc = "30/30 X 7 Rounds \n1. DB Pullover\n2. Medball V-Up\n3. Jump Rope\n4. DB Floor Press\n5. DB Curls";
        let lines = parse(desc);
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
        let lines = parse("40/20 x 4 Rounds\nRow\nWall Balls");
        let names: Vec<&str> = lines.iter().map(|l| l.name_text.as_str()).collect();
        assert_eq!(names, vec!["Row", "Wall Balls"]);
        assert!(lines.iter().all(|l| l.sets.len() == 4));
    }

    #[test]
    fn decimal_leading_number_is_not_mistaken_for_a_list_marker() {
        // "1.5 Turkish Get-Up" must keep its "1.5" (a load/qualifier), not be split into "5 …".
        let lines = parse("1.5 Turkish Get-Up");
        assert_eq!(lines[0].name_text, "1.5 Turkish Get-Up");
    }

    #[test]
    fn skip_lines_produce_no_exercises() {
        // Preamble, prose, and equipment lists are not movements. Emitting any of them would put
        // a junk entry in the athlete's Hevy routine, so the whole input must yield nothing.
        let lines = parse("Hyrox SIM\nThis is the Hyrox Workout. You can do this solo, partner, relay, or half it all! Its your workout!\nA For Time:\nDumbbells: 2 x 50/35lb, 22.5/15kg");
        assert!(lines.is_empty());
    }

    #[test]
    fn full_hyrox_fixture_matches_expected_exercise_count() {
        // A full real workout end to end, as a backstop on the single-behaviour tests above:
        // the repeated runs must each survive as their own line rather than being deduplicated
        // or collapsed, and the prose header must not leak in as an exercise.
        let desc = "Hyrox SIM\nThis is the Hyrox Workout. You can do this solo, partner, relay, or half it all! Its your workout!\n\n1000 m Run\n1000 m SKI\n1000 m Run\n50 m Sled push\n1000 m Run\n50 m Sled pull\n1000 m Run\n80 m Burpee Broad jumps\n1000 m run\n1000 m Row\n1000 m run\n200 m Farmer carry\n1000 m run\n100 m sandbag lunges\n1000 m run\n100 Wall balls";
        let lines = parse(desc);
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
        let lines = parse(desc);
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
        let lines = parse(desc);
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
        let lines = parse("B. 20:00 AMRAP\n\"Vaya Con Dios\"\n2 Wall Walks\n16 Wall Balls");
        let names: Vec<&str> = lines.iter().map(|l| l.name_text.as_str()).collect();
        assert_eq!(names, vec!["Wall Walks", "Wall Balls"]);
    }

    #[test]
    fn enumerated_set_prescriptions_become_the_movements_ramp() {
        // Real 2026-09-24 Performance. The five "Set N:" lines are the prescription for the Push
        // Press above them: they set both the set count (5, not the 6 the "Every 2:30 x 15 Min"
        // header implies) and the per-set percentage ramp. Each one used to become a junk exercise
        // while the movement kept six flat, percentage-less sets — so the load silently fell back
        // to `load_calc`'s 50%-of-max default and no reps were prescribed at all.
        let lines = parse(
            "A. Every 2:30 x 15 Min\nPush Press\nSet 1: 8 Reps @ 60-65%\nSet 2: 7 Reps @ 65-70%\n\
             Set 3: 7 Reps @ 70-75%\nSet 4: 6 Reps @ 75%+\nSet 5: 6 Reps @ 75%+",
        );
        let names: Vec<&str> = lines.iter().map(|l| l.name_text.as_str()).collect();
        assert_eq!(names, vec!["Push Press"]);

        let got: Vec<(Option<i64>, Option<f64>)> = lines[0]
            .sets
            .iter()
            .map(|s| (s.reps, s.percent_1rm))
            .collect();
        assert_eq!(
            got,
            vec![
                (Some(8), Some(0.60)),
                (Some(7), Some(0.65)),
                (Some(7), Some(0.70)),
                (Some(6), Some(0.75)),
                (Some(6), Some(0.75)),
            ]
        );
    }

    #[test]
    fn a_max_reps_set_keeps_its_percentage_without_a_rep_count() {
        // Real 2026-09-25 Performance. "Max Reps" prescribes no number, but the 85% still applies.
        let lines =
            parse("A. Every 2:30 x 6 Sets\nBack Squat\nSet 1: 5 Reps @ 75%\nSet 2: Max Reps @ 85%");
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].sets.len(), 2);
        assert_eq!(lines[0].sets[1].reps, None);
        assert_eq!(lines[0].sets[1].percent_1rm, Some(0.85));
    }

    #[test]
    fn a_set_prescription_keeps_a_load_stated_on_the_movement_line() {
        let lines = parse("Deadlifts (225/155lb)\nSet 1: 4 Reps @ 80%");
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].sets.len(), 1);
        assert_eq!(lines[0].sets[0].explicit_weight_lb, Some(225.0));
        assert_eq!(lines[0].sets[0].percent_1rm, Some(0.80));
    }

    #[test]
    fn couplet_stations_split_into_one_exercise_per_movement() {
        // Real 2026-09-21 Fitness and HYROX. Each numbered station is two movements sharing one
        // slot. Unsplit, the whole line resolved to nothing and both tracks synced zero exercises.
        let lines = parse(
            "45/25 X 5 Rounds at same couplet\n1. Push ups/ TRX supinated row\n\
             2. DB skull crusher/ DB bird dog row\n3. SKI/ Plate wood chop",
        );
        let names: Vec<&str> = lines.iter().map(|l| l.name_text.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "Push ups",
                "TRX supinated row",
                "DB skull crusher",
                "DB bird dog row",
                "SKI",
                "Plate wood chop",
            ]
        );
        // Both halves of a station are performed every round, so each gets the full round count.
        assert!(lines.iter().all(|l| l.sets.len() == 5), "{lines:?}");
    }

    #[test]
    fn a_couplet_movement_may_begin_with_a_per_side_word() {
        // Real 2026-09-24 Fitness. "Side plank" is a movement, not a "10/side" qualifier — the
        // per-side guard must match a whole part, or this couplet silently refuses to split.
        let lines = parse("40/20 X 6 rounds at same couplet\n3. KB horn curl/ Side plank");
        let names: Vec<&str> = lines.iter().map(|l| l.name_text.as_str()).collect();
        assert_eq!(names, vec!["KB horn curl", "Side plank"]);
    }

    #[test]
    fn an_overloaded_slash_is_never_read_as_a_couplet() {
        // Every one of these is a single movement. A permissive split would shred them: the
        // Rx/scaled pair forms, the work/rest header, a per-side count, and "w/" for "with".
        for (line, want) in [
            ("50/35 Push-Ups", vec!["Push-Ups"]),
            ("1000/900m Row", vec!["Row"]),
            ("500/450m Row", vec!["Row"]),
            ("10/side Band woodchops", vec!["Band woodchops"]),
            ("Bike 20/15 Cals", vec!["Bike"]),
            ("15 DB Lunges w/ KB Press", vec!["DB Lunges w/ KB Press"]),
            (
                "20 Kettlebell Swings (American/Russian)",
                vec!["Kettlebell Swings (American/Russian)"],
            ),
        ] {
            let names: Vec<String> = parse(line).iter().map(|l| l.name_text.clone()).collect();
            assert_eq!(names, want, "for {line:?}");
        }
    }

    #[test]
    fn an_rx_scaled_distance_pair_takes_the_first_distance() {
        // Real 2026-09-24. "1000m Run" already worked; the paired spelling fuses the unit to the
        // second number, so it matched no rule and the whole line became the movement name.
        let lines = parse("500/450m Row\n1000/900m Ski");
        assert_eq!(lines[0].name_text, "Row");
        assert_eq!(lines[0].sets[0].distance_meters, Some(500));
        assert_eq!(lines[1].name_text, "Ski");
        assert_eq!(lines[1].sets[0].distance_meters, Some(1000));
    }

    #[test]
    fn a_section_letter_needs_no_space_before_its_header() {
        // Real 2026-09-23 Performance wrote "B.Time Cap: 17:00" with no space. Requiring one let
        // the whole header through as an exercise, while "B. Time Cap: 13:00" was dropped correctly.
        for desc in [
            "B.Time Cap: 17:00\n10 Thrusters",
            "B. Time Cap: 13:00\n10 Thrusters",
        ] {
            let names: Vec<String> = parse(desc).iter().map(|l| l.name_text.clone()).collect();
            assert_eq!(names, vec!["Thrusters"], "for {desc:?}");
        }
    }

    #[test]
    fn a_comma_separated_rep_ladder_is_a_scheme_not_an_exercise() {
        // Real 2026-09-21 Performance. The "-" spelling already worked; "," did not.
        let lines = parse("A. 35 Min for Quality\n20,18,16,14,12,10\nPush ups");
        let names: Vec<&str> = lines.iter().map(|l| l.name_text.as_str()).collect();
        assert_eq!(names, vec!["Push ups"]);
        let reps: Vec<Option<i64>> = lines[0].sets.iter().map(|s| s.reps).collect();
        assert_eq!(
            reps,
            vec![Some(20), Some(18), Some(16), Some(14), Some(12), Some(10)]
        );
    }

    #[test]
    fn a_rep_range_tolerates_a_space_before_its_dash() {
        // Real 2026-09-22 Fitness: "Min 3- 10 -12 DB step ups". The low end was taken as the rep
        // count and "-12 DB step ups" was left behind as the movement name.
        for desc in ["Min 3- 10 -12 DB step ups", "Min 3- 10-12 DB step ups"] {
            let lines = parse(desc);
            assert_eq!(lines[0].name_text, "DB step ups", "for {desc:?}");
            assert_eq!(lines[0].sets[0].reps, Some(10), "for {desc:?}");
        }
    }

    #[test]
    fn an_equipment_dimension_is_stripped_rather_than_read_as_a_load() {
        // Real 2026-09-22 Performance. "(24/20in)" is a box height: nothing Hevy can store, but
        // left glued to the name it stopped "Box Jumps" resolving, and it must not become 24 lb.
        let lines = parse("9 Box Jumps (24/20in)");
        assert_eq!(lines[0].name_text, "Box Jumps");
        assert_eq!(lines[0].sets[0].reps, Some(9));
        assert_eq!(lines[0].sets[0].explicit_weight_lb, None);
    }

    #[test]
    fn a_load_annotation_tolerates_a_space_after_its_paren() {
        // Real 2026-09-23 Performance wrote "Alt DB Snatch ( 50/35lb)".
        let lines = parse("20 Alt DB Snatch ( 50/35lb)");
        assert_eq!(lines[0].name_text, "Alt DB Snatch");
        assert_eq!(lines[0].sets[0].explicit_weight_lb, Some(50.0));
    }

    #[test]
    fn gym_announcements_and_structural_prose_are_not_exercises() {
        // Real 2026-09-25: a whole track of closure notices, plus the headers and transition
        // markers that surrounded real movements on the other days. None is a movement, and each
        // was previously reported as an unmapped exercise.
        for desc in [
            "NO CLASSES OFFERED AFTER 9AM",
            "HYROX ROOM Prep",
            "ROOM CLOSED 4PM - CLOSE",
            "CLASSES CLOSED:",
            "Come cheer on your community!",
            "6:30-12:40pm",
            "38 Min Cap",
            "B. 10 Min Coach choice core work :)",
            "Directly Into...",
            "A. 35 Min for Quality",
            "B. For Quality with time remaining",
        ] {
            assert!(
                parse(desc).is_empty(),
                "expected no exercises from {desc:?}"
            );
        }
    }

    #[test]
    fn partner_labels_are_stripped_and_keep_the_sections_replication() {
        // Real 2026-09-28 HYROX: a partner AMRAP where P1 runs while P2 works the chipper. The
        // labels used to stay glued to the movement ("P1- 200 m Run"), resolving to nothing.
        // "Chipper" names the format, and the four movements it refers to follow it.
        let lines = parse(
            "35 MIn AMRAP\nP1- 200 m Run\nP2- Chipper\n\n35 Cal Row\n35 Cal SKI\n\
             35 WB walking lunges\n35 Wall Balls",
        );
        let names: Vec<&str> = lines.iter().map(|l| l.name_text.as_str()).collect();
        assert_eq!(
            names,
            vec!["Run", "Row", "SKI", "WB walking lunges", "Wall Balls"]
        );
        assert_eq!(lines[0].sets[0].distance_meters, Some(200));
        // Partners work at the same time, so P1's run is replicated over the section exactly like
        // the unlabeled lines beneath it — not collapsed to one set the way a station label would.
        assert!(
            lines.iter().all(|l| l.sets.len() == lines[0].sets.len()),
            "{lines:?}"
        );
    }

    #[test]
    fn a_station_label_still_suppresses_replication_unlike_a_partner_label() {
        // The distinction PARTNER_LABEL_RE exists to preserve: "Min N-" enumerates one slot of a
        // rotating cycle and gets its count from the cycle, so it must not be replicated per round.
        let stationed = parse("EMOM X 4\nMin 1- 10 Push ups\nMin 2- Rest");
        assert_eq!(stationed.len(), 1);
        assert_eq!(stationed[0].name_text, "Push ups");
        assert_eq!(stationed[0].sets.len(), 2, "4 intervals / 2 stations");
    }

    #[test]
    fn min_to_complete_and_trailing_set_prescription_are_noise() {
        // "A2. 10 Min to Complete" is a section header, and a bare "5/5 x 4 Sets @ 40%" prescription
        // line is dropped (not a junk exercise). The movement between them is kept.
        let lines =
            parse("A2. 10 Min to Complete\nBack Rack Bulgarian Split Squats\n5/5 x 4 Sets @ 40%");
        let names: Vec<&str> = lines.iter().map(|l| l.name_text.as_str()).collect();
        assert_eq!(names, vec!["Back Rack Bulgarian Split Squats"]);
    }

    #[test]
    fn calorie_movement_without_split_resolves_name() {
        // "15 Cal Row" (no men's/women's split) -> "Row", flat 60s like other calorie targets.
        let lines = parse("15 Cal Row");
        assert_eq!(lines[0].name_text, "Row");
        assert_eq!(lines[0].sets[0].duration_seconds, Some(60));
    }

    #[test]
    fn every_x_n_sets_alternating_stations_divides_across_stations() {
        // "Alternating Stations" inverts the usual rule: the 12 sets are spread ACROSS the 3
        // stations (4 visits each), not performed 12 times at each one. Also strips "Station N:"
        // labels and drops the "... visits per station" annotations, including the typo'd one.
        let desc = "Every 3:00 x 12 Sets Alternating Stations\n36:00 tota - 4 visits per station \nStation 1: 45/36 Cal Ski\nStation 2: 35/24 Cal Bike \nStation 3: 45/36 Cal Row\n(4 visits per station, 36:00 total)";
        let lines = parse(desc);
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
        let lines = parse(desc);
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
        let lines = parse(desc);
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
        let lines = parse("B. Deadlift\n5-5-5-5-5");
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
        let lines = parse(desc);
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

    // ---- 2026-10-08 → 10-23 triage. Every description below is verbatim from SugarWOD. ----

    fn names(lines: &[ParsedExerciseLine]) -> Vec<&str> {
        lines.iter().map(|l| l.name_text.as_str()).collect()
    }

    fn reps(line: &ParsedExerciseLine) -> Vec<Option<i64>> {
        line.sets.iter().map(|s| s.reps).collect()
    }

    fn percents(line: &ParsedExerciseLine) -> Vec<Option<f64>> {
        line.sets.iter().map(|s| s.percent_1rm).collect()
    }

    #[test]
    fn sub_section_and_spaced_labels_are_still_headers() {
        // "A2." and "A ." each fell through as an exercise and left their section at one set.
        let lines = parse("A2.Every 2:00 x 3 Sets\n4 Push Press @ 75-80%");
        assert_eq!(names(&lines), vec!["Push Press"]);
        assert_eq!(reps(&lines[0]), vec![Some(4); 3]);
        assert_eq!(lines[0].sets[0].percent_1rm, Some(0.75));

        let lines = parse(
            "A . EMOM x 15:00 Min \nMin 1: 6/6 Front Foot Elevated Goblet Split Squat, For Load\nMin 2: 4 High Box Jumps (Choose a Challenging Height)\nMin 3: 50ft Sled Push, For Load\nChoice on Loads and Height. Challenging and Unbroken",
        );
        assert_eq!(
            names(&lines),
            vec![
                "Front Foot Elevated Goblet Split Squat",
                "High Box Jumps",
                "Sled Push"
            ]
        );
        assert!(
            lines.iter().all(|l| l.sets.len() == 5),
            "15 min / 3 stations"
        );
        assert_eq!(lines[2].sets[0].distance_meters, Some(15));

        // The prefix must not swallow a header's own leading number.
        let lines = parse("12:00 EMOM\nBench Press");
        assert_eq!(lines[0].sets.len(), 12);
    }

    #[test]
    fn numbered_set_lines_and_the_e_very_typo() {
        let lines = parse(
            "A. E very 2:00 Min  x 12 Min \nPush Press for load:\n#1: 7 reps\n#2: 5 reps\n#3: 3 reps\n#4: 7 reps\n#5: 5 reps\n#6: 3 reps",
        );
        assert_eq!(names(&lines), vec!["Push Press"]);
        assert_eq!(reps(&lines[0]), [7, 5, 3, 7, 5, 3].map(Some).to_vec());
    }

    #[test]
    fn a_lettered_work_rest_header_replicates_its_stations() {
        // 2026-10-19 / 10-22: reported as synced while every station collapsed to one set.
        let lines =
            parse("A. 30/30 X 5 rounds at same station\n\n1. DB Push Press\n2. KB Farmer Walk");
        assert_eq!(names(&lines), vec!["DB Push Press", "KB Farmer Walk"]);
        assert!(lines.iter().all(|l| l.sets.len() == 5));

        let lines = parse("C. 40/20 x 8 Rounds \n1. Flutter Kicks \n2. Leg Lifts");
        assert_eq!(names(&lines), vec!["Flutter Kicks", "Leg Lifts"]);
        assert!(lines.iter().all(|l| l.sets.len() == 8));
    }

    #[test]
    fn a_run_of_rep_lines_lists_the_sets_of_the_lift_above() {
        let lines = parse(
            "A.Take 15:00 to Establish a 3RM Deadlift\nSuggested loading and jumps for 3RM for the day.\n5 Reps @ 50%\n4 Reps @ 60%\n3 Reps @ 65-70%\n3 Reps @ 70-75%\n2 Reps @ 80-85%\n1-2 Reps @ 90%",
        );
        assert_eq!(names(&lines), vec!["Deadlift"]);
        assert_eq!(reps(&lines[0]), [5, 4, 3, 3, 2, 1].map(Some).to_vec());
        assert_eq!(
            percents(&lines[0]),
            [0.5, 0.6, 0.65, 0.7, 0.8, 0.9].map(Some).to_vec()
        );
    }

    #[test]
    fn a_single_rep_line_restates_every_set_of_the_lift_above() {
        let lines = parse("A.  12:00 EMOM\nBench Press \n3 Reps 65-75% ");
        assert_eq!(names(&lines), vec!["Bench Press"]);
        assert_eq!(reps(&lines[0]), vec![Some(3); 12]);
        assert_eq!(percents(&lines[0]), vec![Some(0.65); 12]);
    }

    #[test]
    fn a_rep_line_under_a_new_header_adds_sets() {
        let lines = parse("A. 7:00 EMOM\n2 Snatch  @ 70%\n\nA 2. 7:00 EMOM\n1 Rep @ 75%");
        assert_eq!(names(&lines), vec!["Snatch"]);
        let mut want = vec![Some(2); 7];
        want.extend(vec![Some(1); 7]);
        assert_eq!(reps(&lines[0]), want);
        assert_eq!(lines[0].sets[13].percent_1rm, Some(0.75));
    }

    #[test]
    fn a_percentage_of_another_lift_is_not_applied() {
        let lines = parse(
            "A. Every 3:00 x 4 Sets\nBack Rack Lunges\n8/8 Reps @  40-45% of 1RM Back Squats",
        );
        assert_eq!(names(&lines), vec!["Back Rack Lunges"]);
        assert_eq!(reps(&lines[0]), vec![Some(8); 4]);
        assert_eq!(percents(&lines[0]), vec![None; 4]);
    }

    #[test]
    fn a_complex_and_its_rep_line() {
        let lines = parse(
            "A. Every 2:30 x 15 Min \nHigh Hang Clean + Hang Clean + Low Hang Clean\n1+1+1 @ 60-65%",
        );
        assert_eq!(
            names(&lines),
            vec!["High Hang Clean", "Hang Clean", "Low Hang Clean"]
        );
        for line in &lines {
            assert_eq!(reps(line), vec![Some(1); 6]);
            assert_eq!(percents(line), vec![Some(0.6); 6]);
        }
    }

    #[test]
    fn multi_minute_stations_and_plus_combos() {
        // EMOM X 30 over minutes 1, 2-3, 4 and 5-6: a 6-minute cycle, so 5 rounds — counting
        // "Min 2 and 3" as one minute would have made it 7.
        let lines = parse(
            "EMOM X 30\n\nMin 1- 6 DB SA alt devils press\nMin 2 and 3- 400 m run\nMin 4- 6/  DB SA thruster\nMin 5 and 6- 25 Sit ups + 50 DUs",
        );
        assert_eq!(
            names(&lines),
            vec![
                "DB SA alt devils press",
                "run",
                "DB SA thruster",
                "Sit ups",
                "DUs"
            ]
        );
        assert!(lines.iter().all(|l| l.sets.len() == 5), "{lines:?}");
        assert_eq!(lines[1].sets[0].distance_meters, Some(400));
        assert_eq!(lines[4].sets[0].reps, Some(50));
    }

    #[test]
    fn bare_station_lines_group_the_movements_under_them() {
        let lines = parse(
            "B. Every 5:00 x 4 Sets, Alternating Stations\nStation 1\n500/450m Ski Erg\n8 Chest to Bar Pull-Ups + 12 Pull-Ups\n\nStation 2\n500/450m Row\n15  Push-Ups",
        );
        assert_eq!(
            names(&lines),
            vec![
                "Ski Erg",
                "Chest to Bar Pull-Ups",
                "Pull-Ups",
                "Row",
                "Push-Ups"
            ]
        );
        assert!(
            lines.iter().all(|l| l.sets.len() == 2),
            "4 sets / 2 stations"
        );
    }

    #[test]
    fn timed_rest_lines_attach_to_the_movement_above() {
        let lines = parse("4 Sets: \n400m Run\nRest 1:00 b/t sets");
        assert_eq!(names(&lines), vec!["Run"]);
        assert_eq!(lines[0].sets.len(), 4);
        assert_eq!(lines[0].rest_seconds_after, Some(60));

        let lines = parse("15/12 Calorie Bike Erg\nRest 2:00 between sets");
        assert_eq!(names(&lines), vec!["Bike Erg"]);
        assert_eq!(lines[0].rest_seconds_after, Some(120));
    }

    #[test]
    fn round_counts_inside_longer_headers() {
        for (desc, rounds) in [
            ("B. 5 Rounds- 15 Min Cap\n200m Run", 5),
            (
                "B. For Time · 4 Rounds - Cap 12 Min\n8 Hang Power Snatches",
                4,
            ),
            ("Time Cap\n35:00: (Teams of 2) · 4 Rounds\n40 Wall Balls", 4),
            ("B 15 Min For Quality\n3 Sets\n10 Goblet Cossack Squats", 3),
            ("B. 4-5 sets of:\n\n8 DB chest press", 4),
        ] {
            let lines = parse(desc);
            assert_eq!(lines.len(), 1, "{desc:?} -> {lines:?}");
            assert_eq!(lines[0].sets.len(), rounds, "{desc:?}");
        }
    }

    #[test]
    fn a_leading_ladder_covers_every_bare_movement_in_its_section() {
        let lines = parse(
            "A. 10 Min AMRAP\n\n20,18,16...\nDB push press\nTRX low row\n\nB. 10 minute AMRAP\n\nDB curls",
        );
        assert_eq!(
            names(&lines),
            vec!["DB push press", "TRX low row", "DB curls"]
        );
        let ladder = [20, 18, 16].map(Some).to_vec();
        assert_eq!(reps(&lines[0]), ladder);
        assert_eq!(reps(&lines[1]), ladder);
        assert_eq!(
            reps(&lines[2]),
            vec![None; AMRAP_ESTIMATE_DEFAULT_ROUNDS as usize],
            "the ladder belongs to section A"
        );
    }

    #[test]
    fn distances_durations_and_annotations_come_off_the_name() {
        for (line, name, meters) in [
            ("200ft Dumbbell Farmers Carry", "Dumbbell Farmers Carry", 61),
            (
                "50/50ft (15/15m) Single Arm Kettlebell Overhead Carry",
                "Single Arm Kettlebell Overhead Carry",
                15,
            ),
            ("100ft OH DB Walk ", "OH DB Walk", 30),
            ("100-200 m run", "run", 100),
        ] {
            let lines = parse(line);
            assert_eq!(names(&lines), vec![name], "{line:?}");
            assert_eq!(lines[0].sets[0].distance_meters, Some(meters), "{line:?}");
        }

        let lines = parse(":15/:15 Copenhagen Plank");
        assert_eq!(names(&lines), vec!["Copenhagen Plank"]);
        assert_eq!(lines[0].sets[0].duration_seconds, Some(15));

        let lines = parse("20 Single DB Step-Overs  (50/35lb) /(24/20')");
        assert_eq!(names(&lines), vec!["Single DB Step-Overs"]);
        assert_eq!(lines[0].sets[0].reps, Some(20));
        assert_eq!(lines[0].sets[0].explicit_weight_lb, Some(50.0));

        let lines = parse("15/12 Calorie Ski");
        assert_eq!(names(&lines), vec!["Ski"]);
        assert_eq!(lines[0].sets[0].reps, None, "calories are not reps");
    }

    #[test]
    fn coaching_prose_lines_are_noise() {
        let lines = parse(
            "Back Squat\nPercentages are based off a 1RM Back Squat.\nSuggested loading and jumps for 3RM for the day.\nBox Height: 24/20in\nTime Cap\nRest 2:00",
        );
        assert_eq!(names(&lines), vec!["Back Squat"]);
    }
}
