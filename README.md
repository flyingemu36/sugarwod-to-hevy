# SugarWOD → Hevy

Serverless pipeline that pulls your gym's daily Workout of the Day (WOD) from
[SugarWOD](https://www.sugarwod.com/) and turns it into a ready-to-run routine in
[Hevy](https://www.hevyapp.com/), every morning before you train — with weights auto-filled from
your own lift history.

It runs entirely on AWS (Lambda + DynamoDB + EventBridge Scheduler), provisioned with CDK. The
Lambda handlers are written in Rust; the infrastructure is TypeScript CDK.

## What it does

Every morning at 3:30 AM local time (a buffer before an early training slot):

1. Fetch today's `workout-of-the-day` from the SugarWOD public API for the configured affiliate.
2. Parse each workout's free-text description into structured movements.
3. Resolve each movement to a Hevy exercise template, using an alias table for the messy
   real-world names coaches actually write (`"BB DL"` → `Barbell Deadlift`, etc.).
4. Fill in target loads from your personal maxes (derived from real Hevy lift history).
5. Create or update the corresponding Hevy routine (idempotent — a stored routine-id mapping
   means re-runs update in place rather than duplicating).
6. Optionally post a summary (or failure) to a Discord webhook — off by default.

Two supporting jobs keep the data fresh:

- **`refresh-catalog`** — weekly, rebuilds the local mirror of Hevy's exercise-template catalog.
- **`refresh-maxes`** — every 2 days, recomputes personal maxes from Hevy workout history.

## Architecture

```
EventBridge Scheduler ──► sync-wod (Lambda, Rust) ──► SugarWOD API
   3:30 AM daily              │                          │
                              │                          ▼
                              ├──► DynamoDB: ExerciseAlias, HevyExerciseCatalog,
                              │              PersonalMax, RoutineMap
                              │
                              ├──► Hevy API (create / update routine)
                              └──► Discord webhook (summary / failure, opt-in)

EventBridge Scheduler ──► refresh-catalog (weekly)   ──► Hevy API ──► HevyExerciseCatalog
EventBridge Scheduler ──► refresh-maxes   (2-daily)  ──► Hevy API ──► PersonalMax
```

Secrets are **never** in code or CloudFormation:

- **Hevy API key** — AWS Secrets Manager, shape `{"api_key": "..."}`. Secret id is configurable
  (`hevySecretId`), defaulting to `sugarwod-to-hevy/hevy-api-key`.
- **Discord webhook URL** — only when notifications are enabled. SSM Parameter Store
  SecureString at `/sugarwod-to-hevy/<stage>/discord-webhook-url`, written once by hand; not a
  CDK resource, since CloudFormation cannot manage SecureString *values* without putting the
  plaintext in the template.

### Stages

- **`prod`** Runs on an an automatic schedule, with a monitoring
  dashboard. DynamoDB tables use a `RETAIN` removal policy.
- **`gamma`** is set up on the same-account, `-Gamma`-suffixed twin for integration testing.
  No schedule (invoked manually), `DESTROY` removal policy, and its own Discord webhook if
  notifications are enabled at all. It shares the one Hevy account, distinguished by
  test-prefixed routine titles rather than separate credentials.

## Prerequisites

- Rust (stable) with the [`cargo-lambda`](https://www.cargo-lambda.info/) toolchain, or Docker
  bundling for `aws-lambda-rust`.
- Node.js 18+ and the AWS CDK v2 CLI (`npm i -g aws-cdk`).
- An AWS account with credentials configured, and CDK bootstrapped (`cdk bootstrap`).
- A [Hevy Pro](https://www.hevyapp.com/) account (the API is Pro-only) and its API key.
- A gym that posts its WOD to SugarWOD, and its **affiliate slug** — the path segment in your
  gym's SugarWOD URL (`https://app.sugarwod.com/<slug>`).
- A Discord webhook URL (optional — used only for notifications).

## Configuration

All deployment settings come from CDK context. Set them with `-c key=value`, or persist them in
`cdk.context.json` (gitignored) so you do not retype them:

```json
{
  "affiliateId": "your-gym-slug",
  "timezone": "America/Los_Angeles"
}
```

| Key | Required | Default | What it does |
| --- | --- | --- | --- |
| `affiliateId` | **yes** | — | Your gym's SugarWOD affiliate slug. No default by design: one here would sync a stranger's workouts into your Hevy account. |
| `resourcePrefix` | no | `SugarwodToHevy` | Prefixes every physical resource name. Change it to run two independent deployments in one AWS account. |
| `timezone` | no | `America/Los_Angeles` | IANA zone. Drives both the schedule and the Lambda's notion of "today". |
| `hevySecretId` | no | `<slug>/hevy-api-key` | Secrets Manager id of the Hevy API key. |
| `discordEnabled` | no | `false` | Post a run summary to Discord. Off by default; see below. |
| `syncWodCron` | no | `cron(30 3 * * ? *)` | When the daily sync fires, in `timezone`. |
| `refreshCatalogCron` | no | `cron(0 3 ? * SUN *)` | Catalog refresh schedule. |
| `refreshMaxesRate` | no | `rate(2 days)` | Personal-max refresh interval. |

`cdk synth` fails with an explicit error if `affiliateId` is unset, so a misconfigured deploy
never reaches AWS.

One-time secret/param setup (adjust names if you changed `resourcePrefix` / `hevySecretId`):

```bash
# Hevy API key — always required
aws secretsmanager create-secret \
  --name sugarwod-to-hevy/hevy-api-key \
  --secret-string '{"api_key":"<your Hevy Pro API key>"}'
```

### Discord notifications (optional, off by default)

The sync writes to Hevy either way; notifications just tell you how it went, including which
movements couldn't be mapped. They are disabled unless you ask for them, so a default deployment
needs no Discord setup and never warns about a webhook you didn't configure.

When off, `sync-wod` is not given the parameter name and its execution role is not granted
`ssm:GetParameter` at all — the capability is absent, not merely unused.

To turn them on, create the parameter and deploy with the flag:

```bash
aws ssm put-parameter \
  --name /sugarwod-to-hevy/prod/discord-webhook-url \
  --type SecureString \
  --value "https://discord.com/api/webhooks/..."

npx cdk deploy --all -c affiliateId=your-gym-slug -c discordEnabled=true
```

Get the URL from Discord: channel → Settings → Integrations → Webhooks → New Webhook → Copy URL.
Anyone holding it can post to that channel, which is why it lives in a SecureString.

## Deploy

```bash
# 1. Build the Rust Lambdas
cd rust
cargo lambda build --release --arm64   # or your target of choice

# 2. Deploy the infrastructure
cd ../cdk
npm install
npx cdk deploy --all -c affiliateId=your-gym-slug

# 3. Seed the exercise-alias table (one time, after refresh-catalog has run once).
#    Both table names come from the data stack's outputs.
CATALOG_TABLE_NAME=SugarwodToHevy-HevyExerciseCatalog-Prod \
ALIAS_TABLE_NAME=SugarwodToHevy-ExerciseAlias-Prod \
  npm run seed-aliases
```

The shipped `cdk/scripts/exercise-translation.json` dictionary reflects one gym's programming
vocabulary. Treat it as a starting point: expect to add entries for your own coaches' shorthand as
the unmapped-movement reports come in.

## Testing

```bash
# Unit tests — no AWS, no network
cd rust && cargo test --workspace

# Lint exactly as CI does
cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings
```

Integration tests hit **real deployed gamma resources** and live SugarWOD/Hevy GET endpoints. They
are `#[ignore]`d by default and need environment variables sourced from the gamma stack's outputs,
so run them through the script rather than `cargo test -- --ignored` directly:

```bash
cd cdk
SUGARWOD_AFFILIATE_ID=your-gym-slug \
AWS_PROFILE_FOR_TESTS=your-iam-profile \
  npm run integration-test
```

Add `DISCORD_ENABLED=true` only if you deployed with `-c discordEnabled=true`; the webhook test
skips itself otherwise, since a default deployment has no parameter to read.

The script assumes a least-privilege `IntegrationTestRole` scoped to exactly the union of the
three Lambda roles' grants — deliberately not your admin credentials, since the whole point is to
catch "works with my credentials but the deployed role can't do X" bugs. Note that the AWS account
**root user cannot assume roles at all** (a platform rule, not a policy setting), so
`AWS_PROFILE_FOR_TESTS` must name a non-root IAM identity.

## Known issues

- **`refresh-maxes` must run before loads appear.** Until `PersonalMax` is populated, movements
  resolve correctly but are written without target weights, reported as "loaded without a known
  max" in the run summary. This is expected on a fresh deployment, not a bug.
- **Movement resolution depends on your alias data.** A movement whose name the parser reads
  cleanly but that has no alias and no catalog title match is reported as unmapped and omitted
  from the routine. Adding the alias is the fix; see `cdk/scripts/exercise-translation.json`.
- **Hevy's published OpenAPI spec does not match its live behavior** in at least two places. This
  code trusts the live responses. See
  [docs/bug-reports/hevy-openapi-equipment-field-mismatch.md](docs/bug-reports/hevy-openapi-equipment-field-mismatch.md).

## Documentation

- [docs/design-plan.md](docs/design-plan.md) — how the system is built and, more usefully, *why*:
  every significant decision, the tradeoffs, and the worked examples that drove them.
- [CONTRIBUTING.md](CONTRIBUTING.md) — build/test/lint workflow and the conventions worth keeping.
- [SECURITY.md](SECURITY.md) — reporting a vulnerability.

## Disclaimer

This project is **not affiliated with, endorsed by, or supported by** SugarWOD or Hevy. "SugarWOD"
and "Hevy" are trademarks of their respective owners, used here only to describe what this
software talks to.

It consumes SugarWOD's public API and Hevy's documented API on your behalf, using your own
credentials. You are responsible for complying with those services' terms of service, and for any
AWS charges your deployment incurs. It writes to your Hevy account: it creates and updates
routines. Point it at a test account first if that matters to you.

## License

[GNU Affero General Public License v3.0 or later](./LICENSE).

AGPL rather than GPL deliberately. This software is deployed as a service — nobody ever receives a
binary — so GPL's copyleft would never trigger for someone running a modified fork. AGPL section 13
is the clause that matches how it is actually used: if you run a modified version and other people
interact with it over a network, they are entitled to your source.
