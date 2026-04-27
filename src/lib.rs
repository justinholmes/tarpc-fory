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
//! fory.register::<tarpc_fory::ForyTraceContext>(2).unwrap();
//! fory.register::<tarpc_fory::ForyServerError>(3).unwrap();
//! fory.register::<tarpc_fory::ForyResult<String>>(4).unwrap();
//! fory.register::<tarpc_fory::ForyRequest<String>>(5).unwrap();
//! fory.register::<tarpc_fory::ForyResponse<String>>(6).unwrap();
//! fory.register::<tarpc_fory::ForyClientMessage<String>>(7).unwrap();
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
    ForyResult, ForyTraceContext,
};
pub use tokio_serde_fory::ForyCodec;
