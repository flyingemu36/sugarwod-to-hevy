// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Eamun Rahimi

import * as cdk from 'aws-cdk-lib';
import * as cloudwatch from 'aws-cdk-lib/aws-cloudwatch';
import * as dynamodb from 'aws-cdk-lib/aws-dynamodb';
import { Operation } from 'aws-cdk-lib/aws-dynamodb';
import * as lambda from 'aws-cdk-lib/aws-lambda';
import { Construct } from 'constructs';
import { AppConfig, dashboardName, Stage, toSlug } from './config';
import {
  refreshCatalogScheduleGroup,
  refreshMaxesScheduleGroup,
  syncWodScheduleGroup,
} from './schedule-stack';

export interface MonitoringStackProps extends cdk.StackProps {
  stage: Stage;
  config: AppConfig;
  syncWodFunction: lambda.IFunction;
  refreshCatalogFunction: lambda.IFunction;
  refreshMaxesFunction: lambda.IFunction;
  hevyExerciseCatalogTable: dynamodb.Table;
  exerciseAliasTable: dynamodb.Table;
  routineMapTable: dynamodb.Table;
  personalMaxTable: dynamodb.Table;
}

const SCHEDULER_NAMESPACE = 'AWS/Scheduler';

/** One EventBridge Scheduler metric for a given schedule group — see the comment on the group
 * name constants in schedule-stack.ts for why this is grouped by ScheduleGroup, not schedule name. */
function schedulerMetric(metricName: string, scheduleGroup: string, label: string): cloudwatch.Metric {
  return new cloudwatch.Metric({
    namespace: SCHEDULER_NAMESPACE,
    metricName,
    dimensionsMap: { ScheduleGroup: scheduleGroup },
    statistic: 'Sum',
    period: cdk.Duration.hours(1),
    label,
  });
}

interface TableSpec {
  table: dynamodb.Table;
  label: string;
  /** The exact operations this table's Lambda code actually calls (verified against
   * `swh-core::store::*`, the same lists used for this table's IAM grants in ComputeStack). */
  operations: Operation[];
}

function tableSpecs(props: MonitoringStackProps): TableSpec[] {
  return [
    {
      table: props.hevyExerciseCatalogTable,
      label: 'HevyExerciseCatalog',
      operations: [Operation.GET_ITEM, Operation.QUERY, Operation.BATCH_WRITE_ITEM],
    },
    {
      table: props.exerciseAliasTable,
      label: 'ExerciseAlias',
      operations: [Operation.GET_ITEM, Operation.PUT_ITEM, Operation.SCAN],
    },
    {
      table: props.routineMapTable,
      label: 'RoutineMap',
      operations: [Operation.GET_ITEM, Operation.PUT_ITEM],
    },
    {
      table: props.personalMaxTable,
      label: 'PersonalMax',
      operations: [Operation.GET_ITEM, Operation.PUT_ITEM],
    },
  ];
}

/**
 * One narrow widget per table (not 4 lines on one shared graph): `...ForOperations()` metrics
 * are CloudWatch math expressions, and combining more than one of them in the same widget hits
 * `CannotShareSameIdForDifferentMetrics` because their auto-generated per-operation sub-metric
 * ids (e.g. "getitem") collide across tables. Four small tiles side by side is also just a
 * standard, readable CloudWatch dashboard pattern.
 */
function dynamoOpsRow(
  title: string,
  specs: TableSpec[],
  metricFn: (spec: TableSpec) => cloudwatch.IMetric,
): cloudwatch.IWidget {
  return new cloudwatch.Row(
    ...specs.map(
      (spec) =>
        new cloudwatch.GraphWidget({
          title: `${title} — ${spec.label}`,
          left: [metricFn(spec)],
          width: 6,
        }),
    ),
  );
}

/** Lambda invocations/errors/duration/throttles for one function, as a 3-widget row. */
function lambdaRow(title: string, fn: lambda.IFunction): cloudwatch.IWidget {
  return new cloudwatch.Row(
    new cloudwatch.GraphWidget({
      title: `${title} — Invocations & Errors`,
      left: [fn.metricInvocations({ label: 'Invocations', period: cdk.Duration.hours(1) })],
      right: [fn.metricErrors({ label: 'Errors', period: cdk.Duration.hours(1) })],
      width: 8,
    }),
    new cloudwatch.GraphWidget({
      title: `${title} — Duration`,
      left: [
        fn.metricDuration({ label: 'Avg', statistic: 'Average', period: cdk.Duration.hours(1) }),
        fn.metricDuration({ label: 'p99', statistic: 'p99', period: cdk.Duration.hours(1) }),
      ],
      width: 8,
    }),
    new cloudwatch.GraphWidget({
      title: `${title} — Throttles`,
      left: [fn.metricThrottles({ label: 'Throttles', period: cdk.Duration.hours(1) })],
      width: 8,
    }),
  );
}

export class MonitoringStack extends cdk.Stack {
  constructor(scope: Construct, id: string, props: MonitoringStackProps) {
    super(scope, id, props);
    const { config, stage } = props;
    const syncWodGroup = syncWodScheduleGroup(config, stage);
    const refreshCatalogGroup = refreshCatalogScheduleGroup(config, stage);
    const refreshMaxesGroup = refreshMaxesScheduleGroup(config, stage);

    // Defense in depth: if sync-wod's own Discord failure notification never makes
    // it out (e.g. the process crashed before reaching that code path), this is the fallback
    // signal. No SNS action wired up yet — visible on the dashboard and in the CloudWatch Alarms
    // console for now; add an SNS topic + subscription here later if push notification on top of
    // the dashboard becomes worth the extra resource.
    const syncWodErrorsAlarm = new cloudwatch.Alarm(this, 'SyncWodErrorsAlarm', {
      alarmName: `${toSlug(config.resourcePrefix)}-${stage}-sync-wod-errors`,
      alarmDescription: 'sync-wod (the daily WOD sync) errored at least once — check CloudWatch Logs and Discord.',
      metric: props.syncWodFunction.metricErrors({ period: cdk.Duration.hours(1) }),
      threshold: 1,
      evaluationPeriods: 1,
      comparisonOperator: cloudwatch.ComparisonOperator.GREATER_THAN_OR_EQUAL_TO_THRESHOLD,
      treatMissingData: cloudwatch.TreatMissingData.NOT_BREACHING,
    });

    const dashboard = new cloudwatch.Dashboard(this, 'Dashboard', {
      dashboardName: dashboardName(config.resourcePrefix, stage),
      defaultInterval: cdk.Duration.days(7),
    });

    dashboard.addWidgets(
      new cloudwatch.Row(
        new cloudwatch.AlarmWidget({
          title: 'sync-wod Errors Alarm',
          alarm: syncWodErrorsAlarm,
          width: 24,
          height: 4,
        }),
      ),
      lambdaRow('sync-wod (daily)', props.syncWodFunction),
      lambdaRow('refresh-catalog (weekly)', props.refreshCatalogFunction),
      lambdaRow('refresh-maxes (every 2 days)', props.refreshMaxesFunction),
      new cloudwatch.Row(
        new cloudwatch.GraphWidget({
          title: 'EventBridge Scheduler — Invocation Attempts',
          left: [
            schedulerMetric('InvocationAttemptCount', syncWodGroup, 'sync-wod'),
            schedulerMetric('InvocationAttemptCount', refreshCatalogGroup, 'refresh-catalog'),
            schedulerMetric('InvocationAttemptCount', refreshMaxesGroup, 'refresh-maxes'),
          ],
          width: 12,
        }),
        new cloudwatch.GraphWidget({
          title: 'EventBridge Scheduler — Target Errors & Dropped Invocations',
          left: [
            schedulerMetric('TargetErrorCount', syncWodGroup, 'sync-wod target errors'),
            schedulerMetric('TargetErrorCount', refreshCatalogGroup, 'refresh-catalog target errors'),
            schedulerMetric('TargetErrorCount', refreshMaxesGroup, 'refresh-maxes target errors'),
            schedulerMetric('InvocationDroppedCount', syncWodGroup, 'sync-wod dropped'),
            schedulerMetric('InvocationDroppedCount', refreshCatalogGroup, 'refresh-catalog dropped'),
            schedulerMetric('InvocationDroppedCount', refreshMaxesGroup, 'refresh-maxes dropped'),
          ],
          width: 12,
        }),
      ),
      new cloudwatch.Row(
        new cloudwatch.GraphWidget({
          title: 'DynamoDB — Consumed Read Capacity',
          left: [
            props.hevyExerciseCatalogTable.metricConsumedReadCapacityUnits({ label: 'HevyExerciseCatalog' }),
            props.exerciseAliasTable.metricConsumedReadCapacityUnits({ label: 'ExerciseAlias' }),
            props.routineMapTable.metricConsumedReadCapacityUnits({ label: 'RoutineMap' }),
            props.personalMaxTable.metricConsumedReadCapacityUnits({ label: 'PersonalMax' }),
          ],
          width: 12,
        }),
        new cloudwatch.GraphWidget({
          title: 'DynamoDB — Consumed Write Capacity',
          left: [
            props.hevyExerciseCatalogTable.metricConsumedWriteCapacityUnits({ label: 'HevyExerciseCatalog' }),
            props.exerciseAliasTable.metricConsumedWriteCapacityUnits({ label: 'ExerciseAlias' }),
            props.routineMapTable.metricConsumedWriteCapacityUnits({ label: 'RoutineMap' }),
            props.personalMaxTable.metricConsumedWriteCapacityUnits({ label: 'PersonalMax' }),
          ],
          width: 12,
        }),
      ),
      // `metricThrottledRequests()`/`metricSystemErrors()` are deprecated (the latter actually
      // throws at synth time without an explicit `Operation` dimension) — using the
      // ...ForOperations() replacements instead, one narrow widget per table (see
      // `dynamoOpsRow`'s doc comment for why they can't share a widget).
      dynamoOpsRow('DynamoDB Throttled Requests', tableSpecs(props), (spec) =>
        spec.table.metricThrottledRequestsForOperations({ operations: spec.operations, label: spec.label }),
      ),
      dynamoOpsRow('DynamoDB System Errors', tableSpecs(props), (spec) =>
        spec.table.metricSystemErrorsForOperations({ operations: spec.operations, label: spec.label }),
      ),
    );

    new cdk.CfnOutput(this, 'DashboardUrl', {
      value: `https://${this.region}.console.aws.amazon.com/cloudwatch/home?region=${this.region}#dashboards:name=${dashboard.dashboardName}`,
    });
  }
}
