#!/usr/bin/env bash
# Pulls table/parameter names from the deployed gamma stack's CfnOutputs, assumes the
# least-privilege IntegrationTestRole (NOT root or a broad admin session — see the comment on
# IntegrationTestRole in compute-stack.ts for why that distinction matters), and runs the Rust
# integration test suite (rust/crates/swh-core/tests/aws_integration.rs) under those temporary,
# scoped credentials.
#
# Prerequisites (one-time):
#   1. A non-root IAM identity that can call sts:AssumeRole (the account ROOT user is blocked
#      from ever assuming a role — an AWS platform rule, not an IAM policy setting). Configure it
#      as a named AWS CLI profile and point AWS_PROFILE_FOR_TESTS at it.
#   2. Deploy the gamma stacks:
#        npx cdk deploy -c affiliateId=<slug> \
#          "${RESOURCE_PREFIX:-SugarwodToHevy}DataStack-Gamma" \
#          "${RESOURCE_PREFIX:-SugarwodToHevy}ComputeStack-Gamma"
#   3. Create the gamma Discord webhook parameter (name is a CfnOutput of the data stack):
#        aws ssm put-parameter --name /<slug>/gamma/discord-webhook-url \
#          --type SecureString --value "https://discord.com/api/webhooks/..."
#   4. Ensure the Hevy API key secret exists (shared by both stages; its id is the HevySecretId
#      CfnOutput of the data stack).
#
# Configuration — all overridable, nothing here is specific to one deployment:
#   AWS_PROFILE_FOR_TESTS  AWS CLI profile to assume the role from   (required)
#   AWS_REGION_FOR_TESTS   region the stacks are deployed in         (default: your CLI default)
#   RESOURCE_PREFIX        must match the `resourcePrefix` context   (default: SugarwodToHevy)
#   SUGARWOD_AFFILIATE_ID  gym slug for the live SugarWOD fetch test (required)
#   LOCAL_TIMEZONE         IANA zone deciding "today"                (default: UTC)
#   DISCORD_ENABLED        set to "true" only if you deployed with
#                          -c discordEnabled=true                    (default: false)
#
# Usage: SUGARWOD_AFFILIATE_ID=<slug> AWS_PROFILE_FOR_TESTS=<profile> ./scripts/run-integration-tests.sh

set -euo pipefail

RESOURCE_PREFIX="${RESOURCE_PREFIX:-SugarwodToHevy}"
DATA_STACK_NAME="${RESOURCE_PREFIX}DataStack-Gamma"
COMPUTE_STACK_NAME="${RESOURCE_PREFIX}ComputeStack-Gamma"
LOCAL_TIMEZONE="${LOCAL_TIMEZONE:-UTC}"

if [[ -z "${AWS_PROFILE_FOR_TESTS:-}" ]]; then
  echo "AWS_PROFILE_FOR_TESTS must name an AWS CLI profile that can assume IntegrationTestRole." >&2
  echo "(The account root user cannot assume roles at all — use a dedicated IAM identity.)" >&2
  exit 1
fi
if [[ -z "${SUGARWOD_AFFILIATE_ID:-}" ]]; then
  echo "SUGARWOD_AFFILIATE_ID must be set to your gym's SugarWOD affiliate slug." >&2
  exit 1
fi

PROFILE="${AWS_PROFILE_FOR_TESTS}"
REGION="${AWS_REGION_FOR_TESTS:-$(aws configure get region --profile "${PROFILE}")}"
if [[ -z "${REGION}" ]]; then
  echo "No region: set AWS_REGION_FOR_TESTS or configure one on profile '${PROFILE}'." >&2
  exit 1
fi

echo "Using AWS profile '${PROFILE}' in region '${REGION}' (override with AWS_PROFILE_FOR_TESTS / AWS_REGION_FOR_TESTS)."
aws sts get-caller-identity --profile "${PROFILE}" --region "${REGION}" --query 'Arn' --output text
echo

get_output() {
  local stack="$1" key="$2"
  aws cloudformation describe-stacks --profile "${PROFILE}" --region "${REGION}" --stack-name "${stack}" --query 'Stacks[0].Outputs' --output json \
    | python3 -c "import json,sys; print(next(o['OutputValue'] for o in json.load(sys.stdin) if o['OutputKey']=='${key}'))"
}

echo "Reading outputs from ${DATA_STACK_NAME}..."
ALIAS_TABLE_NAME=$(get_output "${DATA_STACK_NAME}" ExerciseAliasTableName)
CATALOG_TABLE_NAME=$(get_output "${DATA_STACK_NAME}" HevyExerciseCatalogTableName)
ROUTINE_MAP_TABLE_NAME=$(get_output "${DATA_STACK_NAME}" RoutineMapTableName)
PERSONAL_MAX_TABLE_NAME=$(get_output "${DATA_STACK_NAME}" PersonalMaxTableName)
# Notifications are off by default (CDK context `discordEnabled`). Only pass the parameter name
# through when they are on; the webhook test skips when it is absent.
if [[ "${DISCORD_ENABLED:-false}" == "true" ]]; then
  DISCORD_WEBHOOK_PARAM_NAME=$(get_output "${DATA_STACK_NAME}" DiscordWebhookParamName)
else
  DISCORD_WEBHOOK_PARAM_NAME=""
fi

HEVY_SECRET_ID=$(get_output "${DATA_STACK_NAME}" HevySecretId)

echo "Reading IntegrationTestRole ARN from ${COMPUTE_STACK_NAME}..."
ROLE_ARN=$(get_output "${COMPUTE_STACK_NAME}" IntegrationTestRoleArn)

echo "ALIAS_TABLE_NAME=${ALIAS_TABLE_NAME}"
echo "CATALOG_TABLE_NAME=${CATALOG_TABLE_NAME}"
echo "ROUTINE_MAP_TABLE_NAME=${ROUTINE_MAP_TABLE_NAME}"
echo "PERSONAL_MAX_TABLE_NAME=${PERSONAL_MAX_TABLE_NAME}"
echo "DISCORD_WEBHOOK_PARAM_NAME=${DISCORD_WEBHOOK_PARAM_NAME}"
echo "HEVY_SECRET_ID=${HEVY_SECRET_ID}"
echo "IntegrationTestRoleArn=${ROLE_ARN}"
echo

echo "Assuming ${ROLE_ARN}..."
CREDS=$(aws sts assume-role --profile "${PROFILE}" --region "${REGION}" --role-arn "${ROLE_ARN}" --role-session-name integration-test --query 'Credentials' --output json)
ASSUMED_ACCESS_KEY_ID=$(echo "${CREDS}" | python3 -c "import json,sys; print(json.load(sys.stdin)['AccessKeyId'])")
ASSUMED_SECRET_ACCESS_KEY=$(echo "${CREDS}" | python3 -c "import json,sys; print(json.load(sys.stdin)['SecretAccessKey'])")
ASSUMED_SESSION_TOKEN=$(echo "${CREDS}" | python3 -c "import json,sys; print(json.load(sys.stdin)['SessionToken'])")

cd "$(dirname "$0")/../../rust"
echo "Running integration tests (serial, under IntegrationTestRole, real AWS + real SugarWOD/Hevy GETs)..."

# Scoped to this one command only — doesn't touch the calling shell's ambient credentials.
env \
  -u AWS_PROFILE \
  AWS_ACCESS_KEY_ID="${ASSUMED_ACCESS_KEY_ID}" \
  AWS_SECRET_ACCESS_KEY="${ASSUMED_SECRET_ACCESS_KEY}" \
  AWS_SESSION_TOKEN="${ASSUMED_SESSION_TOKEN}" \
  AWS_REGION="${REGION}" \
  ALIAS_TABLE_NAME="${ALIAS_TABLE_NAME}" \
  CATALOG_TABLE_NAME="${CATALOG_TABLE_NAME}" \
  ROUTINE_MAP_TABLE_NAME="${ROUTINE_MAP_TABLE_NAME}" \
  PERSONAL_MAX_TABLE_NAME="${PERSONAL_MAX_TABLE_NAME}" \
  DISCORD_WEBHOOK_PARAM_NAME="${DISCORD_WEBHOOK_PARAM_NAME}" \
  HEVY_SECRET_ID="${HEVY_SECRET_ID}" \
  SUGARWOD_AFFILIATE_ID="${SUGARWOD_AFFILIATE_ID}" \
  LOCAL_TIMEZONE="${LOCAL_TIMEZONE}" \
  cargo test --workspace -- --ignored --test-threads=1
