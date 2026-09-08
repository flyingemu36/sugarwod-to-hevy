# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
- `CONTRIBUTING.md`, `SECURITY.md`, `CODE_OF_CONDUCT.md`, issue and PR templates, Dependabot.

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
- The Hevy secret id moved from the hardcoded `prod/hevy/api_key` to configurable
  `hevySecretId`, defaulting to `sugarwod-to-hevy/hevy-api-key`.
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
