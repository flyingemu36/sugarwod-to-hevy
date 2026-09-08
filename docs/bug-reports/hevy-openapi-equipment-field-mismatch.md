# Bug Report: `GET /v1/exercise_templates` response field doesn't match published OpenAPI spec

## Summary

The published OpenAPI spec for `GET /v1/exercise_templates` documents each exercise template as
having an `equipment_category` field (a fixed enum). The live API does not return this field at
all — it returns a field named `equipment` instead, containing a free-form string that happens to
overlap with (but is not formally documented as) the enum's value set.

Any client generated or hand-written strictly from the published spec will fail to deserialize
real responses from this endpoint.

If you're wondering why it's been committed, it's a reminder to send the bug report ;)

## Affected Endpoint

```
GET https://api.hevyapp.com/v1/exercise_templates
```

## Spec Source

Pulled directly from the docs site's own Swagger UI bootstrap script (not a stale/cached copy):

```
https://api.hevyapp.com/docs/swagger-ui-init.js
```

Relevant excerpt from the embedded `swaggerDoc.components.schemas.ExerciseTemplate`:

```json
{
  "type": "object",
  "properties": {
    "id": { "type": "string", "description": "The exercise template ID." },
    "title": { "type": "string", "description": "The exercise title." },
    "type": { "type": "string", "description": "The exercise type." },
    "primary_muscle_group": { "type": "string" },
    "secondary_muscle_groups": { "type": "array", "items": { "type": "string" } },
    "equipment_category": { "$ref": "#/components/schemas/EquipmentCategory" },
    "is_custom": { "type": "boolean" }
  }
}
```

```json
{
  "EquipmentCategory": {
    "type": "enum",
    "enum": ["none", "barbell", "dumbbell", "kettlebell", "machine", "plate", "resistance_band", "suspension", "other"],
    "example": "barbell"
  }
}
```

## Steps to Reproduce

```bash
curl -s -H "api-key: <valid Hevy Pro API key>" \
  "https://api.hevyapp.com/v1/exercise_templates?page=1&pageSize=5"
```

## Expected Result (per the published spec)

Each object in `exercise_templates[]` should include an `equipment_category` field, e.g.:

```json
{ "...": "...", "equipment_category": "barbell" }
```

## Actual Result (live API, response captured 2026-08-01)

```json
{
  "page": 1,
  "page_count": 126,
  "exercise_templates": [
    {
      "id": "3BC06AD3",
      "title": "21s Bicep Curl",
      "type": "weight_reps",
      "primary_muscle_group": "biceps",
      "secondary_muscle_groups": [],
      "equipment": "barbell",
      "is_custom": false
    },
    {
      "id": "8191d3c1-42d1-4c07-bea5-c437bf7fe022",
      "title": "21s Bicep Curl DB",
      "type": "weight_reps",
      "primary_muscle_group": "biceps",
      "secondary_muscle_groups": [],
      "equipment": "dumbbell",
      "is_custom": true
    },
    {
      "id": "B4F2FF72",
      "title": "Ab Scissors",
      "type": "reps_only",
      "primary_muscle_group": "abdominals",
      "secondary_muscle_groups": [],
      "equipment": "none",
      "is_custom": false
    }
  ]
}
```

No `equipment_category` key is present anywhere in the response. The field is named `equipment`,
and its observed values (`"barbell"`, `"dumbbell"`, `"none"`, `"other"`, ...) are consistent with
the documented `EquipmentCategory` enum's value set — this appears to be the same underlying
concept, just under a different (undocumented) key name and without the enum's `type: "enum"`
constraint formally attached to it.

## Impact

- Any strictly-typed client generated from the published spec (OpenAPI codegen, or a hand-written
  client that trusts the docs) will fail to deserialize `equipment_category` because the key
  doesn't exist in real responses, and will silently miss `equipment` entirely because it isn't
  documented.
- Concretely hit this building a Rust client against `aws-sdk`-style strict deserialization: a
  struct field mapped to `equipment_category` (per the spec) produced a hard deserialization
  error (`missing field`) against every real response, resolved only after inspecting the raw
  wire response directly instead of trusting the docs.

## Suggested Fix

One of:
1. Rename the live response field from `equipment` to `equipment_category` to match the spec, or
2. Update the spec's `ExerciseTemplate.equipment_category` property to `equipment`, and formally
   document its value set (currently only implied by the orphaned `EquipmentCategory` schema).

Either is a breaking change for existing integrations depending on current behavior, so this is
likely best paired with a version bump or an announced transition window if the field is renamed
on the live API side.

## Environment / Metadata

| | |
|---|---|
| Endpoint | `GET /v1/exercise_templates` |
| Spec version at time of report | `"version": "0.0.1"` (per `swaggerDoc.info.version`) |
| Date observed | 2026-08-01 |
| Auth | `api-key` header, Hevy Pro account |
| Also worth spot-checking | Whether other endpoints/schemas referencing `EquipmentCategory` or similarly-shaped fields have the same doc/live drift — this was found incidentally while integration-testing a single endpoint, not from an exhaustive audit of the spec |

## Contact

Per the spec's own `info.description`, API questions go to **pedro@hevyapp.com**.
