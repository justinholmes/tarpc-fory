//! Apache Fory binary transport for [`tarpc`](https://docs.rs/tarpc).
//!
//! Provides fory-serialized transport, envelope types, and a zero-copy codec
//! for tarpc RPC services. Use `#[tarpc::service]` + `#[tarpc_fory::fory_service]`
//! for zero-boilerplate type registration.
//!
//! # Quick start
//!
//! ```ignore
//! #[tarpc::service]
//! #[tarpc_fory::fory_service]
//! trait Hello {
//!     async fn hello(name: String) -> String;
//! }
//!
//! // Server
//! let mut listener = tarpc_fory::listen::<HelloService, _>("127.0.0.1:0")
//!     .await?;
//!
//! // Client
//! let transport = tarpc_fory::connect::<HelloService, _>(addr).await?;
//! let client = HelloClient::new(client::Config::default(), transport).spawn();
//! ```

pub mod envelope;
pub mod transport;
pub mod zerocopy;

// Re-export the proc-macro.
pub use tarpc_fory_macros::fory_service;

// Re-export key types at crate root for ergonomics.
pub use envelope::{
    fory_wire_id, register_envelope_types, ForyClientMessage, ForyRequest, ForyResponse,
    ForyServerError, ForyTraceContext, ServiceWireSchema,
};
pub use transport::ForyEnvelopeCodec;

// Re-export tokio-serde-fory codec.
pub use tokio_serde_fory::ForyCodec;

// Feature-gated TCP re-exports.
#[cfg(feature = "tcp")]
pub use transport::{connect, connect_with_fory, listen, listen_with_fory, Incoming};

// Feature-gated TLS re-exports.
#[cfg(all(feature = "tcp", feature = "tls"))]
pub use transport::{connect_tls, connect_tls_with_fory, listen_tls, listen_tls_with_fory, TlsIncoming};

// Zero-copy transport re-exports.
pub use zerocopy::{
    ClientZeroCopyCodec, ServerZeroCopyCodec, ToFrame, ZeroCopyIncoming,
    ZeroCopyServerTransport, ZeroCopySink, ZeroCopyTransport,
    connect_zerocopy, listen_zerocopy,
    encode_frame, split_frame_body, MAX_FRAME_LEN,
};

// TLS zero-copy re-exports.
#[cfg(feature = "tls")]
pub use zerocopy::{
    ZeroCopyTlsIncoming, ZeroCopyTlsServerTransport, ZeroCopyTlsTransport,
    connect_zerocopy_tls, listen_zerocopy_tls,
};

/// Streaming mode for a service method, as emitted by `#[fory_service]`.
///
/// Returned by `XxxService::streaming_methods()` for each method annotated
/// with `#[streaming(server)]`, `#[streaming(client)]`, or
/// `#[streaming(bidirectional)]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamingMode {
    /// Server sends a stream; client sends a single request.
    Server,
    /// Client sends a stream; server replies once.
    Client,
    /// Both sides stream concurrently.
    Bidirectional,
}
