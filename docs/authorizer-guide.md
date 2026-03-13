# Arbor Authorizer Guide

The `arbor-authorizer` is a high-performance gRPC service designed for fast authorization decisions. It can be deployed in various configurations to optimize for latency and scale.

## Deployment Patterns

### 1. Node-local Agent (DaemonSet) — *Recommended*
One authorizer runs on each Kubernetes node, serving all Pods on that node.

- **Transport:** UDS via a `hostPath` volume.
- **Setup:**
    - Mount a host directory (e.g., `/var/run/arbor`) into the authorizer container.
    - Mount the same host directory into your application containers.
- **Benefits:** Best balance of performance and memory efficiency. Near-zero latency with minimal memory footprint compared to the sidecar pattern.

### 2. Sidecar Pattern
In this pattern, the `arbor-authorizer` runs as a container alongside your application in the same Pod.

- **Transport:** Use Unix Domain Sockets (UDS) for the lowest possible latency.
- **Connection Management:** 
    - Your application should open **one** persistent gRPC connection to the UDS socket at startup.
    - Reuse this single connection (and the generated gRPC client) for all requests.
    - gRPC (via HTTP/2) will multiplex all concurrent requests over this single connection.
- **Use Case:** High-traffic services requiring dedicated resource isolation (CPU/Memory).

### 3. Stand-alone Server
The authorizer runs as a separate, reachable service either on a dedicated machine or as a centralized cluster service.

- **Transport:** Standard TCP.
- **Connection Management:** Use a standard gRPC connection pool if necessary, though a single persistent connection is still recommended for many use cases.
- **Use Case:** Non-Kubernetes environments, cross-cluster authorization, or legacy infrastructure where shared volumes or sidecars are not feasible.
- **Consideration:** Higher latency due to network round-trips compared to UDS.

## Performance Tuning

### Concurrency Limits
The authorizer defaults to a high concurrency limit to handle dense workloads.

- **Default:** 1000 concurrent streams per connection.
- **Configuration:** You can adjust this via the `max_concurrent_streams` setting in `config/authorizer.toml` or the `ARBOR_MAX_CONCURRENT_STREAMS` environment variable.

### Connection Pooling vs. Multiplexing
For most use cases, **connection pooling is unnecessary and discouraged**. 

- gRPC's built-in multiplexing is more efficient than managing a pool of sockets.
- If you reach the `max_concurrent_streams` limit, first try increasing the limit on the server.
- Only consider a small connection pool (e.g., 2-4 connections) if you observe significant contention on the HTTP/2 state machine or if you need to exceed ~2,500 concurrent requests from a single client.
- **Scaling Recommendation:** If a single client Pod consistently requires more than 2,500 concurrent requests, it is a strong candidate for its own dedicated **Sidecar** rather than using the shared Node-local Agent. This avoids resource contention with other Pods on the same node and provides the client with its own dedicated CPU/Memory for authorization processing.

## Configuration Reference

| Environment Variable | Description | Default |
| :--- | :--- | :--- |
| `ARBOR_TRANSPORT` | `uds`, `tcp`, or `both` | `both` |
| `ARBOR_UDS_PATH` | Path to the Unix socket | `/tmp/arbor.sock` |
| `ARBOR_GRPC_ADDR` | TCP address for gRPC | `[::1]:50051` |
| `ARBOR_MAX_CONCURRENT_STREAMS` | Max concurrent gRPC streams | `1000` |
| `ARBOR_SNAPSHOT_PATH` | Path to the snapshot file | (Required) |

## Example Client (Rust/Tonic)

To connect to the authorizer over UDS:

```rust
use tonic::transport::{Endpoint, Uri};
use tokio::net::UnixStream;
use tower::service_fn;

let channel = Endpoint::try_from("http://localhost")?
    .connect_with_connector(service_fn(|_| async {
        UnixStream::connect("/tmp/arbor.sock").await
    }))
    .await?;

let mut client = ArborClient::new(channel);
```
