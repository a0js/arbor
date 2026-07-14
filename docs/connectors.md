# Connectors

Connectors are the data ingestion layer for the Arbor indexer. They read entities and policies from external sources and populate the in-memory `Graph` that the indexer uses to build snapshots.

## Design

Connectors are configured via two YAML files:

- **`config/connectors.yaml`** — named connection definitions (credentials live here)
- **`config/entity_types.yaml`** — entity type and policy queries that reference connectors by name

This split keeps credentials separate from data model config. `entity_types.yaml` can be committed to git freely; `connectors.yaml` should be gitignored or delivered via a secret manager in production.

## Configuration

### `config/connectors.yaml`

Defines named connectors. Each key is a connector name referenced by entity type queries.

```yaml
connectors:
  main_db:
    type: postgres
    host: localhost
    port: 5432
    database: mydb
    user: arbor
    # password → ARBOR__CONNECTORS__MAIN_DB__PASSWORD

  hr_db:
    type: postgres
    host: hr-db.internal
    database: hr
    user: arbor_readonly
    # password → ARBOR__CONNECTORS__HR_DB__PASSWORD
```

Passwords are never written to this file. Inject them via environment variables using double-underscore separators for nested keys (e.g. `ARBOR__CONNECTORS__HR_DB__PASSWORD=secret`). For local development, use `config/connectors.local.yaml` (gitignored) instead.

### `config/entity_types.yaml`

Defines entity type queries and policy queries. Each entry references a named connector and provides a SQL query with a fixed output column contract.

```yaml
entity_types:
  - name: User
    connector: hr_db
    query: |
      SELECT
        u.id,
        u.username AS name,
        array_agg(ug.group_id) FILTER (WHERE ug.group_id IS NOT NULL) AS parent_ids
      FROM users u
      LEFT JOIN user_groups ug ON ug.user_id = u.id
      GROUP BY u.id, u.username

  - name: Group
    connector: main_db
    query: |
      SELECT id, name, NULL::uuid[] AS parent_ids
      FROM groups

  - name: File
    connector: main_db
    query: |
      SELECT id, name, ARRAY[folder_id]::uuid[] AS parent_ids
      FROM files
      WHERE deleted_at IS NULL

policies:
  - connector: main_db
    query: |
      SELECT id, name, policy_type, principal_id, resource_id, actions
      FROM policies
      WHERE active = true
```

## Query Column Contracts

Queries must return specific columns. The connector validates this at startup.

### Entity queries

| Column | Type | Description |
|---|---|---|
| `id` | `uuid` | Stable entity identifier |
| `name` | `text` | Display name |
| `parent_ids` | `uuid[]` | Parent entity IDs (nullable — NULL or empty means root) |

Use `array_agg(...) FILTER (WHERE ... IS NOT NULL)` for many-to-many parent relationships to avoid `{NULL}` arrays from LEFT JOINs with no matches.

### Policy queries

| Column | Type | Description |
|---|---|---|
| `id` | `uuid` | Stable policy identifier |
| `name` | `text` | Display name |
| `policy_type` | `text` | `"permit"` or `"forbid"` |
| `principal_id` | `uuid` | Principal entity ID |
| `resource_id` | `uuid` | Resource entity ID |
| `actions` | `uuid[]` | Action IDs this policy covers |

## Connector Types

### `postgres`

Connects to a PostgreSQL database via `sqlx`. Runs all queries for a given connector concurrently on a shared connection pool.

```yaml
type: postgres
host: localhost
port: 5432          # default: 5432
database: mydb
user: arbor
# password: injected via env var
```

### `csv`

Reads two CSV files — one for entities, one for policies. Useful for bootstrapping and offline testing without a live database.

```yaml
type: csv
entities_file: /data/entities.csv
policies_file: /data/policies.csv
```

**CSV column contracts** mirror the SQL query contracts:
- `entities.csv`: `id, name, type_name, parent_ids` (`parent_ids` is a semicolon-separated list of UUIDs)
- `policies.csv`: `id, name, policy_type, principal_id, resource_id, actions` (`actions` is a semicolon-separated list of UUIDs)

### `example` (dev/test only)

Uses the hardcoded example graph (`example_graph::build()`). No connection required. Useful for local development and integration tests.

```yaml
type: example
```

## Startup Validation

On startup, the indexer:

1. Loads `connectors.yaml` and `entity_types.yaml`
2. Validates that every `connector:` reference in `entity_types.yaml` resolves to a key in `connectors.yaml` — fails fast before opening any connections
3. Opens one connection pool per referenced connector
4. Runs entity and policy queries (concurrently within each connector)
5. Populates the `Graph`, then builds the initial snapshot

## Secret Injection

Passwords are never stored in YAML files committed to source control. Three options, applied in priority order:

1. **Environment variables** (production): `ARBOR__CONNECTORS__<NAME>__PASSWORD=secret`
2. **Local overrides** (development): `config/connectors.local.yaml` (gitignored)
3. **Secret manager** (production alternative): Mount secrets as files and reference via `config/connectors.production.yaml`

The `config` crate loads sources in this order, with later sources overriding earlier ones:
```
config/connectors.yaml
config/connectors.<RUN_MODE>.yaml   (e.g. connectors.production.yaml)
config/connectors.local.yaml
env vars (ARBOR__CONNECTORS__*)
```

## V2: Streaming Connectors

V1 connectors do a full load on startup (no incremental updates — restart the indexer to reload). V2 will add:

- **CDC connectors**: Change-data-capture via database triggers or Debezium, feeding a mutation event loop into `IndexerService`
- **Kafka/NATS connectors**: Entity/policy change events streamed in real time

See [pub-sub-protocol.md](./pub-sub-protocol.md) for the planned V2 notification protocol.
