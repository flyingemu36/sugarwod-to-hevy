# Design & Architecture

This document explains how SugarWOD → Hevy is built and *why* it's built that way. For what it
does and how to run it, see the [README](../README.md).

## The problem

My gym posts each day's Workout of the Day (WOD) to [SugarWOD](https://www.sugarwod.com/) as
free-text from a pre-programmed purchased coaching setup. In other words, it's purchased from
SugarWOD or somewhere else with text like `"5:00 EMOM"`, `"2 Push Press @ 65%"`,
`"12/9 Cal Row"`, `"3-Person Team Waterfall"`. I train from [Hevy](https://www.hevyapp.com/),
which wants *structured* routines: exercises resolved to its catalog, sets and reps as numbers,
weights in pounds, distances in meters. Retyping the WOD into Hevy every morning at 5AM is 
exactly the kind of tedious, error-prone translation a computer should do. This system does it 
automatically, before I leave for the gym, with weights pre-filled from my own lift history.

In essence, I'd like to stop spending 5 minutes typing in things, going into my calculator app, 
etc, every weekday morning and spend more time stretching and getting ready to lift. Spending 5 
minutes a day, 5 days a week, for 45+ weeks is like ~18-19 hours a year typing in things that a
set of Lambdas can do in a matter of milliseconds.

## Design principles

Everything below follows from a handful of deliberate choices:

- **Single-tenant, personal, boring-reliable.** This runs for one person once a day. The design
  optimizes for "the right workout is sitting in Hevy every morning with zero intervention," not
  for scale or throughput. Simplicity beats cleverness everywhere.
- **Rust for logic, TypeScript for infra.** The business logic (parsing, matching, load math)
  lives in Rust. No particular reason except that I wanted to learn Rust after a decade++ in
  Java and Python. I guess there's also fast cold starts on Lambda, a strong type system for 
  a domain full of fiddly edge cases, and exhaustive unit-testability. Infrastructure is 
  AWS CDK in TypeScript because I do all of my CDK in typescript at work; the two 
  languages meet only at the Lambda boundary.
- **One Lambda per job, no orchestration.** Each job is a single, self-contained invocation with
  internal retry/backoff around outbound HTTP. There's no branching or fan-out that would
  justify Step Functions, so there isn't any. Would this work in Step-Functions? Absolutely. 
  Are step-functions overkill? Yes. I love using step functions, especially on super-long,
  complex problems with multiple branches. This ain't it.
- **Least privilege, verified against the code.** Every IAM grant lists the exact DynamoDB
  actions the code actually calls.
- **Secrets never live in code or CloudFormation.** The Hevy API key is in Secrets Manager; the
  Discord webhook URL, when notifications are enabled at all, is an SSM SecureString written by
  hand. CDK references both by name only.
- **The live API is ground truth, not the docs.** Hevy's published OpenAPI spec has drifted from
  its real wire behavior in several places. Every integration is verified against the live
  service and coded to match reality (see [Hard-won invariants](#hard-won-invariants)).

## System architecture

```
EventBridge Scheduler (timezone-aware; zone and times come from config)
 ├─ 3:30 AM daily      ──▶ sync-wod         (Lambda, Rust, arm64)
 ├─ 3:00 AM Sunday     ──▶ refresh-catalog  (Lambda, Rust, arm64)
 └─ every 2 days       ──▶ refresh-maxes    (Lambda, Rust, arm64)

sync-wod (the daily job):
  today's date in the configured zone → GET SugarWOD workouts (may return >1 for the day)
  → for each workout: parse description → resolve exercises → compute loads
  → upsert its Hevy routine (idempotent, keyed per workout via RoutineMap)
  → optionally post one Discord summary covering the day's workouts (off by default)

refresh-catalog (weekly):
  paginate GET Hevy /v1/exercise_templates → bulk-write HevyExerciseCatalog

refresh-maxes (every 2 days):
  enumerate template ids in use → GET Hevy /v1/exercise_history/{id} per id
  → max(weight_kg) per exercise → write PersonalMax
```

Three independent jobs, each on its own schedule, each doing one thing. `sync-wod` is the
product; `refresh-catalog` and `refresh-maxes` keep the reference data it depends on fresh.

### Why one Lambda per job

The daily job is a straight-line pipeline: fetch → parse → resolve → upsert → notify. There's no
conditional branching, no parallel fan-out, no step that needs to be independently retried with
different logic. An orchestrator (Step Functions, a state machine) would add nesting, IAM
surface, and cross-service payload marshalling for zero benefit. Transient HTTP failures are
handled *inside* the Lambda by a small backoff wrapper around SugarWOD/Hevy calls — the one place
retries actually make sense. (Notably, **non-idempotent writes are never wrapped in retry** — see
[Hard-won invariants](#hard-won-invariants).)

## Repository layout

```
SugarwodToHevy/
├── rust/                 Cargo workspace — all business logic
└── cdk/                  TypeScript CDK app — all infrastructure
```

### Rust workspace (`rust/`)

The workspace separates a fully-testable core library from three thin Lambda entry points:

```
rust/crates/
├── swh-core/                        # shared library — NO lambda_runtime dep, unit-testable
│   └── src/
│       ├── config.rs                # required env vars — no fallback defaults, ever
│       ├── date.rs                  # "today" in a given timezone → YYYYMMDD (chrono-tz)
│       ├── error.rs                 # thiserror SwhError enum
│       ├── units.rs                 # lb↔kg, round-to-nearest(increment)
│       ├── retry.rs                 # backoff wrapper — outbound HTTP only, never writes
│       ├── secrets.rs               # Secrets Manager: $HEVY_SECRET_ID → {"api_key": "..."}
│       ├── discord.rs               # webhook summary / failure messages
│       ├── sugarwod/{client,jsonp,model}.rs
│       ├── hevy/{client,model,routine_builder}.rs
│       ├── matching/
│       │   ├── normalize.rs         # canonical text form for all lookup keys
│       │   ├── description_parser.rs# mode-aware: EMOM / team-AMRAP / plain lines
│       │   ├── correlator.rs        # optional movements[] corroboration
│       │   ├── resolver.rs          # alias → catalog GSI → unmapped
│       │   └── load_calc.rs         # %1RM → weight_kg via PersonalMax + rounding
│       └── store/{alias_table,catalog_table,routine_map,personal_max}.rs
├── sync-wod/src/main.rs             # lambda_runtime::run — thin, delegates to swh-core
├── refresh-catalog/src/main.rs      # thin
└── refresh-maxes/src/main.rs        # thin
```

The key design decision: **`swh-core` has no `lambda_runtime` dependency and no AWS runtime
assumptions in its logic modules.** The store layer is expressed as small traits
(`AliasStore`, `CatalogStore`, `MaxStore`, …) so the entire matching/load pipeline can be
unit-tested against in-memory `HashMap` doubles — no DynamoDB, no network. Each binary is a
few dozen lines: wire up AWS SDK clients, read env-var config, call into `swh-core`.

### CDK app (`cdk/`)

Four stacks, composed in `bin/sugarwod_to_hevy.ts`. Splitting by lifecycle (data outlives
compute; monitoring watches everything) keeps blast radius small and lets stateful and stateless
resources have different removal policies.

- **`config.ts`** — not a stack, but the single source of truth for everything deployment-
  specific: the gym affiliate, timezone, resource prefix, secret id, schedule expressions, and
  whether Discord notifications are on. Read from CDK context, so nothing under `lib/` hardcodes
  a value that belongs to one person's setup. `affiliateId` has no default and throws at synth
  if unset — a default there would silently sync someone else's gym into your Hevy account.
  Physical resource names are built here too, and they are all prefixed and stage-suffixed
  because DynamoDB tables, schedule groups, dashboards and SSM paths share one namespace per
  account.
- **`DataStack`** — the four DynamoDB tables, named via `config.ts`. It *declares* the Discord
  webhook parameter name for IAM scoping but never owns its value — a SecureString value can't
  be safely managed by CloudFormation, so it's written once by hand.
- **`ComputeStack`** — the three Rust Lambdas, built via the `cargo-lambda-cdk` `RustFunction`
  construct (which wraps `cargo lambda build --release` into CDK asset bundling), running on
  `PROVIDED_AL2023` / `arm64`. All IAM is per-function least-privilege (below). In the gamma
  stage it also defines the integration-test role.
- **`ScheduleStack`** — three EventBridge Scheduler schedules with
  `scheduleExpressionTimezone` set from config (so DST is handled automatically, unlike a
  bare UTC cron). Each schedule sits in its **own** ScheduleGroup, because Scheduler's CloudWatch
  metrics are dimensioned by group, not by schedule name — one group per schedule is the only way
  the dashboard can answer "did *this specific* job fire today." Prod only; gamma is invoked
  manually.
- **`MonitoringStack`** — a CloudWatch dashboard and alarms over the Lambdas, the DynamoDB
  tables, and (via those per-schedule groups) Scheduler's own invocation health. Prod only.

### Stages: prod and gamma

The same CDK code deploys two stages via a `stage: 'prod' | 'gamma'` prop threaded through the
stacks:

- **prod** — the real daily driver: automatic schedules, monitoring dashboard, DynamoDB tables
  with `RETAIN` removal policy (the data is curated real state).
- **gamma** — a same-account, `-Gamma`-suffixed twin for integration testing: no schedules
  (invoked manually), `DESTROY` removal policy (disposable fixtures), and its own Discord
  webhook parameter if notifications are enabled at all. It shares the one real Hevy account
  (there's only one), isolating itself with test-prefixed routine keys rather than a second
  account.

Both stages emit `CfnOutput`s (table names, function names, webhook parameter name) so tooling
resolves resource names at runtime instead of hardcoding them.

## Data model

Four DynamoDB tables, all `PAY_PER_REQUEST` (traffic is a handful of requests a day). They are
referred to below by logical name; the physical names carry the resource prefix and stage
(`SugarwodToHevy-ExerciseAlias-Prod`), since the DynamoDB namespace is per account.

**`HevyExerciseCatalog`** — a local mirror of Hevy's exercise-template catalog.
PK `id` (opaque Hevy template id). A `TitleNormalizedIndex` GSI on `title_normalized` turns
name lookups into point `Query`s instead of full-table scans. Other attributes: `title`, `type`
(`weight_reps`, `reps_only`, `duration`, `distance_duration`, … — used to tell loaded from
bodyweight movements), `equipment`, muscle groups, `updated_at`. Refreshed weekly by
`refresh-catalog`.

**`ExerciseAlias`** — the translation dictionary from messy real-world names to Hevy exercises.
PK `alias` (a normalized SugarWOD text fragment or movement name). Attributes: `canonical_title`,
`hevy_template_id` (denormalized, so one `GetItem` resolves an alias straight to a Hevy id),
`source` (`seeded` / `manual` / `auto-resolved`). A `canonical_title` of `__SKIP__` is an explicit
"this text isn't an exercise" marker — kept distinct from a genuine unmapped miss so noise doesn't
get reported as a gap.

**`RoutineMap`** — the idempotency key for upserting routines. PK `routine_key`, a
**word-order-independent** normalized workout title (lowercase, split to words, drop filler like
`+`/`and`, sort tokens, join with `-`; so `"Performance + Fitness"` and `"Fitness + Performance"`
both map to `performance-fitness`). Attributes: `hevy_routine_id`, `last_synced_title`,
`updated_at`. This is what lets `sync-wod` update the *same* Hevy routine in place day after day
instead of creating duplicates, and it works correctly even when SugarWOD posts multiple distinct
workouts on one date (each gets its own key and its own persistent routine).

**`PersonalMax`** — derived strength numbers. PK `exercise_template_id`. Attributes: `title`,
`max_weight_kg`, `source_workout_id` (traceability), `updated_at`. Populated by `refresh-maxes`
as the heaviest weight ever logged for that template in my Hevy history.

## Matching pipeline (`swh-core::matching`)

This is the heart of the system: turning one workout's free-text `description` into structured,
resolved, load-calculated Hevy exercises. It runs once per workout SugarWOD returns.

### 1. Mode-aware description parsing

The description is line-split, noise headers are skipped (equipment blurbs, `"A For Time:"`,
etc.), and the parser tracks a per-section **interval mode** that following lines inherit until a
new header changes it:

- **`Default`** — each line means exactly what it says. Handled shapes: distance-first
  (`"400m Run"`), dash rep-series (`"Back Squat 12-10-8"`), reps-first (`"40 Toes to Bar"`),
  bare rep-scheme header (`"21-15-9"`, applied to following lines), set×rep (`"Bench Press 5x5"`),
  parenthetical weight (`"(95/65)"`), and RPE suffix (appended to notes — Hevy has no RPE field).
  A line matching no numeric pattern is captured as a bare exercise name rather than silently
  dropped.
- **`Emom { rounds }`** — a `"N:00 EMOM"` line sets this mode; each following exercise becomes
  `rounds` identical sets (one round per minute). A `"-Rest M:00-"` line attaches
  `rest_seconds = M*60` to the *previous* exercise instead of becoming an exercise itself.
- **`AmrapEstimate { rounds }`** — a `"Team … Waterfall/AMRAP"` header sets this mode with a
  conservative default round count (trivially adjusted in the Hevy app). The bare time-cap line
  that follows (`"24:00 AMRAP"`) is treated as noise, not an exercise.
- **X/Y (men's/women's) values** — `"12/9 Cal Row"` and `"12/9 Push-Ups"` both take the first
  (men's Rx) number. Calorie-target cardio has no Hevy equivalent, so it logs as a flat
  60-second, 0-distance interval regardless of the calorie count (trued up in the gym).

### 2. Optional `movements[]` correlation

SugarWOD sometimes includes a structured `movements[]` array — but in practice it's frequently
empty, and when populated it's often unrelated to the day's actual content (it behaves like an
affiliate's video-tag list). So the pipeline treats it as a **purely optional corroborating
signal**: parsed lines are correlated against `movements[]` entries by token overlap, an
uncorrelated entry is discarded, and **every line still resolves independently** if `movements[]`
is empty. The empty case is the common case, not an edge case.

### 3. Identity resolution

Per line: use the correlated movement's name if step 2 found one, else the line's own parsed
text → `ExerciseAlias` `GetItem` → on miss, `HevyExerciseCatalog` GSI `Query` on
`title_normalized`. A catalog hit writes back a new `source="auto-resolved"` alias row, so the
dictionary compounds with use. A still-miss is collected as **unmapped** — surfaced in the run
summary, never silently dropped.

### 4. Load calculation (`load_calc.rs`)

Once a line resolves to a template id:

- **Explicit `@N%`** (`"2 Push Press @ 65%"`) → look up `PersonalMax`, take `N%` of it, round to
  the nearest 5 lb, store as kg.
- **Loaded type, no explicit load** (`"12 Alt DB Hang Snatch"` on a `weight_reps` movement) →
  apply a sensible default percentage of max.
- **Bodyweight / cardio type** → no weight applied.
- **No max on record** for a movement that needs one → leave weight empty, note
  `"no known max — set weight manually"`, and collect it into a **load-unknown** report (distinct
  from the unmapped report).

### 5. Build & report

The resolved `(template_id, sets, rest_seconds)` triples become the Hevy routine payload. Both the
unmapped and load-unknown lists feed the run summary, so gaps in `ExerciseAlias` or
`PersonalMax` surface as a natural side effect of daily use rather than silent degradation.

## Personal max derivation (`refresh-maxes`)

Maxes come from Hevy itself, not hand entry — I already log real lifts there. Rather than
paginating my entire workout history, `refresh-maxes` (1) scans `ExerciseAlias` for the distinct
set of template ids actually referenced by WOD translation (dozens, not the full ~1000-exercise
catalog), (2) calls Hevy's dedicated `GET /v1/exercise_history/{id}` endpoint once per id, taking
the max `weight_kg` across entries, and (3) writes the results to `PersonalMax`. This is bounded
by what's in use and uses exact per-exercise data instead of reducing over nested
workout/exercise/set arrays.

## IAM (least privilege)

Every grant is built with a `grantExact` helper that lists precise DynamoDB actions — CDK's
`grantReadWriteData`-style helpers grant far more (`Scan`, `DeleteItem`, `UpdateItem`) than any
function calls. The grants are verified against the exact SDK calls in `swh-core::store::*`:

- **`sync-wod`** — `GetItem`+`PutItem` on `ExerciseAlias`; `GetItem`+`Query` (no `Scan`) on
  `HevyExerciseCatalog` and its GSI; `GetItem` on `PersonalMax`; `GetItem`+`PutItem` on
  `RoutineMap`; `GetSecretValue` scoped to the Hevy secret; and `ssm:GetParameter` on the webhook
  parameter *only when Discord notifications are enabled* — with them off, the grant is absent
  entirely rather than present and unused.
- **`refresh-catalog`** — `BatchWriteItem` on `HevyExerciseCatalog` only; the scoped secret read.
  No access to any other table.
- **`refresh-maxes`** — `Scan` on `ExerciseAlias` (to enumerate ids in use), `GetItem` on
  `HevyExerciseCatalog` (title recovery), `PutItem` on `PersonalMax`; the scoped secret read.
- Each schedule gets its own role with `lambda:InvokeFunction` scoped to exactly one function ARN.

**Integration-test role (gamma only).** The gamma integration suite runs under a dedicated role
scoped to exactly the *union* of the three Lambda grants — deliberately **not** developer/admin
credentials. The whole point of testing against a deployed stack is to catch "works with my local
credentials but the deployed role is missing a permission" bugs, which an admin identity can never
surface. It runs under a real least-privilege boundary.

## Testing strategy

Three layers, deepest and cheapest first:

1. **Unit tests (`cargo test`, no AWS, no network).** The core of the suite. Every parser line
   shape has a case asserting the exact parsed struct; the resolver, correlator, load-calc
   rounding, routine builder, JSONP unwrapping, and routine-key normalization are all tested
   against in-memory store doubles and real captured fixtures. This is where correctness lives.
2. **Integration tests against gamma (`cargo test -- --ignored`, serial).** Real DynamoDB,
   Secrets Manager, SSM, live SugarWOD, and live Hevy **reads** against the deployed `-Gamma`
   stack — the layer that catches IAM and wire-format bugs unit tests can't. Deliberately no
   automated Hevy `POST`/`PUT` (those have side effects on the one real account).
3. **Manual acceptance.** For a single-tenant system whose output is "does the Hevy app show the
   right workout," the final check is looking at it — verifying EMOM set counts, AMRAP rounds, and
   computed %1RM weights on a real run. `cargo lambda watch` + `cargo lambda invoke` cover local
   smoke tests; test-prefixed routine keys keep manual runs off the real daily routines.

## Hard-won invariants

A few non-obvious rules the design encodes, each learned the expensive way and worth stating
plainly so they're not "cleaned up" by a future change:

- **Never wrap a non-idempotent write in retry.** `create_routine` (`POST`) is *not* retried:
  if the server accepted the write but the response failed to parse, a blind retry creates a
  duplicate routine. Retry belongs around idempotent reads only; a failed write surfaces directly.
- **Trust the live API over the published spec.** Hevy's OpenAPI spec disagrees with its real
  wire behavior in at least three places (the `equipment` field's name, and the response
  envelopes for `POST` vs `PUT /v1/routines` — which are wrapped differently, one even nesting the
  object inside a single-element array). The client tolerates all observed shapes and is coded to
  the live responses, not the docs.
- **Include the raw response body in error messages.** Wire-format surprises are only diagnosable
  if the failing payload is in the error. Stringify-only error mapping hides exactly the detail
  you need.
- **Timezone-aware scheduling, always.** EventBridge Scheduler with an explicit timezone handles
  DST correctly; a bare UTC cron drifts twice a year. Rate-based schedules omit an explicit start
  date so they anchor to deploy time (Scheduler rejects a start date more than 5 minutes in the
  past anyway).
