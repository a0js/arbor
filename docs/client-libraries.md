# Client Libraries

This document describes Arbor's client libraries for different programming languages.

## Overview

Arbor provides **unified client libraries** for multiple languages. Each library:
- ✅ Supports **both transports** (Unix Domain Socket + gRPC)
- ✅ **Auto-detects** the best transport based on deployment
- ✅ Uses the **same Protobuf protocol** for both transports
- ✅ Provides **connection pooling** and reuse
- ✅ Offers **identical API** regardless of transport

## Architecture

```
┌─────────────────────────────────────────────────┐
│        Protocol Definition (.proto)             │
│                                                 │
│  service AuthorizationService {                │
│    rpc Check(CheckRequest) returns (CheckResponse);
│    rpc ListResources(...) returns (...);       │
│    rpc ListPrincipals(...) returns (...);      │
│    rpc ListActions(...) returns (...);         │
│  }                                              │
└─────────────────┬───────────────────────────────┘
                  │
         protoc (code generation)
                  │
    ┌─────────────┼─────────────┐
    │             │             │
    ▼             ▼             ▼
┌─────────┐  ┌─────────┐  ┌─────────┐
│ Node.js │  │  Rust   │  │ Python  │
│ Client  │  │ Client  │  │ Client  │
└────┬────┘  └────┬────┘  └────┬────┘
     │            │            │
     │   Auto-detect transport │
     │            │            │
     ▼            ▼            ▼
┌────────────────────────────────────┐
│  Transport Selection               │
│                                    │
│  if socket_exists:                 │
│    use UnixSocketTransport (30-80μs)
│  else:                             │
│    use GrpcTransport (1-5ms)       │
└────────────────────────────────────┘
```

## Design Principles

### 1. One Library Per Language

**Not separate libraries** for each transport:

```
❌ BAD:
  @arbor/client-grpc      (for network)
  @arbor/client-socket    (for sidecar)

✅ GOOD:
  @arbor/client           (supports both)
```

### 2. Transport Abstraction

Internal abstraction hides transport details:

```javascript
// Public API (same for all transports)
class ArborClient {
  async check(principalId, actionId, resourceId, context) { ... }
}

// Internal implementation
class UnixSocketTransport { ... }
class GrpcTransport { ... }
```

### 3. Auto-Detection

Client automatically chooses the best transport:

```javascript
// Auto-detect based on socket existence
const client = new ArborClient();

// Checks if /var/run/arbor.sock exists:
// - Yes → UnixSocketTransport (30-80μs)
// - No  → GrpcTransport (1-5ms)
```

### 4. Configuration Override

Allow explicit transport selection:

```javascript
// Force sidecar mode
const client = new ArborClient({ mode: 'sidecar' });

// Force gRPC mode
const client = new ArborClient({
  mode: 'grpc',
  address: 'authz.example.com:8080'
});
```

## Protocol Definition

### Protobuf Schema

```protobuf
// arbor/v1/arbor.proto
syntax = "proto3";
package arbor.v1;

// Authorization service
service AuthorizationService {
  // Check if principal can perform action on resource
  rpc Check(CheckRequest) returns (CheckResponse);

  // List resources principal can access
  rpc ListResources(ListResourcesRequest) returns (ListResourcesResponse);

  // List principals that can access resource
  rpc ListPrincipals(ListPrincipalsRequest) returns (ListPrincipalsResponse);

  // List actions principal can perform on resource
  rpc ListActions(ListActionsRequest) returns (ListActionsResponse);
}

// Check request
message CheckRequest {
  bytes principal_id = 1;  // UUID (16 bytes)
  bytes action_id = 2;     // UUID (16 bytes)
  bytes resource_id = 3;   // UUID (16 bytes)
  map<string, AttributeValue> context = 4;
  bool explain = 5;        // Include reason in response
}

// Check response
message CheckResponse {
  Decision decision = 1;
  optional Reason reason = 2;  // Only if explain=true
  uint64 snapshot_version = 3;
}

enum Decision {
  DECISION_UNSPECIFIED = 0;
  DECISION_PERMIT = 1;
  DECISION_DENY = 2;
}

message Reason {
  oneof reason {
    NoApplicablePolicies no_policies = 1;
    ForbiddenBy forbidden_by = 2;
    PermittedBy permitted_by = 3;
    ConditionFailed condition_failed = 4;
  }
}

message NoApplicablePolicies {}

message ForbiddenBy {
  bytes policy_id = 1;
  string policy_name = 2;
}

message PermittedBy {
  bytes policy_id = 1;
  string policy_name = 2;
}

message ConditionFailed {
  bytes policy_id = 1;
  string condition = 2;
}

// Attribute value (for context)
message AttributeValue {
  oneof value {
    ScalarValue scalar = 1;
    bytes entity_ref = 2;
    AttributeSet set = 3;
    AttributeObject object = 4;
  }
}

message ScalarValue {
  oneof value {
    string string_value = 1;
    int64 int_value = 2;
    double float_value = 3;
    bool bool_value = 4;
    int64 timestamp_value = 5;
  }
}

message AttributeSet {
  repeated AttributeValue values = 1;
}

message AttributeObject {
  map<string, AttributeValue> fields = 1;
}

// List resources request
message ListResourcesRequest {
  bytes principal_id = 1;
  bytes action_id = 2;
  optional string resource_type = 3;  // Filter by type
  map<string, AttributeValue> context = 4;
  optional uint32 limit = 5;
  optional uint32 offset = 6;
}

message ListResourcesResponse {
  repeated Entity resources = 1;
  uint32 total_count = 2;
  uint64 snapshot_version = 3;
}

message Entity {
  bytes id = 1;
  string name = 2;
  string entity_type = 3;
  repeated bytes parents = 4;
  map<string, AttributeValue> attributes = 5;
}

// ... similar for ListPrincipals, ListActions
```

## Transport Protocols

### Unix Domain Socket Protocol

**Wire format**: Length-prefixed Protobuf

```
┌────────────┬─────────────────────┐
│ Length     │ Protobuf Message    │
│ (4 bytes)  │ (variable length)   │
│ uint32 LE  │                     │
└────────────┴─────────────────────┘
```

**Example**:

```
Request:
  [0x1A, 0x00, 0x00, 0x00]  // Length: 26 bytes
  [protobuf bytes...]        // CheckRequest

Response:
  [0x0F, 0x00, 0x00, 0x00]  // Length: 15 bytes
  [protobuf bytes...]        // CheckResponse
```

**Optimizations**:
- `setNoDelay(true)` to disable Nagle's algorithm
- Abstract sockets on Linux (prefix with `\0`)
- Persistent connection (no reconnect per request)
- Fixed-size buffers to avoid allocations

### gRPC Protocol

**Wire format**: Standard gRPC/HTTP2

Uses the same Protobuf messages but over HTTP/2 with gRPC framing.

## Client Library Implementations

### Node.js Client

**Package**: `@arbor/client`

**Installation**:
```bash
npm install @arbor/client
```

**API**:

```javascript
const { ArborClient } = require('@arbor/client');

// Auto-detect mode
const client = new ArborClient();

// Explicit sidecar mode
const client = new ArborClient({
  mode: 'sidecar',
  socketPath: '/var/run/arbor.sock'
});

// Explicit gRPC mode
const client = new ArborClient({
  mode: 'grpc',
  address: 'authz.example.com:8080',
  credentials: grpc.credentials.createSsl() // Optional TLS
});

// Check authorization
const result = await client.check(
  principalId,   // UUID string or Buffer
  actionId,
  resourceId,
  { tier: 'gold' }  // Context attributes (optional)
);

if (result.decision === 'permit') {
  console.log('Access granted');
} else {
  console.log('Access denied:', result.reason);
}

// List resources
const resources = await client.listResources(
  principalId,
  actionId,
  'document',  // Resource type filter (optional)
  {},          // Context
  { limit: 100, offset: 0 }
);

console.log(`Can access ${resources.resources.length} documents`);

// Close connection
await client.close();
```

**Implementation Structure**:

```javascript
class ArborClient {
  constructor(options) {
    this.transport = this._createTransport(options);
  }

  _createTransport(options) {
    if (options.mode === 'sidecar') {
      return new UnixSocketTransport(options);
    } else if (options.mode === 'grpc') {
      return new GrpcTransport(options);
    } else {
      // Auto-detect
      const socketPath = options.socketPath || '/var/run/arbor.sock';
      if (fs.existsSync(socketPath)) {
        return new UnixSocketTransport({ socketPath });
      } else {
        const address = options.address || 'localhost:8080';
        return new GrpcTransport({ address });
      }
    }
  }

  async check(principalId, actionId, resourceId, context = {}) {
    return this.transport.check(principalId, actionId, resourceId, context);
  }

  async close() {
    return this.transport.close();
  }
}

class UnixSocketTransport {
  constructor({ socketPath }) {
    this.socket = net.connect(socketPath);
    this.socket.setNoDelay(true); // Disable Nagle
    this.requestId = 0;
    this.pending = new Map();
    this._setupHandlers();
  }

  async check(principalId, actionId, resourceId, context) {
    const requestId = ++this.requestId;

    const request = CheckRequest.encode({
      principal_id: Buffer.from(principalId),
      action_id: Buffer.from(actionId),
      resource_id: Buffer.from(resourceId),
      context: this._encodeContext(context),
    }).finish();

    // Length-prefixed send
    const length = Buffer.alloc(4);
    length.writeUInt32LE(request.length, 0);

    return new Promise((resolve, reject) => {
      this.pending.set(requestId, { resolve, reject });
      this.socket.write(Buffer.concat([length, request]));
    });
  }

  _setupHandlers() {
    let buffer = Buffer.alloc(0);

    this.socket.on('data', (chunk) => {
      buffer = Buffer.concat([buffer, chunk]);

      while (buffer.length >= 4) {
        const length = buffer.readUInt32LE(0);

        if (buffer.length < 4 + length) {
          break; // Wait for more data
        }

        const message = buffer.slice(4, 4 + length);
        buffer = buffer.slice(4 + length);

        const response = CheckResponse.decode(message);
        const pending = this.pending.get(response.requestId);

        if (pending) {
          this.pending.delete(response.requestId);
          pending.resolve(response);
        }
      }
    });
  }
}

class GrpcTransport {
  constructor({ address, credentials }) {
    const packageDef = protoLoader.loadSync('arbor.proto');
    const proto = grpc.loadPackageDefinition(packageDef);

    this.client = new proto.arbor.v1.AuthorizationService(
      address,
      credentials || grpc.credentials.createInsecure()
    );
  }

  async check(principalId, actionId, resourceId, context) {
    return new Promise((resolve, reject) => {
      this.client.Check({
        principal_id: Buffer.from(principalId),
        action_id: Buffer.from(actionId),
        resource_id: Buffer.from(resourceId),
        context: this._encodeContext(context),
      }, (err, response) => {
        if (err) reject(err);
        else resolve(response);
      });
    });
  }
}
```

**Performance**:
- Sidecar mode: 50-80μs (Node.js has ~20μs overhead vs Rust)
- gRPC mode: 1-5ms (network-dependent)

---

### Rust Client

**Crate**: `arbor-client`

**Installation**:
```toml
[dependencies]
arbor-client = "0.1"
```

**API**:

```rust
use arbor_client::{ArborClient, ClientConfig, Mode};
use uuid::Uuid;

#[tokio::main]
async fn main() -> Result<()> {
    // Auto-detect mode
    let mut client = ArborClient::new(ClientConfig::default()).await?;

    // Explicit sidecar mode
    let mut client = ArborClient::new(ClientConfig {
        mode: Mode::Sidecar,
        socket_path: Some("/var/run/arbor.sock".into()),
        ..Default::default()
    }).await?;

    // Explicit gRPC mode
    let mut client = ArborClient::new(ClientConfig {
        mode: Mode::Grpc,
        address: Some("authz.example.com:8080".into()),
        ..Default::default()
    }).await?;

    // Check authorization
    let result = client.check(
        principal_id,
        action_id,
        resource_id,
        Attributes::from([("tier", "gold")]),
    ).await?;

    if result.decision == Decision::Permit {
        println!("Access granted");
    } else {
        println!("Access denied: {:?}", result.reason);
    }

    // List resources
    let resources = client.list_resources(
        principal_id,
        action_id,
        Some("document"),  // Type filter
        Attributes::default(),
        ListOptions { limit: Some(100), offset: None },
    ).await?;

    println!("Can access {} documents", resources.resources.len());

    Ok(())
}
```

**Implementation Structure**:

```rust
pub struct ArborClient {
    transport: Box<dyn Transport>,
}

#[async_trait]
trait Transport: Send + Sync {
    async fn check(
        &mut self,
        principal_id: Uuid,
        action_id: Uuid,
        resource_id: Uuid,
        context: Attributes,
    ) -> Result<CheckResponse>;
}

struct UnixSocketTransport {
    stream: UnixStream,
    request_id: u64,
}

#[async_trait]
impl Transport for UnixSocketTransport {
    async fn check(...) -> Result<CheckResponse> {
        self.request_id += 1;

        let request = CheckRequest {
            principal_id: principal_id.as_bytes().to_vec(),
            action_id: action_id.as_bytes().to_vec(),
            resource_id: resource_id.as_bytes().to_vec(),
            context: context.into(),
            explain: false,
        };

        // Encode protobuf
        let bytes = request.encode_to_vec();

        // Write length-prefixed
        let len = (bytes.len() as u32).to_le_bytes();
        self.stream.write_all(&len).await?;
        self.stream.write_all(&bytes).await?;

        // Read response
        let mut len_buf = [0u8; 4];
        self.stream.read_exact(&mut len_buf).await?;
        let len = u32::from_le_bytes(len_buf) as usize;

        let mut response_buf = vec![0u8; len];
        self.stream.read_exact(&mut response_buf).await?;

        let response = CheckResponse::decode(&response_buf[..])?;
        Ok(response)
    }
}

struct GrpcTransport {
    client: AuthorizationServiceClient<Channel>,
}

#[async_trait]
impl Transport for GrpcTransport {
    async fn check(...) -> Result<CheckResponse> {
        let request = tonic::Request::new(CheckRequest { ... });
        let response = self.client.check(request).await?;
        Ok(response.into_inner())
    }
}
```

**Performance**:
- Sidecar mode: 30-50μs (native performance)
- gRPC mode: 1-5ms (network-dependent)

---

### Python Client

**Package**: `arbor-client`

**Installation**:
```bash
pip install arbor-client
```

**API**:

```python
from arbor_client import ArborClient
import uuid

# Auto-detect mode
client = ArborClient()

# Explicit sidecar mode
client = ArborClient(mode='sidecar', socket_path='/var/run/arbor.sock')

# Explicit gRPC mode
client = ArborClient(mode='grpc', address='authz.example.com:8080')

# Check authorization
result = client.check(
    principal_id=uuid.UUID('...'),
    action_id=uuid.UUID('...'),
    resource_id=uuid.UUID('...'),
    context={'tier': 'gold'}
)

if result.decision == 'permit':
    print('Access granted')
else:
    print(f'Access denied: {result.reason}')

# List resources
resources = client.list_resources(
    principal_id=uuid.UUID('...'),
    action_id=uuid.UUID('...'),
    resource_type='document',
    context={},
    limit=100
)

print(f'Can access {len(resources.resources)} documents')

# Close connection
client.close()
```

**Performance**:
- Sidecar mode: 60-100μs (Python has ~30-50μs overhead)
- gRPC mode: 1-5ms (network-dependent)

---

## Performance Comparison

| Language | Sidecar (UDS) | gRPC (localhost) | gRPC (network) |
|----------|---------------|------------------|----------------|
| Rust     | 30-50μs       | 200-500μs        | 1-5ms          |
| Node.js  | 50-80μs       | 300-600μs        | 1-5ms          |
| Python   | 60-100μs      | 400-700μs        | 1-5ms          |
| Go       | 35-60μs       | 250-550μs        | 1-5ms          |

**Note**: Sidecar mode gives 5-10x faster latency than network gRPC.

## Configuration

### Environment Variables

All client libraries support these environment variables:

```bash
# Transport mode
ARBOR_MODE=sidecar          # or "grpc", or "auto" (default)

# Sidecar configuration
ARBOR_SOCKET_PATH=/var/run/arbor.sock

# gRPC configuration
ARBOR_GRPC_ADDRESS=authz.example.com:8080
ARBOR_GRPC_TLS=true

# Connection settings
ARBOR_CONNECT_TIMEOUT=5s
ARBOR_REQUEST_TIMEOUT=30s
ARBOR_RETRY_ATTEMPTS=3
```

### Configuration Files

Example configuration (varies by language):

```yaml
# config.yaml
arbor:
  mode: auto  # auto, sidecar, or grpc

  sidecar:
    socket_path: /var/run/arbor.sock
    abstract: true  # Linux only

  grpc:
    address: authz.example.com:8080
    tls:
      enabled: true
      ca_cert: /path/to/ca.pem

  timeouts:
    connect: 5s
    request: 30s

  retry:
    attempts: 3
    backoff: exponential
```

## Testing

### Mock Client (for unit tests)

```javascript
// Node.js example
const { MockArborClient } = require('@arbor/client/mock');

const client = new MockArborClient();

// Set up expectations
client.expectCheck({
  principalId: userId,
  actionId: readActionId,
  resourceId: documentId,
}).returns({ decision: 'permit' });

// Run test
const result = await client.check(userId, readActionId, documentId);
expect(result.decision).toBe('permit');
```

### Integration Tests

```javascript
describe('Arbor Client Integration', () => {
  let client;

  beforeAll(async () => {
    // Start test authorizer
    await startTestAuthorizer();
    client = new ArborClient({ mode: 'sidecar' });
  });

  it('should check authorization', async () => {
    const result = await client.check(userId, actionId, resourceId);
    expect(result.decision).toBeDefined();
  });

  afterAll(async () => {
    await client.close();
    await stopTestAuthorizer();
  });
});
```

## Migration Guide

### From Cedar

```javascript
// Cedar
const decision = await cedar.isAuthorized({
  principal: { type: 'User', id: userId },
  action: { type: 'Action', id: 'read' },
  resource: { type: 'Document', id: documentId },
});

// Arbor
const result = await arbor.check(userId, readActionId, documentId);
const decision = result.decision === 'permit';
```

### From OpenFGA

```javascript
// OpenFGA
const { allowed } = await fga.check({
  user: `user:${userId}`,
  relation: 'viewer',
  object: `document:${documentId}`,
});

// Arbor
const result = await arbor.check(userId, viewActionId, documentId);
const allowed = result.decision === 'permit';
```

## Best Practices

### Connection Management

**✅ DO**: Reuse client instances

```javascript
// Create once
const client = new ArborClient();

// Reuse for all requests
app.use(async (req, res) => {
  const result = await client.check(...);
});
```

**❌ DON'T**: Create new client per request

```javascript
// BAD: Creates new connection every time
app.use(async (req, res) => {
  const client = new ArborClient(); // ❌
  const result = await client.check(...);
});
```

### Error Handling

```javascript
try {
  const result = await client.check(principalId, actionId, resourceId);

  if (result.decision === 'permit') {
    // Allow access
  } else {
    // Deny access
    log.info('Access denied', { reason: result.reason });
  }
} catch (err) {
  if (err.code === 'UNAVAILABLE') {
    // Authorizer is down, fail closed (deny access)
    log.error('Authorizer unavailable, denying access');
    return res.status(503).send('Service unavailable');
  }
  throw err;
}
```

### Performance Monitoring

```javascript
const start = Date.now();
const result = await client.check(...);
const duration = Date.now() - start;

metrics.histogram('arbor.check.duration_ms', duration);
metrics.counter(`arbor.check.${result.decision}`, 1);

if (duration > 100) {
  log.warn('Slow authorization check', { duration, mode: client.mode });
}
```

## Related Documentation

- [Architecture](./architecture.md) - Overall system design
- [Data Model](./data-model.md) - Entity, Policy, Action types
- [Authorization Flow](./authorization-flow.md) - How checks work
- [Implementation Roadmap](./implementation-roadmap.md) - Client library tasks
