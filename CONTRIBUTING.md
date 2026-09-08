# Contributing

Thanks for taking a look. This is a small, single-purpose project — a personal utility that
happens to be useful enough to share — so the bar is "does it work and is it clear", not
ceremony.

## Getting set up

You do not need an AWS account to work on the parsing, matching, or load-calculation logic. That
is where most of the interesting bugs live, and it is all unit-testable offline.

```bash
git clone https://github.com/flyingemu36/sugarwod-to-hevy.git
cd sugarwod-to-hevy/rust
cargo test --workspace          # 86 tests, no network, no credentials
```

For the CDK side:

```bash
cd cdk
npm ci
npx tsc --noEmit
npx cdk synth --all -c affiliateId=example    # no AWS credentials required
```

## Before you open a PR

CI runs exactly these, and `-D warnings` means clippy lints fail the build:

```bash
cd rust
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

cd ../cdk
npx tsc --noEmit
npx cdk synth --all -c affiliateId=example
```

The `--ignored` integration tests are **not** run in CI — they need a deployed gamma stack and
real credentials. Run them locally if your change touches AWS interaction; see the README.

## Conventions worth keeping

These are load-bearing, not style preferences. Each one exists because the alternative caused a
real problem:

- **Parser fixes are test-first, with the real WOD text as the fixture.** Add a failing test
  containing the exact description that broke, then fix it. Paraphrased or simplified inputs miss
  the thing that actually went wrong — see the `incident_wod_*` tests in `description_parser.rs`
  for the shape.
- **Prefer static fixtures over live API calls in tests.** A test that fetches a specific gym's
  workout for a specific past date cannot pass for anyone else and breaks when the upstream data
  moves. Capture the bytes.
- **IAM grants use `grantExact()`, not `grantReadData()`/`grantWriteData()`.** The CDK convenience
  methods grant a much broader action set (Scan, DeleteItem, UpdateItem…) than this code calls.
  New grants should list the exact actions, verified against `swh-core::store::*`.
- **No new hardcoded deployment values.** Anything specific to one gym, timezone, account, or
  resource name belongs in `cdk/lib/config.ts` and reaches Rust through an environment variable.
  Required config has no fallback default — failing loudly beats syncing the wrong gym silently.
- **Trust the live API over the published spec.** Hevy's OpenAPI document has drifted from real
  wire behavior in several places. If they disagree, verify against the live service and code to
  what it actually returns, with a comment saying so.
- **Every source file carries an SPDX header.** New `.rs`/`.ts` files need:
  ```
  // SPDX-License-Identifier: AGPL-3.0-or-later
  // Copyright (C) <year> <your name>
  ```

## Adding exercise aliases

Most "the sync missed a movement" reports are a missing alias rather than a code bug. Add the
mapping to `cdk/scripts/exercise-translation.json` and re-run the seed script.

Map to the **most specific correct target**. A position qualifier usually carries equipment
information: `Back Rack Bulgarian Split Squat` means the barbell variant, so it should not be
aliased to a generic entry that points at the dumbbell one.

## Reporting bugs

The single most useful thing you can include is the **raw WOD description text** that produced the
wrong result — copy it verbatim, newlines and typos intact. That text is the input to every stage
of the pipeline, and with it a fix usually takes minutes instead of guesswork. The issue template
asks for it.

## License

By contributing, you agree that your contributions are licensed under the AGPL-3.0-or-later, the
same terms that cover the project.
