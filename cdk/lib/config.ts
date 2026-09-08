// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Eamun Rahimi

import * as cdk from 'aws-cdk-lib';

export type Stage = 'prod' | 'gamma';

/**
 * Every value that is specific to one person's gym, timezone, or AWS account. Nothing under
 * `lib/` may hardcode any of these — the whole point is that a stranger can clone this repo and
 * deploy it against their own box without editing source.
 *
 * Values come from CDK context, so they can be set three ways, in increasing precedence:
 *   1. the `context` block in `cdk.json` (committed — generic defaults only, never real values)
 *   2. `cdk.context.json` (gitignored — the natural home for your own deployment's settings)
 *   3. `-c key=value` on the command line
 */
export interface AppConfig {
  /** SugarWOD affiliate slug — the `.../affiliates/<slug>/workouts/...` path segment. Required. */
  readonly affiliateId: string;
  /** Prefixes every physical resource name, so two deployments can share one AWS account. */
  readonly resourcePrefix: string;
  /** IANA timezone. Drives both the EventBridge cron and the Lambda's notion of "today". */
  readonly timezone: string;
  /** Secrets Manager id holding `{"api_key": "..."}` for Hevy. */
  readonly hevySecretId: string;
  /**
   * Whether to post a run summary to Discord. **Off by default**: notifications are a
   * convenience, not part of syncing, and enabling them by default would mean every deployment
   * either warns on every run or needs an SSM parameter created before it works.
   *
   * When off, the Lambda is not given the webhook parameter name and its execution role is not
   * granted `ssm:GetParameter` at all — the capability is absent rather than merely unused.
   */
  readonly discordEnabled: boolean;
  readonly syncWodCron: string;
  readonly refreshCatalogCron: string;
  readonly refreshMaxesRate: string;
}

const DEFAULTS = {
  resourcePrefix: 'SugarwodToHevy',
  timezone: 'America/Los_Angeles',
  // 3:30 AM local daily. You may want to change this based on when your WOD is updated by your 
  // gym and when you decide to work out. EventBridge Scheduler's `scheduleExpressionTimezone` 
  // handles DST, so this stays anchored to local time year-round.
  syncWodCron: 'cron(30 3 * * ? *)',
  refreshCatalogCron: 'cron(0 3 ? * SUN *)',
  refreshMaxesRate: 'rate(2 days)',
};

/** `SugarwodToHevy` -> `sugarwod-to-hevy`, for SSM paths and schedule-group names. */
export function toSlug(prefix: string): string {
  return prefix
    .replace(/([a-z0-9])([A-Z])/g, '$1-$2')
    .replace(/[^a-zA-Z0-9]+/g, '-')
    .toLowerCase()
    .replace(/^-+|-+$/g, '');
}

/**
 * Reads a boolean context value. `-c key=value` always arrives as a string, while `cdk.json` and
 * `cdk.context.json` give a real boolean, so both have to be accepted. Anything unrecognised is
 * rejected rather than treated as false: silently ignoring `-c discordEnabled=yes` would leave
 * you wondering why notifications never arrive.
 */
function booleanContext(app: cdk.App, key: string, fallback: boolean): boolean {
  const value = app.node.tryGetContext(key);
  if (value === undefined || value === null || value === '') return fallback;
  if (typeof value === 'boolean') return value;
  const normalized = String(value).trim().toLowerCase();
  if (normalized === 'true') return true;
  if (normalized === 'false') return false;
  throw new Error(
    `CDK context "${key}" must be true or false, got ${JSON.stringify(value)}.`,
  );
}

function requireContext(app: cdk.App, key: string, hint: string): string {
  const value = app.node.tryGetContext(key);
  if (typeof value !== 'string' || value.trim() === '') {
    throw new Error(
      `Missing required CDK context "${key}".\n\n${hint}\n\n` +
        `Set it with:  cdk deploy --all -c ${key}=<value>\n` +
        `or persist it in cdk.context.json (gitignored):  { "${key}": "<value>" }`,
    );
  }
  return value.trim();
}

export function loadConfig(app: cdk.App): AppConfig {
  const resourcePrefix = app.node.tryGetContext('resourcePrefix') ?? DEFAULTS.resourcePrefix;

  const affiliateId = requireContext(
    app,
    'affiliateId',
    'This is your gym\'s SugarWOD affiliate slug — there is deliberately no default, because a\n' +
      'default here would silently sync somebody else\'s workouts into your Hevy account.\n' +
      'Find it in the URL of your gym\'s SugarWOD page: https://app.sugarwod.com/<slug>',
  );

  return {
    affiliateId,
    resourcePrefix,
    timezone: app.node.tryGetContext('timezone') ?? DEFAULTS.timezone,
    hevySecretId:
      app.node.tryGetContext('hevySecretId') ?? `${toSlug(resourcePrefix)}/hevy-api-key`,
    discordEnabled: booleanContext(app, 'discordEnabled', false),
    syncWodCron: app.node.tryGetContext('syncWodCron') ?? DEFAULTS.syncWodCron,
    refreshCatalogCron:
      app.node.tryGetContext('refreshCatalogCron') ?? DEFAULTS.refreshCatalogCron,
    refreshMaxesRate: app.node.tryGetContext('refreshMaxesRate') ?? DEFAULTS.refreshMaxesRate,
  };
}

// --- Physical resource naming -------------------------------------------------------------
//
// Every name below is account-global (DynamoDB tables, EventBridge schedule groups, CloudWatch
// dashboards, and SSM paths all share one namespace per account+region). Prefixing them is what
// makes a second deployment — someone else's, or your own gamma stage — safe rather than a
// silent collision with, or overwrite of, whatever is already there.

export function tableName(prefix: string, base: string, stage: Stage): string {
  return `${prefix}-${base}-${stage === 'gamma' ? 'Gamma' : 'Prod'}`;
}

/**
 * SSM parameter holding the Discord webhook URL. Deliberately NOT a CDK/CloudFormation resource:
 * it is a SecureString, and CloudFormation cannot manage SecureString *values* without putting
 * the plaintext in the template. Written once by hand per stage; only ever read at runtime.
 */
export function discordWebhookParamName(prefix: string, stage: Stage): string {
  return `/${toSlug(prefix)}/${stage}/discord-webhook-url`;
}

export function scheduleGroupName(prefix: string, stage: Stage, job: string): string {
  return `${toSlug(prefix)}-${stage}-${job}`;
}

export function dashboardName(prefix: string, stage: Stage): string {
  return `${prefix}-${stage === 'gamma' ? 'Gamma' : 'Prod'}`;
}
