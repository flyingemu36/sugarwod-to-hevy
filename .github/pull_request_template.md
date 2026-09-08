## What and why

<!-- What changes, and what problem it solves. Link an issue if there is one. -->

## Checklist

- [ ] `cargo fmt --check` and `cargo clippy --workspace --all-targets -- -D warnings` pass
- [ ] `cargo test --workspace` passes
- [ ] `npx tsc --noEmit` and `npx cdk synth --all -c affiliateId=example` pass (if CDK changed)
- [ ] New source files carry the `SPDX-License-Identifier: AGPL-3.0-or-later` header
- [ ] No new hardcoded gym / timezone / account / resource names (see `cdk/lib/config.ts`)

## For parser or resolver changes

- [ ] A test containing the **real WOD text** that broke, added first and failing before the fix
- [ ] No new live-network dependency in tests — capture fixtures instead

## Notes for the reviewer

<!-- Anything surprising, or a decision you would like a second opinion on. -->
