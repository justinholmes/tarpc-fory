# Streaming RPC for fory-rpc

**Date:** 2026-04-29
**Author:** Justin Holmes
**Status:** Spec — revised (no tarpc fork architecture)

## Summary

Add server streaming, client streaming, and bidirectional streaming RPC as an independent crate (`fory-rpc-streaming`) built on top of `tarpc-fory`'s zero-copy codec. No modifications to upstream `tarpc`. Streaming protocol has its own wire types, dispatch, and proc-macro (`#[fory_service]`).

Companion refactor (Phase S0): move all fory-specific code from `justinholmes/tarpc` fork into `tarpc-fory`, add a `tarpc-fory-macros` proc-macro crate, eliminate the tarpc fork entirely.

## Architecture — no tarpc fork

```
Repos:
├── tokio-serde-fory          — fory codec for tokio-serde (unchanged)
├── tarpc-fory                — transport + envelope types + zero-copy codec
│   └── tarpc-fory-macros     — proc-macro: #[fory_service] attribute
├── fory-rpc-streaming        — streaming protocol (new, independent)
└── (no tarpc fork)           — depend on upstream tarpc from crates.io
```

### How `#[fory_service]` replaces the tarpc fork proc-macro

Currently `tarpc-fork/plugins/src/lib.rs` patches `#[tarpc::service]` to emit ForyObject derives + `ServiceWireSchema`. This couples us to a fork.

New approach — two composable attributes:

```rust
#[tarpc::service]                    // upstream tarpc, unmodified
#[tarpc_fory::fory_service]          // tarpc-fory's own proc-macro
trait Hello {
    async fn hello(name: String) -> String;
    
    #[streaming(server)]
    async fn list_objects(prefix: String) -> ObjectEntry;
}
```

Execution order (outer-to-inner): `#[tarpc::service]` runs first, emits `HelloRequest`, `HelloResponse`, `HelloClient`, server trait. `#[tarpc_fory::fory_service]` runs second on the expanded output:

1. Finds `HelloRequest` / `HelloResponse` enums in the token stream
2. Emits manual `fory::Serializer` impls (EXT path — no TYPE_ID_COUNTER collision)
3. Emits `pub struct HelloService;` marker
4. Emits `impl ServiceWireSchema for HelloService { fn register(...) { ... } }`
5. Parses `#[streaming(...)]` attributes on methods, emits streaming stubs
6. Passes through everything else unchanged

~500 LOC proc-macro in `tarpc-fory-macros` (proc-macro crate), re-exported via `tarpc-fory`.

### What moves where

| Currently in tarpc-fork | Moves to |
|---|---|
| `tarpc/src/serde_transport/fory.rs` (codec, connect/listen, TLS) | `tarpc-fory/src/transport.rs` |
| `tarpc/src/serde_transport/fory_envelope.rs` (wrapper types, conversions) | `tarpc-fory/src/envelope.rs` |
| `tarpc/src/serde_transport/fory_zerocopy.rs` (zero-copy codec) | `tarpc-fory/src/zerocopy.rs` |
| `plugins/src/lib.rs` (ForyObject derive + ServiceWireSchema) | `tarpc-fory-macros/src/lib.rs` |
| `tarpc/tests/fory_*.rs` (all fory tests) | `tarpc-fory/tests/` |
| `tarpc/benches/fory_codec_compare.rs` | `tarpc-fory/benches/` |

tarpc-fork → deleted after migration. `tarpc-fory` depends on upstream `tarpc` from crates.io.

## Motivation

- tarpc currently supports unary RPC only. For tarpc-fory to be a general-purpose framework, streaming is the biggest gap vs tonic/gRPC.
- Use cases: paginated lists, event feeds, bulk uploads, sensor ingest, shard replication with per-shard acks.

## Goals

1. Three streaming modes: `#[streaming(server)]`, `#[streaming(client)]`, `#[streaming(bidirectional)]`
2. Stream multiplexing: concurrent streams + unary calls on one connection via `stream_id`
3. Flow control: TCP backpressure + per-stream `max_buffered_items` (default 64)
4. Error handling: immediate abort — `StreamError` → client's Stream yields Err then None
5. Zero-copy: `StreamData` frames use `[envelope][body][body_len]` wire format
6. No tarpc fork: depend on upstream tarpc, all fory code in our crates

## Non-goals

- HTTP/2-style WINDOW_UPDATE flow control
- Cross-language streaming interop testing (Rust-only for v1)
- Streaming over classic tokio-serde codec (zero-copy only)
- Modifying upstream tarpc source

## Wire protocol

### Stream messages (owned by fory-rpc-streaming, NOT tarpc)

```rust
// Client → Server (extends the zero-copy codec's message type)
pub enum StreamClientMessage<T> {
    StreamOpen { stream_id: u64, context: Context, message: T },
    StreamData { stream_id: u64, message: T },
    StreamEnd { stream_id: u64 },
}

// Server → Client
pub enum StreamServerMessage<T> {
    StreamData { stream_id: u64, message: T },
    StreamEnd { stream_id: u64 },
    StreamError { stream_id: u64, error: StreamError },
}
```

These are SEPARATE from tarpc's `ClientMessage<T>` / `Response<T>`. The zero-copy codec discriminates between unary (tarpc) and streaming (fory-rpc-streaming) at the frame level via a 1-byte message-class prefix:

```
Frame layout:
[ msg_class: u8 ][ fory-encoded envelope ][ body bytes ][ body_len: u32 LE ]

msg_class:
  0x00 = tarpc unary ClientMessage (existing)
  0x01 = tarpc unary Response (existing)
  0x10 = StreamClientMessage
  0x11 = StreamServerMessage
```

This lets unary and streaming coexist on the same connection without touching tarpc's types.

`stream_id` is client-allocated from the same monotonic counter as tarpc's `request_id` to avoid ID collisions.

## Service trait syntax

```rust
#[tarpc::service]
#[tarpc_fory::fory_service]
trait Storage {
    // Unary (handled by tarpc)
    async fn put_shard(data: Vec<u8>) -> bool;

    // Server streaming
    #[streaming(server)]
    async fn list_objects(prefix: String) -> ObjectEntry;

    // Client streaming
    #[streaming(client)]
    async fn upload_chunks(chunk: Vec<u8>) -> UploadResult;

    // Bidirectional streaming
    #[streaming(bidirectional)]
    async fn replicate(shard: ShardData) -> ReplicateAck;
}
```

### Generated types

**Server streaming** (`list_objects`):
- Client: `async fn list_objects(ctx, prefix) -> Result<impl Stream<Item = Result<ObjectEntry, StreamError>>>`
- Server: `fn list_objects(self, ctx, prefix) -> impl Stream<Item = ObjectEntry> + Send`

**Client streaming** (`upload_chunks`):
- Client: `async fn upload_chunks(ctx, chunks: impl Stream<Item = Vec<u8>>) -> Result<UploadResult>`
- Server: `async fn upload_chunks(self, ctx, chunks: impl Stream<Item = Vec<u8>>) -> UploadResult`

**Bidirectional** (`replicate`):
- Client: `async fn replicate(ctx, shards: impl Stream<Item = ShardData>) -> Result<impl Stream<Item = Result<ReplicateAck, StreamError>>>`
- Server: `fn replicate(self, ctx, shards: impl Stream<Item = ShardData>) -> impl Stream<Item = ReplicateAck> + Send`

## Stream lifecycle

### Server streaming

```
Client                              Server
  ├──StreamOpen{id, ctx, prefix}───>│  handler starts
  │<────────StreamData{id, entry1}──┤  handler yields
  │<────────StreamData{id, entry2}──┤
  │<────────StreamEnd{id}───────────┤  handler done
```

### Client streaming

```
Client                              Server
  ├──StreamOpen{id, ctx, chunk1}───>│  handler starts with Stream
  ├──StreamData{id, chunk2}────────>│
  ├──StreamEnd{id}─────────────────>│  handler's stream yields None
  │<────────Response{id, result}────┤  unary response
```

### Bidirectional

```
Client                              Server
  ├──StreamOpen{id, ctx, shard1}───>│  handler starts
  ├──StreamData{id, shard2}────────>│
  │<────────StreamData{id, ack1}────┤  concurrent output
  ├──StreamEnd{id}─────────────────>│
  │<────────StreamData{id, ack2}────┤
  │<────────StreamEnd{id}───────────┤
```

### Error (any mode)

```
  │<────────StreamError{id, err}────┤  handler panic or Err
  │  client Stream yields Err then None
  │  both sides drop stream state
```

## Flow control

TCP backpressure by default. Per-stream `max_buffered_items: usize` (default 64) — Sink returns `Poll::Pending` when buffer exceeds N. Configurable: `#[streaming(server, buffer = 128)]`.

## Fory envelope wrappers

Manual Serializer impls (EXT path) for `StreamClientMessage<T>` and `StreamServerMessage<T>`, same pattern as existing `ForyClientMessage<T>` / `ForyResponse<T>`. Registered via `ServiceWireSchema::register`.

## Crate structure

### `tarpc-fory-macros` (new proc-macro crate)

```
tarpc-fory-macros/
├── Cargo.toml
└── src/
    ├── lib.rs          — #[fory_service] attribute entry point
    ├── parse.rs        — parse tarpc-generated token stream, find Request/Response enums
    ├── fory_impls.rs   — emit Serializer + ForyDefault + ServiceWireSchema impls
    └── streaming.rs    — parse #[streaming] attrs, emit streaming stubs
```

### `fory-rpc-streaming` (new crate)

```
fory-rpc-streaming/
├── Cargo.toml
└── src/
    ├── lib.rs          — re-exports
    ├── protocol.rs     — StreamClientMessage, StreamServerMessage, StreamError
    ├── codec.rs        — zero-copy codec extension for stream messages
    ├── server.rs       — StreamDispatcher, handler spawning, StreamEnd/Error lifecycle
    ├── client.rs       — StreamClient, stream_id allocation, response routing
    └── sink.rs         — buffered Sink with max_buffered_items
```

## Implementation phases

| Phase | Scope | Repo | Est |
|---|---|---|---|
| **S0** | Refactor: move fory code from tarpc-fork → tarpc-fory, create tarpc-fory-macros, delete tarpc fork | tarpc-fory, tarpc-fory-macros | 3 days |
| S1 | Wire types (StreamClientMessage, StreamServerMessage, fory wrappers) | fory-rpc-streaming | 2 days |
| S2 | Server dispatch (StreamDispatcher, handler spawning, lifecycle) | fory-rpc-streaming | 3 days |
| S3 | Client stubs (StreamClient, stream_id alloc, response routing) | fory-rpc-streaming | 2 days |
| S4 | Proc-macro `#[streaming]` in tarpc-fory-macros | tarpc-fory-macros | 3 days |
| S5 | Zero-copy codec msg_class discriminator for stream + unary coexistence | tarpc-fory | 2 days |
| S6 | Tests + bench (all 3 modes, error, concurrent, backpressure, 4 MiB zerocopy) | all | 2 days |
| **Total** | | | **~17 days** |

## Risks

| # | Risk | Mitigation |
|---|---|---|
| R1 | `#[fory_service]` can't parse tarpc's expanded token stream reliably across tarpc versions | Pin tarpc version; test against multiple versions in CI; the generated enum names are stable (`TraitNameRequest`) |
| R2 | msg_class prefix byte breaks existing zero-copy codec wire compat | Phase S0 refactor is the right time — no existing deployments on the zero-copy path |
| R3 | Streaming dispatch complexity (per-stream mpsc channels + backpressure) | Start with server-streaming only (simplest), add client + bidi incrementally |
| R4 | proc-macro crate adds compilation overhead | Proc-macros are one-time per crate compile; acceptable |
| R5 | Two proc-macro attributes (`#[tarpc::service]` + `#[fory_service]`) — user forgets one | Compile error on missing types; document clearly |

## Testing plan

| Test | Crate | What it proves |
|---|---|---|
| `server_stream_list` | fory-rpc-streaming | Server yields N items, client receives all N |
| `client_stream_upload` | fory-rpc-streaming | Client sends N items, server receives, returns result |
| `bidi_stream_replicate` | fory-rpc-streaming | Both sides stream concurrently |
| `stream_error_mid_flight` | fory-rpc-streaming | Error after 3 items → client sees 3 + Err |
| `stream_cancel` | fory-rpc-streaming | Client drops → server handler cancelled |
| `concurrent_streams_and_unary` | fory-rpc-streaming | 10 streams + 10 unary calls on one connection |
| `stream_backpressure` | fory-rpc-streaming | Slow consumer → sender blocks at max_buffered_items |
| `stream_4mib_zerocopy` | fory-rpc-streaming | 4 MiB body per StreamData, verify aliasing |
| `fory_service_macro` | tarpc-fory | `#[fory_service]` generates correct types from tarpc output |
| `no_fork_unary` | tarpc-fory | Unary RPC works with upstream tarpc (no fork) |

## Open questions

- Should `StreamOpen` carry a `method_id: u32` hash for server-side dispatch without full deserialization? Recommended: yes.
- Should stream cancellation reuse tarpc's `Cancel { request_id }` or define a new `StreamCancel { stream_id }`? Recommended: new type in `StreamClientMessage` — cleaner separation from tarpc's unary cancel.
