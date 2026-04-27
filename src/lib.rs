//! Apache Fory binary transport for [`tarpc`](https://docs.rs/tarpc).
//!
//! Re-exports the fory transport from `tarpc::serde_transport::fory` plus
//! the wrapper envelope types from `tarpc::serde_transport::fory_envelope`,
//! letting you depend on a single stable crate name without manually pinning
//! a fork branch or flipping feature flags on tarpc directly.
//!
//! # Status: experimental, partial integration
//!
//! - Wire codec + wrapper envelopes + lower-level transport API: works, tested.
//! - `#[tarpc::service]` proc-macro flow: NOT supported. Generated request/
//!   response types derive `serde::{Serialize, Deserialize}` but not
//!   `fory::ForyObject`, so the `client::Stub` / `server::BaseChannel`
//!   machinery cannot use this transport.
//!
//! Full integration requires upstream apache/fory work (serde compatibility
//! layer). Until that lands, this crate is suitable only for use cases
//! that go through the lower-level `Sink + Stream` transport directly.
//!
//! See README.md for details.
//!
//! # Quick start
//!
//! ```ignore
//! use fory::Fory;
//! use std::sync::Arc;
//!
//! let mut fory = Fory::default();
//! // Use register_envelope_types to register all envelope types at once.
//! // This registers via register_serializer (EXT type path) which avoids the
//! // fory-derive TYPE_ID_COUNTER collision between independently compiled crates.
//! tarpc_fory::register_envelope_types::<String>(&mut fory).unwrap();
//! let fory = Arc::new(fory);
//!
//! # async fn ex(addr: std::net::SocketAddr) -> std::io::Result<()> {
//! let transport = tarpc_fory::connect::<_, String, String>(addr, fory).await?;
//! # Ok(()) }
//! ```
//!
//! See the [README](https://github.com/justinholmes/tarpc-fory) for the
//! full guide including limitations.

pub use tarpc::serde_transport::fory::{connect, listen, Incoming, ForyEnvelopeCodec};
pub use tarpc::serde_transport::fory_envelope::{
    ForyClientMessage, ForyRequest, ForyResponse, ForyServerError,
    ForyResult, ForyTraceContext, register_envelope_types,
};
pub use tokio_serde_fory::ForyCodec;
