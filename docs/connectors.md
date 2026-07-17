# Connectors

Connectors are the data ingestion layer for the Arbor indexer. They read entities and policies from external sources and populate the in-memory `Graph` that the indexer uses to build snapshots.

## Design

Connectors are configured via two YAML files:

- **`config/connectors.yaml`** — named connection definitions (credentials live here)
- **`config/data_model.yaml`** — entity type and policy queries that reference connectors by name

This split keeps credentials separate from data model config. `data_model.yaml` can be committed to git freely; `connectors.yaml` should be gitignored or delivered via a secret manager in production.

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

### `config/data_model.yaml`

Defines entity type queries and policy queries, under the `entity_types:` and `policies:` keys respectively. Each entry references a named connector and provides a SQL query with a fixed output column contract.

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

Reads entities and policies from CSV files. Useful for bootstrapping and offline testing without a live database, and for one-off imports from systems (HRIS exports, spreadsheets) whose CSVs won't have Arbor's field names as headers.

`connectors.yaml` holds only the file location — the same split the `postgres` connector uses for credentials:

```yaml
connectors:
  employees_csv:
    type: csv
    file: employees.csv    # resolved relative to connectors.yaml's directory
  policies_csv:
    type: csv
    file: policies.csv
```

`data_model.yaml` is where the data model lives, same as for `postgres` — except instead of a SQL query whose `AS` aliases do the column mapping, a CSV connector declares an explicit `columns:` mapping from Arbor's logical fields to that file's actual header names:

**One file per entity type.** Each `entity_types` entry names exactly one entity type and one connector, so a full dataset needs one entry (and typically one CSV) per type. Most rows have **at most one parent** (`parent_id`, optional) — this matches how most external systems export hierarchy (a single `manager_id` column). Sources with genuine multi-parent structure (e.g. group membership layered on top of an org chart) can instead use `parent_ids`, a `;`-separated list of UUIDs mirroring the `actions` column's semicolon-list convention below — this is the CSV-side equivalent of the Postgres connector's `parent_ids uuid[]` column. Both `parent_id` and `parent_ids` may be set together; the resulting parents are their union, deduplicated.

**Attribute columns (ABAC).** An `entity_types` entry can also declare `attributes:` — a list of `{path, column, value_type}` mappings, one per scalar CSV column. `path` is a dotted string (e.g. `consent_flags.share_with_specialists`); nesting comes entirely from a multi-segment path, never from the column's own value — there's no embedded JSON/object syntax in a cell. `value_type` is `string` | `integer` | `float` | `bool`, since a raw CSV string can't be typed by inspection alone.

```yaml
entity_types:
  - name: Employee
    connector: employees_csv
    columns:
      id: emp_id                 # required
      name: full_name            # required
      parent_id: manager_id      # optional; omit entirely for root-level entities
      parent_ids: group_ids      # optional; ';'-separated UUIDs for additional parents (DAG)
      attributes:                # optional; scalar columns mapped to (possibly nested) attribute paths
        - path: consent_flags.share_with_specialists
          column: share_specialists
          value_type: bool
        - path: age
          column: age
          value_type: integer

policies:
  - connector: policies_csv
    columns:
      id: policy_id
      name: policy_name
      policy_type: ptype                # "permit" | "forbid"
      principal_type: principal_kind    # "entity" | "entity_with_descendants" | "entity_type" | "all"
      principal_id: principal           # a UUID for entity/entity_with_descendants,
                                         # a type name for entity_type, ignored for all
      resource_type: resource_kind
      resource_id: resource
      actions: action_names             # ';'-separated action *names*, not UUIDs
      action_sets: set_names            # optional; ';'-separated action-set names
      condition: condition_text         # optional; free-text ABAC condition (see below)

actions:
  - connector: actions_csv
    columns:
      name: action_name
      entity_type: scoped_type          # descriptive only, not part of the action's identity
      description: notes                # optional

action_sets:
  - connector: action_sets_csv
    columns:
      name: set_name
      actions: member_actions           # ';'-separated action names
      description: notes                # optional
```

**Condition grammar.** `policies.csv`'s optional `condition` column carries one free-text expression, parsed by `crates/arbor-connectors/src/condition_parser.rs` into arbor's `Condition` AST (`or` binds loosest, `not` tightest, `()` groups):

```text
expr       := or_expr
or_expr    := and_expr ( "or" and_expr )*
and_expr   := unary ( "and" unary )*
unary      := "not" unary | "(" expr ")" | comparison
comparison := operand ( binop operand )?
binop      := "==" | "!=" | "<=" | ">=" | "<" | ">"
            | "in" | "contains_all" | "contains_any" | "contains"
            | "starts_with" | "ends_with" | "string_contains" | "like"
            | "in_hierarchy"
operand    := variable | string | number | "true" | "false" | set
variable   := ("principal" | "resource" | "context") ( "." ident )*
set        := "(" operand ( "," operand )* ")"
```

Examples:

```text
resource.consent_flags.share_with_specialists == true
not (resource.restricted == true) or principal.clearance == "break_glass"
resource.status in ("active", "pending")
principal in_hierarchy "018e0000-0000-7000-8000-000000000001"
```

A variable's dotted path is resolved to attribute names the same lazy way an `entity_type` string is resolved to an `EntityTypeId` — created if not yet registered, not required to appear in any `attributes:` mapping first. `in_hierarchy`'s right-hand side must be a **quoted literal entity UUID**, resolved against the graph at build time (an unregistered UUID is a hard ingestion error, not a silent no-op) — consistent with `policies.csv`'s own `principal_id`/`resource_id` columns already requiring literal UUIDs rather than names.

Not supported yet (no ingestion path needs them): `has_attribute`, `is_type`, `in_network`. Also deliberately not a condition-language concern: role checks like "is this principal a physician" belong in policy *targeting* (`principal_type: entity_type`, `principal_id: Physician`) or action sets, not in an attribute comparison — a condition should test entity *data*, not stand in for a target filter.

`entity_type` policy targets are looked up (and auto-registered, matching some `entity_types` entry's `name:`) by string, not a pre-resolved `EntityTypeId` — so `resource_type: entity_type` / `resource_id: File` in a policy row just needs to name the same string used as some entry's `name:`.

**Actions and action sets are referenced by name, not UUID** — the way a real export actually has them (`"read_chart"`, never an internal Arbor UUID). `arbor-connectors` derives each one's UUID from its name via a single internal function (`action_id_for_name` / `action_set_id_for_name`), used consistently by `actions.csv`/`action_sets.csv` ingestion *and* by `policies.csv`'s `actions`/`action_sets` name lookups — there is deliberately no UUID column anywhere in this path, so there's nothing for two independently-written pieces of code (e.g. a dataset generator and the indexer) to disagree on. If `data_model.yaml` declares no `actions:` section at all, the indexer falls back to a standard `read`/`write`/`delete` set scoped to `File`, for compatibility with data models that predate the `actions:`/`action_sets:` sections.

See `crates/arbor-connectors/src/lib.rs` for the loader (`load_connector_config`, `load_data_model_config`, `load_all`) and `benches/src/bin/gen_company_dataset.rs` / `benches/src/bin/gen_healthcare_dataset.rs` for generators that produce a full multi-file dataset plus matching `connectors.yaml` / `data_model.yaml`.

### `example` (dev/test only)

Uses the hardcoded example graph (`example_graph::build()`). No connection required. Useful for local development and integration tests.

```yaml
type: example
```

## Startup Validation

On startup, the indexer:

1. Loads `connectors.yaml` and `data_model.yaml`
2. Validates that every `connector:` reference in `data_model.yaml` resolves to a key in `connectors.yaml` — fails fast before opening any connections
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
