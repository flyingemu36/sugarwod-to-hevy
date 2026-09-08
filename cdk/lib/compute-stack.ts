// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Eamun Rahimi

import * as cdk from 'aws-cdk-lib';
import * as dynamodb from 'aws-cdk-lib/aws-dynamodb';
import * as iam from 'aws-cdk-lib/aws-iam';
import * as lambda from 'aws-cdk-lib/aws-lambda';
import { RustFunction } from 'cargo-lambda-cdk';
import { Construct } from 'constructs';
import { AppConfig, discordWebhookParamName, Stage } from './config';

export interface ComputeStackProps extends cdk.StackProps {
  stage: Stage;
  config: AppConfig;
  hevyExerciseCatalogTable: dynamodb.Table;
  exerciseAliasTable: dynamodb.Table;
  routineMapTable: dynamodb.Table;
  personalMaxTable: dynamodb.Table;
}

// The secret holds `{"api_key": "..."}`. Both stages read the SAME one: there is only one Hevy
// account behind a deployment, and gamma is distinguished by test-prefixed routine titles rather
// than by separate credentials. The trailing `*` matches the 6-character suffix Secrets Manager
// appends to every secret's ARN.
const hevySecretArnPattern = (stack: cdk.Stack, secretId: string) =>
  `arn:${stack.partition}:secretsmanager:${stack.region}:${stack.account}:secret:${secretId}*`;

/**
 * Grants exactly the given DynamoDB actions on `table` (and, if provided, one of its GSIs) —
 * used instead of CDK's `grantReadData`/`grantWriteData`/`grantReadWriteData` convenience
 * methods, which each grant a much broader action set (Scan, DeleteItem, UpdateItem, etc.)
 * than any of these functions' code actually calls. Verified against the exact DynamoDB SDK
 * calls in `swh-core::store::*` — see the plan's "IAM (least privilege)" section.
 *
 * Takes `iam.IGrantable` (not `lambda.IFunction`) so the same precision applies whether the
 * grantee is a deployed Lambda's execution role or the standalone `IntegrationTestRole` below.
 */
function grantExact(
  grantee: iam.IGrantable,
  table: dynamodb.Table,
  actions: string[],
  indexName?: string,
) {
  const resources = [table.tableArn];
  if (indexName) {
    resources.push(`${table.tableArn}/index/${indexName}`);
  }
  iam.Grant.addToPrincipal({ grantee, actions, resourceArns: resources });
}

export class ComputeStack extends cdk.Stack {
  public readonly syncWodFunction: lambda.IFunction;
  public readonly refreshCatalogFunction: lambda.IFunction;
  public readonly refreshMaxesFunction: lambda.IFunction;

  constructor(scope: Construct, id: string, props: ComputeStackProps) {
    super(scope, id, props);
    const { config } = props;
    // Only meaningful when notifications are on; see `discordEnabled` in config.ts.
    const webhookParamName = discordWebhookParamName(config.resourcePrefix, props.stage);
    const webhookReadPolicy = new iam.PolicyStatement({
      actions: ['ssm:GetParameter'],
      resources: [
        `arn:${this.partition}:ssm:${this.region}:${this.account}:parameter${webhookParamName}`,
      ],
    });

    const secretsPolicy = new iam.PolicyStatement({
      actions: ['secretsmanager:GetSecretValue'],
      resources: [hevySecretArnPattern(this, config.hevySecretId)],
    });

    const commonBundling = {
      architecture: lambda.Architecture.ARM_64,
    };

    // Injected into all three functions. Nothing in the Rust code has a fallback for these —
    // a missing one is a startup error rather than a silent default, so a misconfigured deploy
    // fails loudly instead of quietly syncing the wrong gym's workouts.
    const commonEnv = {
      HEVY_SECRET_ID: config.hevySecretId,
    };

    // --- sync-wod: the daily job ---
    const syncWod = new RustFunction(this, 'SyncWodFunction', {
      manifestPath: '../rust/crates/sync-wod/Cargo.toml',
      bundling: commonBundling,
      memorySize: 256,
      timeout: cdk.Duration.minutes(2),
      environment: {
        ...commonEnv,
        ALIAS_TABLE_NAME: props.exerciseAliasTable.tableName,
        CATALOG_TABLE_NAME: props.hevyExerciseCatalogTable.tableName,
        ROUTINE_MAP_TABLE_NAME: props.routineMapTable.tableName,
        PERSONAL_MAX_TABLE_NAME: props.personalMaxTable.tableName,
        SUGARWOD_AFFILIATE_ID: config.affiliateId,
        // Must match the schedule's timezone: the handler always syncs "today", and "today" has
        // to mean the same day the 3:30 AM local trigger intended.
        LOCAL_TIMEZONE: config.timezone,
        ...(config.discordEnabled ? { DISCORD_WEBHOOK_PARAM_NAME: webhookParamName } : {}),
      },
    });
    syncWod.addToRolePolicy(secretsPolicy);
    // ExerciseAlias: GetItem (lookup) + PutItem (auto-resolved write-back on a catalog hit).
    grantExact(syncWod, props.exerciseAliasTable, ['dynamodb:GetItem', 'dynamodb:PutItem']);
    // HevyExerciseCatalog: GetItem (by id, for is_loaded_type recovery) + Query on the
    // TitleNormalizedIndex GSI. No Scan and no write access on this table at all.
    grantExact(
      syncWod,
      props.hevyExerciseCatalogTable,
      ['dynamodb:GetItem', 'dynamodb:Query'],
      'TitleNormalizedIndex',
    );
    // PersonalMax: GetItem only — sync-wod never writes maxes, only refresh-maxes does.
    grantExact(syncWod, props.personalMaxTable, ['dynamodb:GetItem']);
    // RoutineMap: GetItem (existing routine id) + PutItem (upsert after create/update).
    grantExact(syncWod, props.routineMapTable, ['dynamodb:GetItem', 'dynamodb:PutItem']);
    if (config.discordEnabled) {
      syncWod.addToRolePolicy(webhookReadPolicy);
    }

    // --- refresh-catalog: weekly Hevy exercise-template catalog sync ---
    const refreshCatalog = new RustFunction(this, 'RefreshCatalogFunction', {
      manifestPath: '../rust/crates/refresh-catalog/Cargo.toml',
      bundling: commonBundling,
      memorySize: 256,
      timeout: cdk.Duration.minutes(5),
      environment: {
        ...commonEnv,
        CATALOG_TABLE_NAME: props.hevyExerciseCatalogTable.tableName,
      },
    });
    refreshCatalog.addToRolePolicy(secretsPolicy);
    // BatchWriteItem only — bulk-upserts the catalog. No access to any other table.
    grantExact(refreshCatalog, props.hevyExerciseCatalogTable, ['dynamodb:BatchWriteItem']);

    // --- refresh-maxes: every-2-days personal max derivation from Hevy history ---
    const refreshMaxes = new RustFunction(this, 'RefreshMaxesFunction', {
      manifestPath: '../rust/crates/refresh-maxes/Cargo.toml',
      bundling: commonBundling,
      memorySize: 256,
      timeout: cdk.Duration.minutes(5),
      environment: {
        ...commonEnv,
        ALIAS_TABLE_NAME: props.exerciseAliasTable.tableName,
        CATALOG_TABLE_NAME: props.hevyExerciseCatalogTable.tableName,
        PERSONAL_MAX_TABLE_NAME: props.personalMaxTable.tableName,
      },
    });
    refreshMaxes.addToRolePolicy(secretsPolicy);
    // Scan only — enumerates distinct hevy_template_ids in use, bounding exercise_history calls
    // to what's relevant instead of the whole ~1090-exercise catalog.
    grantExact(refreshMaxes, props.exerciseAliasTable, ['dynamodb:Scan']);
    // GetItem only — recovers a template's title for the denormalized PersonalMax row.
    grantExact(refreshMaxes, props.hevyExerciseCatalogTable, ['dynamodb:GetItem']);
    // PutItem only — the Rust code upserts one item at a time (per-exercise), not batched.
    grantExact(refreshMaxes, props.personalMaxTable, ['dynamodb:PutItem']);

    this.syncWodFunction = syncWod;
    this.refreshCatalogFunction = refreshCatalog;
    this.refreshMaxesFunction = refreshMaxes;

    new cdk.CfnOutput(this, 'SyncWodFunctionName', { value: syncWod.functionName });
    new cdk.CfnOutput(this, 'RefreshCatalogFunctionName', { value: refreshCatalog.functionName });
    new cdk.CfnOutput(this, 'RefreshMaxesFunctionName', { value: refreshMaxes.functionName });

    // --- gamma only: a role for running the integration test suite ---
    //
    // Deliberately NOT the developer's personal/SSO/admin credentials: the whole point of
    // testing against a deployed stack is to catch "works with my local credentials but the
    // deployed Lambda's IAM role is missing a permission" bugs, which an admin identity can
    // never surface (it can do everything regardless of what the real roles grant). This role
    // is scoped to exactly the UNION of what the three Lambda roles above are granted — no
    // more — using the same `grantExact` precision, so the integration tests run under a real
    // least-privilege boundary instead of an all-powerful one.
    //
    // It's a union rather than three separate per-function roles (which would be even more
    // precise, catching a permission gap specific to e.g. only refresh-catalog) because the
    // test suite currently exercises all three functions' responsibilities in one serial run;
    // splitting into per-role test passes is a reasonable future tightening if it's ever worth
    // the added complexity of three separate assume-role + test-run cycles.
    if (props.stage === 'gamma') {
      const integrationTestRole = new iam.Role(this, 'IntegrationTestRole', {
        assumedBy: new iam.AccountPrincipal(this.account),
        description: 'Least-privilege role for running the gamma integration test suite: union of the three Lambda roles grants, not admin/personal credentials.',
      });

      integrationTestRole.addToPolicy(secretsPolicy);
      if (config.discordEnabled) {
        integrationTestRole.addToPolicy(webhookReadPolicy);
      }
      // ExerciseAlias: union of sync-wod's GetItem+PutItem and refresh-maxes's Scan.
      grantExact(integrationTestRole, props.exerciseAliasTable, [
        'dynamodb:GetItem',
        'dynamodb:PutItem',
        'dynamodb:Scan',
      ]);
      // HevyExerciseCatalog: union of sync-wod's GetItem+Query and refresh-catalog's
      // BatchWriteItem (refresh-maxes's GetItem is already covered by sync-wod's grant).
      grantExact(
        integrationTestRole,
        props.hevyExerciseCatalogTable,
        ['dynamodb:GetItem', 'dynamodb:Query', 'dynamodb:BatchWriteItem'],
        'TitleNormalizedIndex',
      );
      // PersonalMax: union of sync-wod's GetItem and refresh-maxes's PutItem.
      grantExact(integrationTestRole, props.personalMaxTable, ['dynamodb:GetItem', 'dynamodb:PutItem']);
      // RoutineMap: same as sync-wod's grant (no other function touches this table).
      grantExact(integrationTestRole, props.routineMapTable, ['dynamodb:GetItem', 'dynamodb:PutItem']);

      new cdk.CfnOutput(this, 'IntegrationTestRoleArn', { value: integrationTestRole.roleArn });
    }
  }
}
