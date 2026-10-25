# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed

- **A separator line crashed the whole day's sync.** A WOD line of just `-` or `+` normalized to an
  empty alias key, which DynamoDB rejects; the error aborted every track (2026-10-15, 10-21). Such
  keys are now skipped without a lookup.
- **Each failure was posted to Discord three times.** Lambda's own async retries are disabled on
  `sync-wod`; one failing track no longer aborts the others; DynamoDB errors keep their full source
  chain instead of reading "service error".
- **Silent wrong output, reported as success:**
  - lettered work/rest headers ("A. 30/30 X 5 rounds at same station") collapsed every station to
    one set;
  - loaded carries (`short_distance_weight`) never got a weight;
  - a title shared by a built-in and a custom template resolved to either one at random, in both
    `sync-wod` and the alias seed script;
  - a logged-weight typo (161 kg DB Thruster) became the max — `refresh-maxes` now rejects outliers.
- **Prescriptions the parser read as exercises** now apply to the movement they describe:
  `Set N:` / `#N:` set lists, rep lines ("5 Reps @ 50%", "3 Reps 65-75%", "1+1+1 @ 60%"), timed
  rest lines, round counts inside headers ("5 Rounds- 15 Min Cap", "For Time · 4 Rounds"),
  sub-section labels ("A2.", "A 2.", "A ."), multi-minute EMOM stations ("Min 2 and 3-"), bare
  "Station N" groups, " + " combos, feet distances, ":15/:15" holds and spelled-out "Calorie".
- `normalize` drops apostrophes instead of splitting on them, so "Devils Press" reaches "Devil's
  Press". Re-run `refresh-catalog` after deploying to rewrite the stored keys.

### Added

- Resolver: fuzzy candidates are also checked against the alias table; leading qualifiers
  (`Alt`, `Max`, `Heavy`, `Strict`, `SA`) are dropped as a fallback; `walk` and `carry` are
  interchangeable.
- Hevy templates that cannot hold part of a prescription are reported in the summary instead of
  silently dropping it.
- `examples/replay_parse.rs` and `examples/replay_resolve.rs`: offline replay of saved SugarWOD
  responses against read-only snapshots of the Prod tables, with `--seed` to preview dictionary
  changes before seeding.
- 60 new or corrected entries in `exercise-translation.json`.

## [0.1.0] — 2026-09-07

Initial public release. The pipeline itself has been running privately as a daily driver; this
release is the work of making it something other people can read, verify, and deploy.

### Added

- `cdk/lib/config.ts`: all deployment settings (gym affiliate, timezone, resource prefix, secret
  id, schedule expressions) now come from CDK context. `affiliateId` is required with no default,
  and `cdk synth` fails with an actionable error when it is missing.
- `swh-core::config`: matching runtime configuration for the Lambdas, read from environment
  variables with no fallback defaults.
- CI (GitHub Actions): `cargo fmt`/`clippy -D warnings`/`test`, `tsc --noEmit`, and `cdk synth`
  both with and without required config, so the config gate itself is regression-tested.
- `cargo-deny` license gate, guarding AGPL compatibility of the dependency tree.
- CI cross-compiles the Lambdas for `aarch64` during `cdk synth`, so a change that builds on an
  x86_64 developer machine but not for the deployed architecture fails in CI rather than at
  deploy time. Requires Zig, which the workflow installs explicitly.
- `CONTRIBUTING.md`, `SECURITY.md`, `CODE_OF_CONDUCT.md`, issue and PR templates, Dependabot.

### Requirements

- **Node.js 26+** for the CDK app. Node 20 reached end-of-life on 2026-04-30 and Node 18 in
  April 2025; `@types/node` tracks the runtime so `tsc` cannot bless APIs that are absent at
  execution time. The Rust workspace builds on stable.

### Changed

- **Discord notifications are opt-in**, behind the `discordEnabled` CDK context flag and off by
  default. They are a convenience rather than part of syncing; on by default would mean every
  deployment either warns on every run or needs an SSM SecureString created before it works. When
  off, `sync-wod` is not given the webhook parameter name and its execution role is not granted
  `ssm:GetParameter` at all — the capability is absent rather than merely unused.
- **Licensed under AGPL-3.0-or-later** (previously MIT, never published). AGPL rather than GPL
  because the software is deployed as a service and never distributed as a binary, so GPL's
  copyleft would never trigger for a modified fork.
- **All account-global resource names are now prefixed and stage-scoped.** DynamoDB tables were
  bare (`ExerciseAlias`, `PersonalMax`, …), as were the EventBridge schedule groups, the
  CloudWatch dashboard, the alarm, and the SSM parameter path. Deploying alongside anything else
  using those names would have collided with, or `RETAIN`-orphaned, unrelated resources.
- `date::today_pacific_yyyymmdd()` → `date::today_yyyymmdd_in(tz)`, driven by `LOCAL_TIMEZONE`.
- The Hevy secret id is configurable via `hevySecretId`, defaulting to
  `sugarwod-to-hevy/hevy-api-key`, rather than a hardcoded constant.
- The Discord webhook parameter path is now uniform across stages:
  `/<slug>/<stage>/discord-webhook-url` (prod previously omitted the stage segment).
- `seed-aliases` now requires both table names explicitly instead of defaulting to bare names.

### Fixed

- **`sync-wod` no longer fails silently when Discord notification breaks.** Both the SSM parameter
  fetch and the webhook post were discarded with `if let Ok(..)` / `let _ = ..`, so a missing
  parameter produced no message *and* no log line. Failures stay non-fatal — a broken webhook must
  not fail a sync that already wrote to Hevy — but are now logged with the reason.
- Six clippy lints, so `-D warnings` passes.

### Migration notes

Upgrading an existing deployment renames every DynamoDB table. Because prod tables use a `RETAIN`
removal policy, the old tables are **orphaned rather than deleted** — no data is lost, but the new
tables start empty. Run `cdk diff` first, then repopulate: `refresh-catalog`, `refresh-maxes`, and
`npm run seed-aliases`. `RoutineMap` is the one table worth copying over by hand if you want
existing Hevy routines updated in place rather than recreated.

[Unreleased]: https://github.com/flyingemu36/sugarwod-to-hevy/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/flyingemu36/sugarwod-to-hevy/releases/tag/v0.1.0
