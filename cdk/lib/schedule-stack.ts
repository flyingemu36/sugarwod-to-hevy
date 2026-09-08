// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Eamun Rahimi

import * as cdk from 'aws-cdk-lib';
import * as iam from 'aws-cdk-lib/aws-iam';
import * as lambda from 'aws-cdk-lib/aws-lambda';
import * as scheduler from 'aws-cdk-lib/aws-scheduler';
import { Construct } from 'constructs';
import { AppConfig, scheduleGroupName, Stage } from './config';

export interface ScheduleStackProps extends cdk.StackProps {
  stage: Stage;
  config: AppConfig;
  syncWodFunction: lambda.IFunction;
  refreshCatalogFunction: lambda.IFunction;
  refreshMaxesFunction: lambda.IFunction;
}

// EventBridge Scheduler's CloudWatch metrics (InvocationAttemptCount, TargetErrorCount, etc.)
// are dimensioned by ScheduleGroup, NOT by individual schedule name — verified against AWS's
// docs (there is no per-schedule-name dimension at all). Left on the default group, all three
// schedules would share one indistinguishable metric stream. One group per schedule is the only
// way to get an accurate "did DailyWodSync actually fire today" signal on the dashboard, as
// opposed to "did *something* fire" — MonitoringStack keys its EventBridge widgets off these.
//
// Group names are account-global, so they carry the resource prefix and stage like every other
// physical name here.
export const SCHEDULE_GROUP_JOBS = {
  syncWod: 'sync-wod',
  refreshCatalog: 'refresh-catalog',
  refreshMaxes: 'refresh-maxes',
} as const;

export function syncWodScheduleGroup(config: AppConfig, stage: Stage): string {
  return scheduleGroupName(config.resourcePrefix, stage, SCHEDULE_GROUP_JOBS.syncWod);
}
export function refreshCatalogScheduleGroup(config: AppConfig, stage: Stage): string {
  return scheduleGroupName(config.resourcePrefix, stage, SCHEDULE_GROUP_JOBS.refreshCatalog);
}
export function refreshMaxesScheduleGroup(config: AppConfig, stage: Stage): string {
  return scheduleGroupName(config.resourcePrefix, stage, SCHEDULE_GROUP_JOBS.refreshMaxes);
}

function scheduleRole(scope: Construct, id: string, fn: lambda.IFunction): iam.Role {
  const role = new iam.Role(scope, id, {
    assumedBy: new iam.ServicePrincipal('scheduler.amazonaws.com'),
  });
  fn.grantInvoke(role);
  return role;
}

export class ScheduleStack extends cdk.Stack {
  constructor(scope: Construct, id: string, props: ScheduleStackProps) {
    super(scope, id, props);
    const { config, stage } = props;

    const syncWodGroup = new scheduler.CfnScheduleGroup(this, 'SyncWodScheduleGroup', {
      name: syncWodScheduleGroup(config, stage),
    });
    const refreshCatalogGroup = new scheduler.CfnScheduleGroup(this, 'RefreshCatalogScheduleGroup', {
      name: refreshCatalogScheduleGroup(config, stage),
    });
    const refreshMaxesGroup = new scheduler.CfnScheduleGroup(this, 'RefreshMaxesScheduleGroup', {
      name: refreshMaxesScheduleGroup(config, stage),
    });

    // Defaults to 3:30 AM local daily — a buffer before an early training slot. Override with
    // the `syncWodCron` / `timezone` context keys rather than editing this file. EventBridge
    // Scheduler's `scheduleExpressionTimezone` handles DST automatically, so the trigger stays
    // anchored to local time rather than drifting an hour twice a year like a bare UTC cron.
    //
    // Construct ids below are suffixed `Schedule` — NOT the original bare `DailyWodSync` etc. —
    // deliberately, to force CloudFormation to CREATE a new schedule (in its ScheduleGroup) and
    // DELETE the old ungrouped one, rather than attempting an in-place update. EventBridge
    // Scheduler's `UpdateSchedule` API cannot move a schedule between groups (the group is part
    // of the schedule's identity/ARN) — CloudFormation's resource type doesn't mark `GroupName`
    // as replacement-only, so a same-logical-id update attempts and fails with a confusing
    // "resource ... was not found" error instead of a clean replace. Verified via a real failed
    // deploy attempt, not a guess — don't rename these back to the old ids.
    const dailyWodSync = new scheduler.CfnSchedule(this, 'DailyWodSyncSchedule', {
      groupName: syncWodGroup.ref,
      scheduleExpression: config.syncWodCron,
      scheduleExpressionTimezone: config.timezone,
      flexibleTimeWindow: { mode: 'OFF' },
      target: {
        arn: props.syncWodFunction.functionArn,
        roleArn: scheduleRole(this, 'DailyWodSyncRole', props.syncWodFunction).roleArn,
        retryPolicy: { maximumRetryAttempts: 0 }, // sync-wod retries transient HTTP failures internally
      },
    });
    dailyWodSync.addResourceDependency(syncWodGroup);

    // Weekly — refreshes the mirror of Hevy's exercise-template catalog that the resolver
    // matches against. Weekly is plenty: Hevy adds templates rarely, and `sync-wod` writes back
    // any alias it auto-resolves in between runs.
    const weeklyCatalogRefresh = new scheduler.CfnSchedule(this, 'WeeklyCatalogRefreshSchedule', {
      groupName: refreshCatalogGroup.ref,
      scheduleExpression: config.refreshCatalogCron,
      scheduleExpressionTimezone: config.timezone,
      flexibleTimeWindow: { mode: 'OFF' },
      target: {
        arn: props.refreshCatalogFunction.functionArn,
        roleArn: scheduleRole(this, 'WeeklyCatalogRefreshRole', props.refreshCatalogFunction).roleArn,
      },
    });
    weeklyCatalogRefresh.addResourceDependency(refreshCatalogGroup);

    // Defaults to every 2 days, starting from whenever this stack is deployed — derives personal
    // maxes from real Hevy lift history. A `rate()` expression (rather than cron) is the natural
    // fit for "every N days"; override with the `refreshMaxesRate` context key.
    //
    // No explicit `startDate`: EventBridge Scheduler defaults it to schedule creation time when
    // omitted, which is exactly "every 2 days starting now" — a hardcoded literal date was tried
    // here first and rejected by the service (`StartDate` must be within 5 minutes of the current
    // time, not an arbitrary anchor in the past).
    const biDailyMaxesRefresh = new scheduler.CfnSchedule(this, 'BiDailyMaxesRefreshSchedule', {
      groupName: refreshMaxesGroup.ref,
      scheduleExpression: config.refreshMaxesRate,
      scheduleExpressionTimezone: config.timezone,
      flexibleTimeWindow: { mode: 'OFF' },
      target: {
        arn: props.refreshMaxesFunction.functionArn,
        roleArn: scheduleRole(this, 'BiDailyMaxesRefreshRole', props.refreshMaxesFunction).roleArn,
      },
    });
    biDailyMaxesRefresh.addResourceDependency(refreshMaxesGroup);
  }
}
