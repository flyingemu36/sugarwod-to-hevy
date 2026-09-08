// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Eamun Rahimi

import * as cdk from 'aws-cdk-lib';
import * as dynamodb from 'aws-cdk-lib/aws-dynamodb';
import { Construct } from 'constructs';
import { AppConfig, discordWebhookParamName, Stage, tableName } from './config';

export interface DataStackProps extends cdk.StackProps {
  stage: Stage;
  config: AppConfig;
}

export class DataStack extends cdk.Stack {
  public readonly hevyExerciseCatalogTable: dynamodb.Table;
  public readonly exerciseAliasTable: dynamodb.Table;
  public readonly routineMapTable: dynamodb.Table;
  public readonly personalMaxTable: dynamodb.Table;

  constructor(scope: Construct, id: string, props: DataStackProps) {
    super(scope, id, props);
    const { stage, config } = props;
    const name = (base: string) => tableName(config.resourcePrefix, base, stage);

    // Prod RETAINs so a `cdk destroy` can never take the curated alias table and the routine
    // mapping with it; gamma is disposable test data and DESTROYs cleanly.
    const removalPolicy =
      stage === 'gamma' ? cdk.RemovalPolicy.DESTROY : cdk.RemovalPolicy.RETAIN;

    this.hevyExerciseCatalogTable = new dynamodb.Table(this, 'HevyExerciseCatalogTable', {
      tableName: name('HevyExerciseCatalog'),
      partitionKey: { name: 'id', type: dynamodb.AttributeType.STRING },
      billingMode: dynamodb.BillingMode.PAY_PER_REQUEST,
      removalPolicy,
    });
    // Point Query lookups by normalized title — the resolver's hot path. Without this GSI the
    // only way to find a template by title is a full-table Scan.
    this.hevyExerciseCatalogTable.addGlobalSecondaryIndex({
      indexName: 'TitleNormalizedIndex',
      partitionKey: { name: 'title_normalized', type: dynamodb.AttributeType.STRING },
      projectionType: dynamodb.ProjectionType.ALL,
    });

    this.exerciseAliasTable = new dynamodb.Table(this, 'ExerciseAliasTable', {
      tableName: name('ExerciseAlias'),
      partitionKey: { name: 'alias', type: dynamodb.AttributeType.STRING },
      billingMode: dynamodb.BillingMode.PAY_PER_REQUEST,
      removalPolicy,
    });

    this.routineMapTable = new dynamodb.Table(this, 'RoutineMapTable', {
      tableName: name('RoutineMap'),
      partitionKey: { name: 'routine_key', type: dynamodb.AttributeType.STRING },
      billingMode: dynamodb.BillingMode.PAY_PER_REQUEST,
      removalPolicy,
    });

    this.personalMaxTable = new dynamodb.Table(this, 'PersonalMaxTable', {
      tableName: name('PersonalMax'),
      partitionKey: { name: 'exercise_template_id', type: dynamodb.AttributeType.STRING },
      billingMode: dynamodb.BillingMode.PAY_PER_REQUEST,
      removalPolicy,
    });

    new cdk.CfnOutput(this, 'HevyExerciseCatalogTableName', {
      value: this.hevyExerciseCatalogTable.tableName,
    });
    new cdk.CfnOutput(this, 'ExerciseAliasTableName', { value: this.exerciseAliasTable.tableName });
    new cdk.CfnOutput(this, 'RoutineMapTableName', { value: this.routineMapTable.tableName });
    new cdk.CfnOutput(this, 'PersonalMaxTableName', { value: this.personalMaxTable.tableName });
    new cdk.CfnOutput(this, 'DiscordWebhookParamName', {
      value: discordWebhookParamName(config.resourcePrefix, stage),
    });
    new cdk.CfnOutput(this, 'HevySecretId', { value: config.hevySecretId });
  }
}
