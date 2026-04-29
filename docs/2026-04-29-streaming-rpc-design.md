# Streaming RPC for tarpc-fory

**Date:** 2026-04-29
**Author:** Justin Holmes
**Status:** Spec — approved in conversation
**Repo:** justinholmes/tarpc (fork), branch `feat/fory-codec`

## Summary

Add server streaming, client streaming, and bidirectional streaming RPC to tarpc via the `#[streaming(...)]` attribute on service trait methods. Extends the wire protocol with stream lifecycle messages (`StreamOpen`, `StreamData`, `StreamEnd`, `StreamError`) multiplexed alongside unary calls on the same connection. Integrates with the existing zero-copy fory codec for bulk-payload streaming.

## Motivation

tarpc currently supports unary RPC only. For tarpc-fory to compete with tonic/gRPC as a general-purpose framework, streaming is the biggest missing feature. Use cases:

- **Server streaming**: paginated list results, event feeds, log tailing
- **Client streaming**: bulk uploads, sensor data ingest, WAL replication
- **Bidirectional**: shard replication with per-shard acks, chat, collaborative editing

Without streaming, tarpc-fory users must either use a separate framework for streaming workloads or implement application-level streaming over unary (fire N separate RPCs, correlate by convention).

## Goals

1. Three streaming modes via proc-macro attributes: `#[streaming(server)]`, `#[streaming(client)]`, `#[streaming(bidirectional)]`
2. Stream multiplexing: multiple concurrent streams + unary calls on one connection via `stream_id`
3. Flow control: TCP backpressure default + per-stream `max_buffered_items` (default 64)
4. Error handling: immediate abort — server sends `StreamError`, client's Stream yields Err then None
5. Zero-copy integration: `StreamData` frames use the `[envelope][body][body_len]` wire format
6. Backward compat: connections using only unary see no change; `ServerMessage::Response` wraps existing `Response<T>`

## Non-goals

- HTTP/2-style application-level flow control (WINDOW_UPDATE credits). TCP + buffer cap is sufficient.
- Cross-language streaming interop testing (Rust-only for v1).
- Streaming over the classic tokio-serde codec path. Streaming is zero-copy codec only.
- Replacing tarpc's existing unary path. Both coexist.

## Wire protocol

### Extended `ClientMessage<T>`

```rust
#[non_exhaustive]
pub enum ClientMessage<T> {
    // Existing
    Request(Request<T>),
    Cancel { trace_context: trace::Context, request_id: u64 },

    // New: streaming
    StreamOpen {
        stream_id: u64,
        context: context::Context,
        message: T,
    },
    StreamData {
        stream_id: u64,
        message: T,
    },
    StreamEnd {
        stream_id: u64,
    },
}
```

### New `ServerMessage<T>` (replaces `Response<T>` as transport output on client side)

```rust
#[non_exhaustive]
pub enum ServerMessage<T> {
    // Existing unary (wraps Response<T>)
    Response(Response<T>),

    // New: streaming
    StreamData {
        stream_id: u64,
        message: T,
    },
    StreamEnd {
        stream_id: u64,
    },
    StreamError {
        stream_id: u64,
        error: ServerError,
    },
}
```

`stream_id` is client-allocated, unique per connection (same address space as `request_id`). Unary `Request.id` and streaming `stream_id` share the same monotonic counter to avoid collisions.

### Frame layout (zero-copy codec)

Each message uses the existing frame structure:

```
[ fory-encoded envelope ][ body bytes ][ body_len: u32 LE ]
```

`StreamData` frames carrying bulk payloads put the bulk into the body region (zero-copy on receive). Small streaming messages (metadata, acks) use body_len = 0.

## Service trait syntax

```rust
#[tarpc::service]
trait Storage {
    // Unary (existing, unchanged)
    async fn put_shard(data: Vec<u8>) -> bool;

    // Server streaming: server returns a stream of responses
    #[streaming(server)]
    async fn list_objects(prefix: String) -> ObjectEntry;

    // Client streaming: client sends a stream, server returns one response
    #[streaming(client)]
    async fn upload_chunks(chunk: Vec<u8>) -> UploadResult;

    // Bidirectional: both sides stream
    #[streaming(bidirectional)]
    async fn replicate(shard: ShardData) -> ReplicateAck;
}
```

### Generated types

For `#[streaming(server)] async fn list_objects(prefix: String) -> ObjectEntry`:

**Client stub:**
```rust
impl StorageClient {
    pub async fn list_objects(
        &self,
        ctx: context::Context,
        prefix: String,
    ) -> Result<impl Stream<Item = Result<ObjectEntry, StreamError>>, RpcError> { ... }
}
```

**Server handler trait:**
```rust
trait Storage {
    fn list_objects(
        self,
        ctx: context::Context,
        prefix: String,
    ) -> impl Stream<Item = ObjectEntry> + Send;
}
```

For `#[streaming(client)] async fn upload_chunks(chunk: Vec<u8>) -> UploadResult`:

**Client stub:**
```rust
impl StorageClient {
    pub async fn upload_chunks(
        &self,
        ctx: context::Context,
        chunks: impl Stream<Item = Vec<u8>> + Send + 'static,
    ) -> Result<UploadResult, RpcError> { ... }
}
```

**Server handler trait:**
```rust
trait Storage {
    async fn upload_chunks(
        self,
        ctx: context::Context,
        chunks: impl Stream<Item = Vec<u8>>,
    ) -> UploadResult;
}
```

For `#[streaming(bidirectional)] async fn replicate(shard: ShardData) -> ReplicateAck`:

**Client stub:**
```rust
impl StorageClient {
    pub async fn replicate(
        &self,
        ctx: context::Context,
        shards: impl Stream<Item = ShardData> + Send + 'static,
    ) -> Result<impl Stream<Item = Result<ReplicateAck, StreamError>>, RpcError> { ... }
}
```

**Server handler trait:**
```rust
trait Storage {
    fn replicate(
        self,
        ctx: context::Context,
        shards: impl Stream<Item = ShardData>,
    ) -> impl Stream<Item = ReplicateAck> + Send;
}
```

## Stream lifecycle

### Server streaming

```
Client                              Server
  │                                   │
  ├──StreamOpen{id, ctx, prefix}─────>│  handler called with prefix
  │                                   │  handler returns Stream<ObjectEntry>
  │<────────StreamData{id, entry1}────┤  stream yields entry1
  │<────────StreamData{id, entry2}────┤  stream yields entry2
  │<────────StreamEnd{id}─────────────┤  stream exhausted
```

### Client streaming

```
Client                              Server
  │                                   │
  ├──StreamOpen{id, ctx, chunk1}─────>│  handler called with Stream<Vec<u8>>
  ├──StreamData{id, chunk2}──────────>│  handler's stream yields chunk2
  ├──StreamData{id, chunk3}──────────>│  handler's stream yields chunk3
  ├──StreamEnd{id}───────────────────>│  handler's stream yields None
  │                                   │  handler returns UploadResult
  │<────────Response{id, result}──────┤  unary Response (not StreamEnd)
```

### Bidirectional

```
Client                              Server
  │                                   │
  ├──StreamOpen{id, ctx, shard1}─────>│  handler called with (Stream<in>, Sink<out>)
  ├──StreamData{id, shard2}──────────>│
  │<────────StreamData{id, ack1}──────┤  handler yields ack per shard
  ├──StreamData{id, shard3}──────────>│
  │<────────StreamData{id, ack2}──────┤
  ├──StreamEnd{id}───────────────────>│  client done sending
  │<────────StreamData{id, ack3}──────┤  server finishes processing
  │<────────StreamEnd{id}─────────────┤  server done
```

### Error (any mode)

```
  │<────────StreamError{id, err}──────┤  at any point: handler panics or returns Err
  │  client Stream yields Err then None
  │  both sides drop stream state for this id
```

## Flow control

**Default: TCP backpressure.** If receiver is slow, kernel socket buffer fills, sender's `poll_write` returns `Pending`.

**Per-stream buffer cap:** `max_buffered_items: usize` (default 64). On the sender side, the Sink returns `Poll::Pending` when the outbound buffer for a stream_id exceeds this count. On the receiver side, the internal mpsc channel for a stream_id is bounded at this count — backpressure propagates to the transport reader.

Configurable per method:

```rust
#[streaming(server, buffer = 128)]
async fn list_objects(prefix: String) -> ObjectEntry;
```

## Fory envelope wrappers

New wrapper types (manual Serializer impls, same pattern as existing ForyClientMessage/ForyResponse):

```rust
pub enum ForyClientMessage<T: ...> {
    Request(ForyRequest<T>),
    Cancel { trace_context: ForyTraceContext, request_id: u64 },
    StreamOpen { stream_id: u64, trace: ForyTraceContext, deadline_ns: u64, message: T },
    StreamData { stream_id: u64, message: T },
    StreamEnd { stream_id: u64 },
}

pub enum ForyServerMessage<T: ...> {
    Response(ForyResponse<T>),
    StreamData { stream_id: u64, message: T },
    StreamEnd { stream_id: u64 },
    StreamError { stream_id: u64, error: ForyServerError },
}
```

`ForyClientMessage` is extended (already an enum). `ForyServerMessage` is new — replaces `ForyResponse` as the server-to-client wire type.

Registered via `ServiceWireSchema::register` alongside existing types.

## Architecture changes

### Client (`client.rs`)

- New `StreamDispatcher` alongside existing `InFlightRequests`
- Tracks active streams by `stream_id` → `mpsc::Sender<ServerMessage<T>>`
- When transport yields `ServerMessage::StreamData/End/Error`, routes to the right channel
- Client stub for streaming methods: allocates stream_id, sends `StreamOpen`, returns a `ReceiverStream` wrapping the mpsc

### Server (`server.rs`)

- `BaseChannel::execute` extended: on `ClientMessage::StreamOpen`, spawns a stream handler task
- Stream handler receives a `ReceiverStream<T>` (fed by subsequent `StreamData` messages) and sends results via `Sink<ServerMessage<T>>`
- On `StreamEnd`, the handler's input stream yields `None`
- On handler panic/drop, sends `StreamError` automatically

### Proc-macro (`plugins/src/lib.rs`)

- Parse `#[streaming(server|client|bidirectional)]` attribute on trait methods
- For each streaming method, generate different stub signatures (as shown above)
- Generate server dispatch arms for streaming methods alongside unary

### Zero-copy codec (`fory_zerocopy.rs`)

- `ServerZeroCopyCodec` Decoder produces `(ClientMessage<T>, Option<Bytes>)` — already handles all ClientMessage variants; new variants added
- `ClientZeroCopyCodec` Decoder produces `(ServerMessage<T>, Option<Bytes>)` — new type replacing `(Response<T>, ...)`
- `ZeroCopySink` `ToFrame` trait: implement for `(ServerMessage<T>, Option<Bytes>)`

## Files touched

| File | Change | LOC est |
|---|---|---|
| `tarpc/src/lib.rs` | Extend `ClientMessage<T>`, add `ServerMessage<T>` | +80 |
| `tarpc/src/client.rs` | `StreamDispatcher`, stream call dispatch, response routing | +400 |
| `tarpc/src/server.rs` | BaseChannel stream handler spawning, StreamEnd/Error lifecycle | +350 |
| `tarpc/src/server/in_flight_requests.rs` | Track active streams | +100 |
| `plugins/src/lib.rs` | `#[streaming]` attr parsing, stream stub generation | +400 |
| `tarpc/src/serde_transport/fory_envelope.rs` | Extend ForyClientMessage, add ForyServerMessage, Serializer impls | +200 |
| `tarpc/src/serde_transport/fory.rs` | ForyEnvelopeCodec handles new variants | +100 |
| `tarpc/src/serde_transport/fory_zerocopy.rs` | ZeroCopy codec handles ServerMessage, new variants | +100 |
| `tarpc/tests/` | Streaming e2e tests (server, client, bidirectional, error, concurrent) | +500 |
| **Total** | | **~2230** |

## Implementation phases

| Phase | Scope | Est |
|---|---|---|
| S1 | Wire types (`ClientMessage` extension, `ServerMessage<T>`, fory wrappers) | 2 days |
| S2 | Server dispatch (BaseChannel stream handler, StreamEnd/Error lifecycle) | 3 days |
| S3 | Client stubs (StreamDispatcher, stream call dispatch, ReceiverStream) | 2 days |
| S4 | Proc-macro (`#[streaming]` attribute, stub generation for all 3 modes) | 3 days |
| S5 | Zero-copy codec integration (new variants in Decoder/Encoder) | 2 days |
| S6 | Tests + bench (server/client/bidi e2e, error paths, concurrent streams, throughput) | 2 days |
| **Total** | | **~14 days** |

## Risks

| # | Risk | Mitigation |
|---|---|---|
| R1 | `ServerMessage<T>` replacing `Response<T>` as transport output breaks existing classic-codec users | Gate behind `streaming` feature flag; classic codec stays on `Response<T>` |
| R2 | Stream handler panic leaves dangling stream_id in client's dispatcher | Server's `BaseChannel` wraps handler in `catch_unwind`-equivalent (tokio JoinHandle), sends `StreamError` on task exit |
| R3 | `max_buffered_items` too small → perf regression from excessive backpressure | Default 64 is generous for most use cases; per-method override available |
| R4 | Proc-macro complexity doubles (unary + 3 streaming modes) | Factor stub generation into helpers by mode; share common patterns |
| R5 | `stream_id` / `request_id` collision if both use the same counter | Unified counter per connection — already the plan |

## Testing plan

| Test | What it proves |
|---|---|
| `server_stream_list_objects` | Server yields N items, client receives all N in order |
| `client_stream_upload_chunks` | Client sends N chunks, server receives all, returns result |
| `bidi_stream_replicate` | Both sides stream concurrently, items interleave correctly |
| `stream_error_mid_flight` | Server returns Err after 3 items, client sees 3 items then Err |
| `stream_cancel` | Client drops stream, server's handler gets cancelled |
| `concurrent_streams` | 10 concurrent streams + 10 unary calls on one connection |
| `stream_backpressure` | Slow consumer causes sender to block at max_buffered_items |
| `stream_4mib_zerocopy` | StreamData with 4 MiB body, verify zero-copy aliasing |
| `stream_empty` | Server stream yields 0 items, client gets StreamEnd immediately |
| `stream_single_item` | Edge case: stream with exactly 1 item |

## Open questions

- Should `StreamOpen` carry the method discriminant (which method is being streamed), or is that inferred from the `T` type? Recommendation: carry a `method_id: u32` (hash of method name) so the server can dispatch without deserializing the full message first.
- Should cancellation of a stream send `Cancel` (existing) or a new `StreamCancel { stream_id }`? Recommendation: reuse `Cancel` with `request_id = stream_id` — same semantics, no new variant.
