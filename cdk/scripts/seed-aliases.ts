// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Eamun Rahimi

/**
 * Seeds the `ExerciseAlias` table from the `exercise-translation.json` starter dictionary — the
 * messy real-world movement names coaches actually write, mapped to canonical Hevy exercise
 * titles. Run once after `refresh-catalog` has populated `HevyExerciseCatalog`, since this
 * resolves each canonical name to a template id against that table.
 *
 * The shipped dictionary reflects one gym's programming vocabulary. It is a useful starting
 * point rather than a universal mapping: expect to add entries for your own coaches' shorthand
 * as the unmapped-movement reports come in.
 *
 * Usage:
 *   CATALOG_TABLE_NAME=... ALIAS_TABLE_NAME=... npm run seed-aliases
 *
 * Both table names are required — take them from the data stack's CfnOutputs. There are
 * deliberately no defaults: a wrong guess here writes rows into somebody else's table.
 */
import { DynamoDBClient, PutItemCommand, ScanCommand } from '@aws-sdk/client-dynamodb';
import { marshall, unmarshall } from '@aws-sdk/util-dynamodb';
import * as fs from 'fs';
import * as path from 'path';

const SKIP_SENTINEL = '__SKIP__';
/** Dictionary marker for "this line is noise, never resolve it to an exercise". */
const UNDEFINED_SENTINEL = 'UNDEFINED';

function requiredEnv(name: string): string {
  const value = process.env[name];
  if (!value) {
    throw new Error(
      `${name} is required. Take it from the data stack's outputs, e.g.\n` +
        `  aws cloudformation describe-stacks --stack-name <YourPrefix>DataStack-Prod \\\n` +
        `    --query "Stacks[0].Outputs"`,
    );
  }
  return value;
}

const catalogTableName = requiredEnv('CATALOG_TABLE_NAME');
const aliasTableName = requiredEnv('ALIAS_TABLE_NAME');

// Mirrors `swh_core::matching::normalize::normalize` exactly — lowercase, strip punctuation to
// single spaces, trim. Both sides of this migration must use the same normalization or lookups
// won't line up with what the Rust code computes at runtime.
function normalize(input: string): string {
  const lower = input.toLowerCase();
  let out = '';
  let lastWasSpace = false;
  for (const ch of lower) {
    if (/[a-z0-9]/.test(ch)) {
      out += ch;
      lastWasSpace = false;
    } else if (!lastWasSpace) {
      out += ' ';
      lastWasSpace = true;
    }
  }
  return out.trim();
}

async function loadCatalogTitleIndex(client: DynamoDBClient): Promise<Map<string, string>> {
  const index = new Map<string, string>();
  let lastEvaluatedKey: Record<string, any> | undefined;
  do {
    const resp = await client.send(
      new ScanCommand({ TableName: catalogTableName, ExclusiveStartKey: lastEvaluatedKey }),
    );
    for (const item of resp.Items ?? []) {
      const row = unmarshall(item) as { id: string; title_normalized: string };
      index.set(row.title_normalized, row.id);
    }
    lastEvaluatedKey = resp.LastEvaluatedKey;
  } while (lastEvaluatedKey);
  return index;
}

async function main() {
  const client = new DynamoDBClient({});
  const raw = fs.readFileSync(path.join(__dirname, 'exercise-translation.json'), 'utf8');
  const oldDict: Record<string, string> = JSON.parse(raw);

  console.log(`Loading ${catalogTableName} for title -> id resolution...`);
  const catalogIndex = await loadCatalogTitleIndex(client);
  console.log(`Loaded ${catalogIndex.size} catalog entries.`);

  let seeded = 0;
  let skipped = 0;
  const unresolved: string[] = [];

  for (const [rawAlias, rawCanonical] of Object.entries(oldDict)) {
    const alias = normalize(rawAlias);
    const now = new Date().toISOString();

    if (rawCanonical === UNDEFINED_SENTINEL) {
      await client.send(
        new PutItemCommand({
          TableName: aliasTableName,
          Item: marshall({
            alias,
            canonical_title: SKIP_SENTINEL,
            hevy_template_id: '',
            source: 'seeded',
            updated_at: now,
          }),
        }),
      );
      skipped++;
      continue;
    }

    const hevyTemplateId = catalogIndex.get(normalize(rawCanonical));
    if (!hevyTemplateId) {
      unresolved.push(`"${rawAlias}" -> "${rawCanonical}" (no catalog match for "${rawCanonical}")`);
      continue;
    }

    await client.send(
      new PutItemCommand({
        TableName: aliasTableName,
        Item: marshall({
          alias,
          canonical_title: rawCanonical,
          hevy_template_id: hevyTemplateId,
          source: 'seeded',
          updated_at: now,
        }),
      }),
    );
    seeded++;
  }

  console.log(`\nSeeded ${seeded} resolved aliases, ${skipped} skip-sentinel entries.`);
  if (unresolved.length > 0) {
    console.log(`\n${unresolved.length} entries could NOT be resolved against the catalog:`);
    for (const line of unresolved) {
      console.log(`  - ${line}`);
    }
    console.log(
      '\nThese need manual review — either the catalog title differs slightly from the ' +
        'dictionary\'s canonical name, or the exercise no longer exists in Hevy\'s catalog. Add ' +
        'them by hand via the AWS console/CLI once resolved.',
    );
  }
}

main().catch((err) => {
  console.error('Seed script failed:', err);
  process.exit(1);
});
