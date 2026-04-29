# tarpc-fory

[![License: MIT/Apache-2.0](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](#license)
[![tarpc 0.37](https://img.shields.io/badge/tarpc-0.37-blue.svg)](https://docs.rs/tarpc/0.37)

[Apache Fory](https://fury.apache.org/) binary transport for [`tarpc`](https://docs.rs/tarpc) 0.37.

## Overview

`tarpc-fory` is a zero-boilerplate Fory wire codec for tarpc. It uses **upstream tarpc 0.37**
with no fork. A companion proc-macro crate (`tarpc-fory-macros`) emits type registration and
`ServiceWireSchema` impls so user code never calls `fory.register*` manually.

It pulls in [`tokio-serde-fory`](../tokio-serde-fory) — a standalone `tokio_serde::{Serializer,
Deserializer}` implementation backed by Fory — as the underlying framing layer.

## Quick Start

Two attributes on the trait. Nothing else.

```rust,ignore
use futures::StreamExt as _;
use tarpc::{client, context, server::{self, Channel}};

#[tarpc::service(derive = [Clone, serde::Serialize, serde::Deserialize])]
#[tarpc_fory::fory_service]
pub trait Hello {
    async fn hello(name: String) -> String;
}

#[derive(Clone)]
struct HelloServer;

impl Hello for HelloServer {
    async fn hello(self, _: context::Context, name: String) -> String {
        format!("hello, {}", name)
    }
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    // Server — zero manual registration.
    let mut listener = tarpc_fory::listen::<HelloService, _>("127.0.0.1:0").await?;
    let addr = listener.local_addr();

    tokio::spawn(async move {
        while let Some(Ok(transport)) = listener.next().await {
            let channel = tarpc::server::BaseChannel::with_defaults(transport);
            tokio::spawn(
                channel
                    .execute(HelloServer.serve())
                    .for_each(|fut| async move { tokio::spawn(fut); }),
            );
        }
    });

    // Client — zero manual registration.
    let transport = tarpc_fory::connect::<HelloService, _>(addr).await?;
    let client = HelloClient::new(client::Config::default(), transport).spawn();

    let resp = client.hello(context::current(), "world".to_string()).await?;
    assert_eq!(resp, "hello, world");
    Ok(())
}
```

### Two-attribute pattern

| Attribute | Role |
|---|---|
| `#[tarpc::service]` | Generates the service trait, request/response types, `Client`, and `serve()` |
| `#[tarpc_fory::fory_service]` | Generates a `HelloService` marker type implementing `ServiceWireSchema`, registering all envelope and payload types automatically |

The `HelloService` marker type is the schema token passed to `listen` and `connect`. The
proc-macro derives the full registration list at compile time — no runtime `fory.register*` calls
in application code.

## Features

- **Zero-copy receive** — `ServerZeroCopyCodec` / `ClientZeroCopyCodec` deserialize directly from
  the network buffer: ~2.5 GiB/s on localhost.
- **Vectored-write send** — length prefix and payload are written with `writev`, avoiding a copy
  into a contiguous buffer: ~3.5 GiB/s on localhost.
- **TLS with cert rotation** — `listen_tls` / `connect_tls` accept a `TlsAcceptor` / `TlsConnector`
  and support live cert rotation via `Arc<RwLock<...>>` wrappers.
- **Auto type registration** — `ServiceWireSchema` (emitted by `#[tarpc_fory::fory_service]`)
  registers envelope wrappers, generated request/response types, and any user types referenced in
  method signatures, all with stable wire IDs derived at compile time.

## Codec Paths

### Classic (envelope codec)

```rust,ignore
tarpc_fory::listen::<HelloService, _>(addr)
tarpc_fory::connect::<HelloService, _>(addr)
```

Serializes the full tarpc envelope (`ClientMessage<T>`, `Response<T>`) via Fory. Correct for all
use cases. Slightly higher per-message overhead than zero-copy because the envelope fields are
heap-allocated individually.

**Use when:** correctness is the priority, payload sizes are small to medium, or you need
out-of-the-box TLS.

### Zero-copy

```rust,ignore
tarpc_fory::listen_zerocopy::<HelloService, _>(addr)
tarpc_fory::connect_zerocopy::<HelloService, _>(addr)
```

Bypasses intermediate allocation on receive by deserializing directly from the `BytesMut` read
buffer. On send, uses `writev` to write the length prefix and Fory payload as separate iovec
entries.

**Use when:** payload sizes exceed ~64 KiB, throughput is critical, or profiling shows the classic
codec as a bottleneck.

TLS zero-copy variants are available via `connect_zerocopy_tls` / `listen_zerocopy_tls` (requires
the `tls` feature).

## Exports

```rust,ignore
// Core re-exports from tarpc_fory::
pub use tarpc_fory::{
    // Proc-macro
    fory_service,

    // Envelope types
    ForyClientMessage, ForyRequest, ForyResponse, ForyServerError, ForyTraceContext,
    ServiceWireSchema, fory_wire_id, register_envelope_types,

    // Codec
    ForyEnvelopeCodec,
    ForyCodec,          // from tokio-serde-fory

    // TCP (feature = "tcp")
    connect, connect_with_fory, listen, listen_with_fory, Incoming,

    // TLS (feature = "tcp" + "tls")
    connect_tls, connect_tls_with_fory, listen_tls, listen_tls_with_fory, TlsIncoming,

    // Zero-copy
    connect_zerocopy, listen_zerocopy,
    ClientZeroCopyCodec, ServerZeroCopyCodec, ToFrame,
    ZeroCopyIncoming, ZeroCopyServerTransport, ZeroCopySink, ZeroCopyTransport,

    // TLS zero-copy (feature = "tls")
    connect_zerocopy_tls, listen_zerocopy_tls,
    ZeroCopyTlsIncoming, ZeroCopyTlsServerTransport, ZeroCopyTlsTransport,
};
```

## Status

Working. 68 tests covering round-trip encoding, multiplexed requests, large payloads (16 MiB),
TLS, zero-copy send/receive, and full end-to-end service calls over TCP localhost.

```bash
cargo test           # all tests
cargo test --release # includes 16 MiB payload benchmark
```

## Companion Crate

[`tokio-serde-fory`](../tokio-serde-fory) — standalone `tokio_serde` codec backed by Fory.
Usable independently of `tarpc-fory` wherever a `tokio_serde::{Serializer, Deserializer}` is
needed.

## License

Dual-licensed under either of:

- [MIT license](LICENSE-MIT)
- [Apache License, Version 2.0](LICENSE-APACHE)

at your option.
