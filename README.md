# tarpc-fory

[![License: MIT/Apache-2.0](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](#license)

[Apache Fory](https://fury.apache.org/) binary transport for [`tarpc`](https://docs.rs/tarpc).

## Overview

`tarpc-fory` is a thin wrapper that exposes Apache Fory as a wire codec for `tarpc`'s TCP transport. It depends on a fork of `google/tarpc` while [an upstream PR](https://github.com/google/tarpc/pulls) adds the `serde-transport-fory` feature; once merged, this crate will repoint at the published `tarpc` version.

It also pulls in [`tokio-serde-fory`](../tokio-serde-fory) — a standalone `tokio_serde::{Serializer, Deserializer}` impl backed by Fory.

## Why

Fory is a high-performance, cross-language binary serialization format from the Apache Foundation, with bindings for Rust, Java, Python, Go, and JavaScript. If your service already uses Fory for storage or wire interop with non-Rust callers, using it as the RPC codec keeps the encoding consistent end-to-end and benefits from Fory's compact format.

## How it works

tarpc's native envelope types (`ClientMessage<T>`, `Response<T>`, `Request<T>`, `ServerError`) reference stdlib types (`Result<T, E>`, `Instant`, `io::ErrorKind`) that Fory does not provide built-in `Serializer` impls for. To bridge the gap, this crate ships parallel Fory-friendly wrapper types (`ForyClientMessage<T>`, `ForyResponse<T>`, etc.) and a codec adapter (`ForyEnvelopeCodec`) that converts at the wire boundary. From the user's perspective the wrappers are invisible — `tarpc_fory::connect` and `tarpc_fory::listen` accept native `tarpc` types, encode them as Fory bytes, and reverse on receive.

## Status

**Working as of commit 5ae0738 (tarpc fork) / 203b4bf (tarpc-fory):**
- Wire codec — encodes/decodes Apache Fory bytes via `tokio-serde`
- Wrapper envelope types (`ForyClientMessage<T>`, `ForyResponse<T>`, etc.) — round-trip through fory and across TCP
- Lower-level `Sink + Stream` transport API with manual `ClientMessage<T>` / `Response<T>`
- **Canonical `#[tarpc::service]` flow over fory TCP** — define a service trait, run server + client over fory transport, RPC end-to-end

The wrapper envelope types use hand-written `fory::Serializer` impls registered via `fory.register_serializer(id)`, avoiding fory-derive's compile-time `TYPE_ID_COUNTER` (which would otherwise collide with user-derived types from `#[tarpc::service]`-generated request/response payloads). User application types continue to use `#[derive(fory::ForyObject)]` normally — there's no per-user-type rewrite.

29 integration tests on this crate + 3 end-to-end service tests on the underlying tarpc fork — all passing on TCP localhost.

## Quick start

```rust,ignore
use fory::Fory;
use std::sync::Arc;
use tarpc::{client, context, server::{self, Channel}};
use tarpc::serde_transport::fory as fory_transport;
use tarpc::serde_transport::fory_envelope::register_envelope_types;
use futures::StreamExt as _;

// The `derive` list keeps serde happy for tarpc internals;
// the fory feature on the tarpc fork emits #[derive(fory::ForyObject)] too.
#[tarpc::service(derive = [Clone, serde::Serialize, serde::Deserialize])]
trait Hello {
    async fn hello(name: String) -> String;
}

#[derive(Clone)]
struct HelloServer;

impl Hello for HelloServer {
    async fn hello(self, _: context::Context, name: String) -> String {
        format!("hello, {}", name)
    }
}

async fn run() -> std::io::Result<()> {
    let mut fory = Fory::default();

    // Envelope types use register_serializer (EXT path — never touches
    // fory_type_index, so no collision with user types at index 0).
    use tarpc::serde_transport::fory_envelope::{
        ForyClientMessage, ForyRequest, ForyResponse, ForyResult,
        ForyServerError, ForyTraceContext,
    };
    fory.register_serializer::<ForyTraceContext>(2).unwrap();
    fory.register_serializer::<ForyServerError>(3).unwrap();
    fory.register_serializer::<ForyResult<HelloRequest>>(4).unwrap();
    fory.register_serializer::<ForyRequest<HelloRequest>>(5).unwrap();
    fory.register_serializer::<ForyClientMessage<HelloRequest>>(7).unwrap();
    fory.register_serializer::<ForyResult<HelloResponse>>(8).unwrap();
    fory.register_serializer::<ForyResponse<HelloResponse>>(6).unwrap();

    // User types use register (STRUCT path).
    fory.register::<HelloRequest>(100).unwrap();
    fory.register::<HelloResponse>(101).unwrap();

    let fory = Arc::new(fory);

    // Server
    let mut listener = fory_transport::listen::<_, HelloRequest, HelloResponse>(
        "127.0.0.1:0", fory.clone()).await?;
    let addr = listener.local_addr();
    tokio::spawn(async move {
        while let Some(Ok(transport)) = listener.next().await {
            let channel = tarpc::server::BaseChannel::with_defaults(transport);
            tokio::spawn(
                channel.execute(HelloServer.serve())
                    .for_each(|fut| async move { tokio::spawn(fut); })
            );
        }
    });

    // Client
    let transport = fory_transport::connect::<_, HelloRequest, HelloResponse>(
        addr, fory).await?;
    let client = HelloClient::new(client::Config::default(), transport).spawn();
    let resp = client.hello(context::current(), "world".into()).await?;
    assert_eq!(resp, "hello, world");
    Ok(())
}
```

## Testing

```bash
cargo test           # round-trip, multiplex
cargo test --release # also runs the 16 MiB payload test
```

## Versioning

Pinned to `fory = "0.17"`. Major fory upgrades will be major-version bumps here.

## License

Dual-licensed under either of:

- [MIT license](LICENSE-MIT)
- [Apache License, Version 2.0](LICENSE-APACHE)

at your option.
