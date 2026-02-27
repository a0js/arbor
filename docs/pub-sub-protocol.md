# Pub/Sub Protocol

This document describes the protocol for distributing snapshot availability notifications from the indexer to authorizers.

## Overview

Arbor uses a **pub/sub model** for notifying authorizers about new snapshots:

```
Indexer (Publisher)
    │
    │ Publishes to topic: "arbor.snapshots"
    ▼
Pub/Sub Broker (NATS, Kafka, Redis Streams, etc.)
    │
    │ Subscribers
    ▼
Authorizers (many instances)
    │
    │ Fetch full snapshot via HTTP/gRPC
    ▼
Indexer (HTTP/gRPC server)
```

**V1 Simplification**: No deltas - authorizers download full snapshots when updates are available.

**Key Requirements**:
- **At-least-once delivery**: Authorizers must receive availability notifications
- **Message buffering**: Broker buffers messages if authorizer temporarily offline
- **Checksum-based deduplication**: Authorizers skip download if checksum unchanged
- **Broker-agnostic**: Pluggable connectors for different message brokers

## Broker Abstraction

### Trait Interface

```rust
#[async_trait]
pub trait MessageBroker: Send + Sync {
    /// Connect to the broker
    async fn connect(&mut self) -> Result<()>;

    /// Publish a message to a topic
    async fn publish(&self, topic: &str, message: &[u8]) -> Result<()>;

    /// Subscribe to a topic
    async fn subscribe(&self, topic: &str) -> Result<Box<dyn MessageStream>>;

    /// Close connection
    async fn close(&mut self) -> Result<()>;
}

#[async_trait]
pub trait MessageStream: Send {
    /// Get next message
    async fn next(&mut self) -> Option<Result<Message>>;
}

pub struct Message {
    pub payload: Vec<u8>,
    pub timestamp: i64,
}
```

### Broker Implementations (V1)

**NATS** (Reference implementation):
```rust
pub struct NatsMessageBroker {
    client: async_nats::Client,
    url: String,
}
```

**Future implementations**:
- Kafka (V2)
- Redis Streams (V2)
- RabbitMQ (V2+)
- AWS SQS/SNS (V2+)
- Google Pub/Sub (V2+)

## Message Format

### Snapshot Available Message

The only message type in V1:

```rust
pub struct SnapshotAvailableMessage {
    /// New snapshot version
    pub version: u64,

    /// Timestamp when snapshot was created
    pub created_at: i64,

    /// Indexer ID that created this snapshot
    pub indexer_id: String,

    /// SHA256 checksum of the snapshot
    pub checksum: [u8; 32],

    /// Snapshot size in bytes
    pub size_bytes: u64,

    /// Metadata
    pub metadata: SnapshotMetadata,
}

pub struct SnapshotMetadata {
    pub entity_count: u32,
    pub policy_count: u32,
    pub action_count: u32,
    pub generation_duration_ms: u64,
}
```

**Published to**: `arbor.snapshots`

**Frequency**: When snapshot is generated (based on batching strategy)

**Serialization**: MessagePack (compact, fast, schema-less)

## Message Flow

### Normal Update Flow

```
1. Source data changes
   │
2. Indexer batches changes (TBD: time/count/debounce)
   │
3. Indexer generates snapshot v101
   │
4. Indexer publishes SnapshotAvailableMessage to broker
   │
   Topic: "arbor.snapshots"
   Payload: { version: 101, checksum: [ABC...], size: 10485760, ... }
   │
5. Broker delivers to all subscribed authorizers
   │
6. Each authorizer:
   ├─ Receives message
   ├─ Compares checksum with current snapshot
   ├─ If different:
   │   ├─ Fetches full snapshot from indexer (HTTP/gRPC)
   │   ├─ Verifies checksum
   │   └─ Atomically swaps snapshot
   └─ If same: Skips download (no changes)
```

### Authorizer Startup Flow

```
1. Authorizer starts up (no snapshot)
   │
2. Authorizer connects to broker
   │
3. Authorizer subscribes to "arbor.snapshots"
   │
4. Authorizer fetches latest snapshot from indexer (HTTP/gRPC)
   │
   Request: GET /snapshots/latest
   Response: Snapshot v100 + checksum
   │
5. Authorizer loads snapshot into memory
   │
6. Authorizer begins receiving update notifications
   │
7. Authorizer processes updates as they arrive
```

### Indexer Restart Flow

```
1. Indexer restarts (authorizers keep running)
   │
2. Indexer rebuilds snapshot from source data
   │
3. Indexer publishes SnapshotAvailableMessage
   │
4. Authorizers receive notification
   │
5. Authorizers compare checksums
   │
6. If different: Download new snapshot
   │
   If same: Skip (data unchanged)
```

## Broker-Specific Configuration

### NATS Configuration

```toml
[broker]
type = "nats"

[broker.nats]
url = "nats://localhost:4222"
# Cluster: "nats://nats1:4222,nats2:4222,nats3:4222"

# Optional authentication
username = "arbor"
password = "${NATS_PASSWORD}"

# Optional TLS
tls = true
ca_cert = "/path/to/ca.pem"

# Connection options
max_reconnects = 10
reconnect_wait = "5s"
```

### Kafka Configuration (V2+)

```toml
[broker]
type = "kafka"

[broker.kafka]
brokers = ["localhost:9092"]
topic = "arbor.snapshots"

# Consumer group for authorizers
group_id = "arbor-authorizers"

# Optional authentication
sasl_mechanism = "PLAIN"
sasl_username = "arbor"
sasl_password = "${KAFKA_PASSWORD}"

# Optional TLS
tls = true
```

### Redis Streams Configuration (V2+)

```toml
[broker]
type = "redis"

[broker.redis]
url = "redis://localhost:6379"
stream = "arbor:snapshots"
consumer_group = "arbor-authorizers"

# Optional authentication
password = "${REDIS_PASSWORD}"

# Optional TLS
tls = true
```

## Indexer Implementation

### Publishing Snapshots

```rust
pub struct Indexer {
    broker: Box<dyn MessageBroker>,
    http_server: HttpServer,
    current_snapshot: Arc<RwLock<Option<Snapshot>>>,
}

impl Indexer {
    pub async fn publish_snapshot(&self, snapshot: Snapshot) -> Result<()> {
        // Store snapshot (for HTTP fetch)
        *self.current_snapshot.write().await = Some(snapshot.clone());

        // Create availability message
        let msg = SnapshotAvailableMessage {
            version: snapshot.version,
            created_at: snapshot.created_at,
            indexer_id: self.id.clone(),
            checksum: snapshot.checksum,
            size_bytes: Self::estimate_size(&snapshot),
            metadata: SnapshotMetadata {
                entity_count: snapshot.data.indexed_entities.len() as u32,
                policy_count: snapshot.data.indexed_policies.len() as u32,
                action_count: snapshot.data.actions.len() as u32,
                generation_duration_ms: snapshot.metadata.generation_duration_ms,
            },
        };

        // Serialize with MessagePack
        let payload = rmp_serde::to_vec(&msg)?;

        // Publish to broker
        self.broker.publish("arbor.snapshots", &payload).await?;

        log::info!(
            "Published snapshot v{} ({} MB)",
            snapshot.version,
            msg.size_bytes / 1_000_000
        );

        Ok(())
    }
}
```

### Serving Snapshots (HTTP)

```rust
// HTTP endpoint for fetching snapshots
async fn get_latest_snapshot(
    State(indexer): State<Arc<Indexer>>
) -> Result<Response, StatusCode> {
    let snapshot = indexer.current_snapshot.read().await;

    match snapshot.as_ref() {
        Some(snap) => {
            // Serialize and compress
            let bytes = bincode::serialize(snap)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

            let compressed = compress_gzip(&bytes);

            Ok(Response::builder()
                .header("Content-Type", "application/octet-stream")
                .header("Content-Encoding", "gzip")
                .header("X-Snapshot-Version", snap.version)
                .header("X-Snapshot-Checksum", hex::encode(snap.checksum))
                .body(compressed.into())
                .unwrap())
        }
        None => Err(StatusCode::NOT_FOUND),
    }
}

// Alternative: gRPC endpoint
async fn get_snapshot(
    request: Request<GetSnapshotRequest>
) -> Result<Response<GetSnapshotResponse>, Status> {
    // Similar implementation
}
```

## Authorizer Implementation

### Subscribing to Updates

```rust
pub struct Authorizer {
    broker: Box<dyn MessageBroker>,
    indexer_url: String,
    snapshot: Arc<RwLock<Option<Snapshot>>>,
    current_checksum: Arc<RwLock<[u8; 32]>>,
}

impl Authorizer {
    pub async fn watch_for_updates(&self) -> Result<()> {
        let mut stream = self.broker.subscribe("arbor.snapshots").await?;

        while let Some(result) = stream.next().await {
            let msg = result?;

            // Deserialize
            let update: SnapshotAvailableMessage =
                rmp_serde::from_slice(&msg.payload)?;

            // Handle update
            self.handle_snapshot_available(update).await?;
        }

        Ok(())
    }

    async fn handle_snapshot_available(
        &self,
        msg: SnapshotAvailableMessage
    ) -> Result<()> {
        let current_checksum = *self.current_checksum.read().await;

        // Check if snapshot changed
        if msg.checksum == current_checksum {
            log::debug!(
                "Snapshot v{} unchanged (checksum match), skipping download",
                msg.version
            );
            return Ok(());
        }

        log::info!(
            "New snapshot v{} available ({} MB), downloading...",
            msg.version,
            msg.size_bytes / 1_000_000
        );

        // Fetch full snapshot from indexer
        let snapshot = self.fetch_snapshot_http().await?;

        // Verify checksum
        let computed_checksum = compute_checksum(&snapshot);
        if computed_checksum != msg.checksum {
            return Err(Error::ChecksumMismatch {
                expected: msg.checksum,
                computed: computed_checksum,
            });
        }

        // Atomic swap
        *self.snapshot.write().await = Some(snapshot);
        *self.current_checksum.write().await = msg.checksum;

        log::info!("Updated to snapshot v{}", msg.version);

        Ok(())
    }

    async fn fetch_snapshot_http(&self) -> Result<Snapshot> {
        let response = reqwest::get(&format!("{}/snapshots/latest", self.indexer_url))
            .await?;

        if !response.status().is_success() {
            return Err(Error::SnapshotFetchFailed);
        }

        let bytes = response.bytes().await?;
        let decompressed = decompress_gzip(&bytes)?;
        let snapshot: Snapshot = bincode::deserialize(&decompressed)?;

        Ok(snapshot)
    }
}
```

## Serialization

### Message Serialization: MessagePack

**Why MessagePack**:
- ✅ Compact (smaller than JSON)
- ✅ Fast (faster than JSON)
- ✅ Schema-less (easier evolution)
- ✅ Wide language support

```rust
use rmp_serde;

// Serialize
let msg = SnapshotAvailableMessage { ... };
let bytes = rmp_serde::to_vec(&msg)?;

// Deserialize
let msg: SnapshotAvailableMessage = rmp_serde::from_slice(&bytes)?;
```

### Snapshot Serialization: Bincode + Gzip

**Why Bincode**:
- ✅ Fast (native Rust)
- ✅ Compact
- ✅ Zero-copy deserialization

**Why Gzip**:
- ✅ Good compression (5-10x)
- ✅ Fast enough
- ✅ Standard (widely supported)

```rust
use bincode;
use flate2::write::GzEncoder;

// Serialize + compress
let bytes = bincode::serialize(&snapshot)?;
let compressed = compress_gzip(&bytes);

// Decompress + deserialize
let bytes = decompress_gzip(&compressed)?;
let snapshot: Snapshot = bincode::deserialize(&bytes)?;
```

## Error Handling

### Broker Connection Failures

```rust
// Retry with exponential backoff
let mut retry = ExponentialBackoff::new(
    Duration::from_millis(100),
    Duration::from_secs(30),
    2.0
);

loop {
    match broker.connect().await {
        Ok(()) => break,
        Err(e) => {
            log::warn!("Broker connection failed: {}", e);
            tokio::time::sleep(retry.next()).await;
        }
    }
}
```

### Message Delivery Failures

```rust
// Retry publishing with timeout
retry_with_timeout(
    Duration::from_secs(30),
    3,  // max attempts
    || broker.publish("arbor.snapshots", &msg)
).await?;
```

### Snapshot Fetch Failures

```rust
// Retry snapshot download
for attempt in 1..=3 {
    match self.fetch_snapshot_http().await {
        Ok(snapshot) => return Ok(snapshot),
        Err(e) => {
            log::warn!("Snapshot fetch attempt {} failed: {}", attempt, e);
            if attempt < 3 {
                tokio::time::sleep(Duration::from_secs(5 * attempt)).await;
            }
        }
    }
}

Err(Error::SnapshotFetchExhausted)
```

## Monitoring

### Metrics

```rust
// Indexer
metrics::counter!("arbor.indexer.snapshots_published", 1);
metrics::histogram!("arbor.indexer.snapshot_size_bytes", size);
metrics::gauge!("arbor.indexer.current_snapshot_version", version as f64);

// Authorizer
metrics::counter!("arbor.authorizer.snapshots_downloaded", 1);
metrics::histogram!("arbor.authorizer.snapshot_download_duration_ms", duration);
metrics::gauge!("arbor.authorizer.current_snapshot_version", version as f64);
metrics::gauge!("arbor.authorizer.snapshot_lag", version_lag as f64);
```

### Alerts

```yaml
# Authorizer falling behind
- alert: AuthorizerSnapshotLag
  expr: arbor_indexer_current_snapshot_version - arbor_authorizer_current_snapshot_version > 5
  for: 5m
  annotations:
    summary: "Authorizer is >5 versions behind indexer"

# Broker connection issues
- alert: BrokerConnectionDown
  expr: rate(arbor_broker_connection_errors[5m]) > 0.1
  for: 5m
  annotations:
    summary: "Frequent broker connection errors"
```

## Broker Comparison

| Feature | NATS | Kafka | Redis Streams |
|---------|------|-------|---------------|
| **V1 Status** | ✅ Recommended | 🔮 V2+ | 🔮 V2+ |
| **Lightweight** | ✅ Yes (~20MB) | ❌ No (JVM) | ✅ Yes |
| **Simple deployment** | ✅ Single binary | ❌ Complex | ✅ Simple |
| **Throughput** | ✅ High (millions/sec) | ✅ Very high | ✅ High |
| **Message persistence** | ⚠️ JetStream only | ✅ Yes | ✅ Yes |
| **Replay capability** | ⚠️ JetStream only | ✅ Yes | ✅ Yes |
| **Operational maturity** | ✅ Mature | ✅ Very mature | ✅ Mature |

**V1 Recommendation**: Start with **NATS** for simplicity, add other brokers as connectors in V2+.

## Related Documentation

- [Architecture](./architecture.md) - Overall system design
- [Snapshot Format](./snapshot-format.md) - Snapshot structure
- [Implementation Roadmap](./implementation-roadmap.md) - Broker connector tasks
