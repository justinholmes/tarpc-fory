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

## Quick start

```rust,ignore
use fory::{Fory, ForyObject};
use std::sync::Arc;
use tarpc_fory::{
    ForyClientMessage, ForyRequest, ForyResponse, ForyResult,
    ForyServerError, ForyTraceContext,
};

// 1. Build a fory registry. Register the wrapper envelopes and your inner T.
let mut fory = Fory::default();
fory.register::<ForyTraceContext>(2).unwrap();
fory.register::<ForyServerError>(3).unwrap();
fory.register::<ForyResult<String>>(4).unwrap();
fory.register::<ForyRequest<String>>(5).unwrap();
fory.register::<ForyResponse<String>>(6).unwrap();
fory.register::<ForyClientMessage<String>>(7).unwrap();
let fory = Arc::new(fory);

// 2. Connect / listen.
let transport = tarpc_fory::connect::<_, String, String>("127.0.0.1:8080", fory.clone()).await?;
let listener = tarpc_fory::listen::<_, String, String>("127.0.0.1:0", fory).await?;
```

The inner `T` (here `String`) must implement `fory::Serializer + fory::ForyDefault + 'static`. Fory ships built-in impls for primitives, `String`, `Vec<T>`, `Option<T>`, `HashMap<K, V>`, etc. For your own types, derive `ForyObject` and register them.

## Status: experimental, partial integration

**What works (tested):**
- Wire codec — encodes/decodes Apache Fory bytes via `tokio-serde`
- Wrapper envelope types (`ForyClientMessage<T>`, `ForyResponse<T>`, etc.) — round-trip through `Fory::serialize`/`deserialize` and across TCP
- Lower-level `Sink + Stream` transport API — manual `ClientMessage<T>` / `Response<T>` over a TCP connection works correctly for any `T: fory::Serializer + fory::ForyDefault + 'static`
- Multiplexing, large payloads, error responses, cancel, multi-client, full `io::ErrorKind` mapping (29 integration tests, all green)

**What does not yet work:**
- The canonical `#[tarpc::service]` proc-macro flow: the generated request/response types derive `serde::{Serialize, Deserialize}` but NOT `fory::ForyObject`, so the high-level `client::Stub` / `server::BaseChannel` machinery cannot use this transport. **You cannot define a service trait with `#[tarpc::service]` and run it over fory yet.**

**Why:** Apache Fory 0.17's serializer trait (`fory::Serializer`) is incompatible with `serde::Serialize` — there is no built-in adapter, and `Result<T, E>`, `std::time::Instant`, and `std::io::ErrorKind` lack `fory::Serializer` impls in fory-core. Wrapper types in this crate paper over the second issue but cannot fix the first; the proc-macro-generated types are out of our reach.

**Path to full integration:** add a serde compatibility layer in `apache/fory` (blanket `impl<T: serde::Serialize> fory::Serializer for T` and stdlib type impls) so any `serde`-deriving type works as a Fory payload. That contribution is being scoped separately; until it lands and ships in a fory release, this crate is not a drop-in `#[tarpc::service]` codec.

If your use case fits the lower-level transport API (you control your own message types and don't need the service macro), this crate works today. If you want `#[tarpc::service]` over fory, wait.

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
