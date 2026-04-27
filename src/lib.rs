//! Apache Fory transport for [tarpc](https://docs.rs/tarpc).
//!
//! Re-exports the `fory` codec module from `tarpc::serde_transport::fory`,
//! plus the wrapper envelope types from `tarpc::serde_transport::fory_envelope`,
//! so consumers can depend on a single stable crate name (`tarpc-fory`)
//! without manually pinning a fork branch or flipping feature flags on tarpc
//! directly.
//!
//! ## Status
//!
//! Currently depends on a fork of `google/tarpc` while the upstream PR adding
//! the `fory` feature is in flight. Once merged and released, `tarpc-fory`
//! will repoint at the published `tarpc` version and you can drop this
//! wrapper in favor of `tarpc = { features = ["serde-transport-fory"] }`.
//!
//! ## Example
//!
//! ```ignore
//! use fory::{Fory, ForyObject};
//! use std::sync::Arc;
//! use tarpc::{client, context};
//!
//! #[tarpc::service]
//! trait Hello {
//!     async fn hello(name: String) -> String;
//! }
//!
//! # async fn example() -> std::io::Result<()> {
//! let mut fory = Fory::default();
//! // Register HelloRequest, HelloResponse, and the fory wrapper envelope types.
//! let fory = Arc::new(fory);
//!
//! let transport = tarpc_fory::connect::<_, HelloRequest, HelloResponse>(
//!     "127.0.0.1:8080",
//!     fory,
//! ).await?;
//! let client = HelloClient::new(client::Config::default(), transport).spawn();
//! let resp = client.hello(context::current(), "world".into()).await?;
//! # Ok(()) }
//! ```

pub use tarpc::serde_transport::fory::{connect, listen, Incoming, ForyEnvelopeCodec};
pub use tarpc::serde_transport::fory_envelope::{
    ForyClientMessage, ForyRequest, ForyResponse, ForyServerError,
    ForyResult, ForyTraceContext,
};
pub use tokio_serde_fory::ForyCodec;
