#!/usr/bin/env node
// SPDX-License-Identifier: AGPL-3.0-or-later
//
// SugarWOD to Hevy: turns a gym's daily Workout of the Day into a ready-to-run
// Hevy routine, with loads filled in from your own lift history.
// Copyright (C) 2026 Eamun Rahimi
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

import * as cdk from 'aws-cdk-lib';
import { ComputeStack } from '../lib/compute-stack';
import { loadConfig } from '../lib/config';
import { DataStack } from '../lib/data-stack';
import { MonitoringStack } from '../lib/monitoring-stack';
import { ScheduleStack } from '../lib/schedule-stack';

const app = new cdk.App();

// Deployment-specific settings (gym affiliate, timezone, resource prefix, schedule) all come
// from CDK context — see lib/config.ts. `affiliateId` has no default and throws here if unset,
// so a misconfigured deploy fails at synth rather than silently syncing someone else's gym.
const config = loadConfig(app);

// Stack ids carry an explicit stage suffix so both stages can coexist in one AWS account.
const stackId = (name: string, stage: 'Prod' | 'Gamma') =>
  `${config.resourcePrefix}${name}Stack-${stage}`;

// --- prod: the real daily-driver deployment ---
const dataStack = new DataStack(app, stackId('Data', 'Prod'), { stage: 'prod', config });

const computeStack = new ComputeStack(app, stackId('Compute', 'Prod'), {
  stage: 'prod',
  config,
  hevyExerciseCatalogTable: dataStack.hevyExerciseCatalogTable,
  exerciseAliasTable: dataStack.exerciseAliasTable,
  routineMapTable: dataStack.routineMapTable,
  personalMaxTable: dataStack.personalMaxTable,
});
computeStack.addStackDependency(dataStack);

const scheduleStack = new ScheduleStack(app, stackId('Schedule', 'Prod'), {
  stage: 'prod',
  config,
  syncWodFunction: computeStack.syncWodFunction,
  refreshCatalogFunction: computeStack.refreshCatalogFunction,
  refreshMaxesFunction: computeStack.refreshMaxesFunction,
});
scheduleStack.addStackDependency(computeStack);

// Watches the three prod Lambdas, the four prod DynamoDB tables, and (via the per-schedule
// ScheduleGroups in ScheduleStack) EventBridge Scheduler's own invocation health — prod only,
// since gamma has no schedule to watch and its DynamoDB traffic is just test noise.
const monitoringStack = new MonitoringStack(app, stackId('Monitoring', 'Prod'), {
  stage: 'prod',
  config,
  syncWodFunction: computeStack.syncWodFunction,
  refreshCatalogFunction: computeStack.refreshCatalogFunction,
  refreshMaxesFunction: computeStack.refreshMaxesFunction,
  hevyExerciseCatalogTable: dataStack.hevyExerciseCatalogTable,
  exerciseAliasTable: dataStack.exerciseAliasTable,
  routineMapTable: dataStack.routineMapTable,
  personalMaxTable: dataStack.personalMaxTable,
});
monitoringStack.addStackDependency(scheduleStack);

// --- gamma: same-account, stage-suffixed twin for integration testing ---
// Deliberately no ScheduleStack: gamma is invoked manually (`npm run integration-test`,
// `aws lambda invoke`), never on an automatic cron. It shares the real Hevy account — routines
// are test-prefixed rather than credentials being separated — but gets its own isolated DynamoDB
// tables (DESTROY removal policy, unlike prod's RETAIN) and its own Discord webhook parameter.
const gammaDataStack = new DataStack(app, stackId('Data', 'Gamma'), { stage: 'gamma', config });

const gammaComputeStack = new ComputeStack(app, stackId('Compute', 'Gamma'), {
  stage: 'gamma',
  config,
  hevyExerciseCatalogTable: gammaDataStack.hevyExerciseCatalogTable,
  exerciseAliasTable: gammaDataStack.exerciseAliasTable,
  routineMapTable: gammaDataStack.routineMapTable,
  personalMaxTable: gammaDataStack.personalMaxTable,
});
gammaComputeStack.addStackDependency(gammaDataStack);
