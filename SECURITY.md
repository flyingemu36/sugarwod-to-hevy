# Security Policy

## Reporting a vulnerability

Email **eamun@eamunrahimi.com** with the details. Please do not open a public issue for a
vulnerability report.

Include what you can: what the issue is, how to reproduce it, and what an attacker could do with
it. You should get a reply within a week. This is a personal project maintained by one person, so
there is no formal SLA beyond that — but security reports go to the front of the queue.

## Supported versions

Only the `main` branch is supported. There are no maintained release branches.

## What this software has access to

Worth understanding before you deploy it, and useful context for a report:

- **Your Hevy API key**, stored in AWS Secrets Manager and read at runtime by all three Lambdas.
  It grants full API access to your Hevy account.
- **Write access to your Hevy routines.** `sync-wod` creates and updates routines. It never
  deletes them, and never touches logged workouts.
- **Four DynamoDB tables** in your AWS account, with IAM grants scoped to the exact actions each
  function calls (see `grantExact()` in `cdk/lib/compute-stack.ts`) rather than CDK's broader
  convenience grants.
- **A Discord webhook URL**, stored as an SSM SecureString. Anyone holding it can post to that
  channel.
- **Outbound network access** to `app.sugarwod.com`, `api.hevyapp.com`, and `discord.com`.

## If you fork and deploy this

The secrets are yours, not the project's. A few things that are easy to get wrong:

- Never put the Hevy API key or the Discord webhook URL in `cdk.json` — it is committed. Use
  Secrets Manager and SSM SecureString, as the README describes.
- `cdk.context.json` is gitignored because it is the natural place to put your affiliate slug and
  can also cache AWS account ids. Keep it that way.
- Deploy with a dedicated IAM identity rather than the account root user. Beyond being good
  practice, root cannot assume roles at all, which the integration test flow depends on.
- The `gamma` stage shares the same Hevy account as `prod` and writes real routines to it (with
  test-prefixed titles). If that is not acceptable for you, point it at a separate Hevy account.
