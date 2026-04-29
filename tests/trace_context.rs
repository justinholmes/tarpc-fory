//! Round-trip tests for non-default trace contexts through the transport.
//!
//! Verifies that:
//!   - A non-default `trace_id` (full u128) is preserved bit-for-bit.
//!   - A non-default `span_id` (u64) is preserved bit-for-bit.
//!   - Both `Sampled` and `Unsampled` sampling decisions survive the wire.
//!
//! Uses `u32` as the generic parameter to avoid compile-time type-index
//! collisions (u32 is a builtin and does not occupy the derive-macro index).

use fory::Fory;
use futures::{SinkExt as _, StreamExt as _};
use std::sync::Arc;
use std::time::Duration;
use tarpc::{ClientMessage, Request, Response, context, trace};
use tarpc_fory::{
    ForyClientMessage, ForyRequest, ForyResponse, ForyServerError, ForyTraceContext,
};

fn make_fory() -> Arc<Fory> {
    let mut fory = Fory::default();
    fory.register_serializer::<ForyTraceContext>(2).unwrap();
    fory.register_serializer::<ForyServerError>(3).unwrap();
    fory.register_serializer::<ForyRequest<u32>>(5).unwrap();
    fory.register_serializer::<ForyResponse<u32>>(6).unwrap();
    fory.register_serializer::<ForyClientMessage<u32>>(7).unwrap();
    Arc::new(fory)
}

/// Spin up a server that echoes back the trace context it received.
/// Returns (addr, JoinHandle) where the JoinHandle resolves to the decoded
/// trace context fields (trace_id_raw, span_id_raw, sampling_byte).
async fn spawn_trace_echo_server(
    fory: Arc<Fory>,
) -> (
    std::net::SocketAddr,
    tokio::sync::oneshot::Receiver<(u128, u64, u8)>,
) {
    let mut listener: tarpc_fory::Incoming<u32, u32> =
        tarpc_fory::listen_with_fory("127.0.0.1:0", fory).await.unwrap();
    let addr = listener.local_addr();
    let (tx, rx) = tokio::sync::oneshot::channel::<(u128, u64, u8)>();

    tokio::spawn(async move {
        if let Some(Ok(mut transport)) = listener.next().await {
            if let Some(Ok(ClientMessage::Request(req))) = transport.next().await {
                let tc = &req.context.trace_context;
                let _ = tx.send((
                    u128::from(tc.trace_id),
                    u64::from(tc.span_id),
                    u8::from(tc.sampling_decision),
                ));
                // Echo back a trivial response so client doesn't stall.
                let resp = Response {
                    request_id: req.id,
                    message: Ok(0u32),
                };
                let _ = transport.send(resp).await;
            }
        }
    });

    (addr, rx)
}

/// Non-default trace_id and span_id with Sampled decision survive the wire.
#[tokio::test]
async fn trace_context_sampled_preserves_ids() {
    let fory = make_fory();

    let trace_id_raw: u128 = 0x1234_5678_9abc_def0_fedc_ba98_7654_3210_u128;
    let span_id_raw: u64 = 0xA1B2_C3D4_E5F6_0001_u64;

    let (addr, rx) = spawn_trace_echo_server(fory.clone()).await;

    let mut transport = tarpc_fory::connect_with_fory::<_, u32, u32>(addr, fory).await.unwrap();

    // context::Context is #[non_exhaustive]; start from current() and override
    // the trace_context field directly.
    let mut ctx = context::current();
    ctx.trace_context = trace::Context {
        trace_id: trace::TraceId::from(trace_id_raw),
        span_id: trace::SpanId::from(span_id_raw),
        sampling_decision: trace::SamplingDecision::Sampled,
    };

    transport
        .send(ClientMessage::Request(Request {
            context: ctx,
            id: 1,
            message: 0u32,
        }))
        .await
        .unwrap();

    // Wait for response so server has had time to process.
    tokio::time::timeout(Duration::from_secs(5), transport.next())
        .await
        .expect("timeout")
        .expect("stream ended")
        .expect("transport error");

    let (got_trace, got_span, got_sampling) =
        tokio::time::timeout(Duration::from_secs(5), rx)
            .await
            .expect("server timeout")
            .expect("server dropped sender");

    assert_eq!(got_trace, trace_id_raw, "trace_id corrupted");
    assert_eq!(got_span, span_id_raw, "span_id corrupted");
    assert_eq!(got_sampling, 1u8, "Sampled should encode as 1");
}

/// Non-default trace_id and span_id with Unsampled decision survive the wire.
#[tokio::test]
async fn trace_context_unsampled_preserves_ids() {
    let fory = make_fory();

    let trace_id_raw: u128 = 0xFEDC_BA98_7654_3210_1234_5678_9ABC_DEF0_u128;
    let span_id_raw: u64 = 0x0011_2233_4455_6677_u64;

    let (addr, rx) = spawn_trace_echo_server(fory.clone()).await;

    let mut transport = tarpc_fory::connect_with_fory::<_, u32, u32>(addr, fory).await.unwrap();

    let mut ctx = context::current();
    ctx.trace_context = trace::Context {
        trace_id: trace::TraceId::from(trace_id_raw),
        span_id: trace::SpanId::from(span_id_raw),
        sampling_decision: trace::SamplingDecision::Unsampled,
    };

    transport
        .send(ClientMessage::Request(Request {
            context: ctx,
            id: 2,
            message: 0u32,
        }))
        .await
        .unwrap();

    tokio::time::timeout(Duration::from_secs(5), transport.next())
        .await
        .expect("timeout")
        .expect("stream ended")
        .expect("transport error");

    let (got_trace, got_span, got_sampling) =
        tokio::time::timeout(Duration::from_secs(5), rx)
            .await
            .expect("server timeout")
            .expect("server dropped sender");

    assert_eq!(got_trace, trace_id_raw, "trace_id corrupted");
    assert_eq!(got_span, span_id_raw, "span_id corrupted");
    assert_eq!(got_sampling, 0u8, "Unsampled should encode as 0");
}

/// In-memory round-trip: ForyTraceContext serializes/deserializes the full
/// u128 trace_id without truncation or corruption.
#[test]
fn fory_trace_context_full_u128_round_trip() {
    let mut fory = Fory::default();
    fory.register_serializer::<ForyTraceContext>(2).unwrap();

    let original = ForyTraceContext {
        trace_id: 0x1234_5678_9abc_def0_fedc_ba98_7654_3210_u128,
        span_id: 0xA1B2_C3D4_E5F6_0001_u64,
        sampling: 1,
    };

    let bytes = fory.serialize(&original).unwrap();
    let decoded: ForyTraceContext = fory.deserialize(&bytes).unwrap();

    assert_eq!(decoded.trace_id, original.trace_id, "trace_id truncated");
    assert_eq!(decoded.span_id, original.span_id, "span_id corrupted");
    assert_eq!(decoded.sampling, 1u8, "sampling mismatch");
}

/// Unsampled variant also round-trips in-memory.
#[test]
fn fory_trace_context_unsampled_round_trip() {
    let mut fory = Fory::default();
    fory.register_serializer::<ForyTraceContext>(2).unwrap();

    let original = ForyTraceContext {
        trace_id: 0xFEDC_BA98_7654_3210_1234_5678_9ABC_DEF0_u128,
        span_id: 0x0011_2233_4455_6677_u64,
        sampling: 0,
    };

    let bytes = fory.serialize(&original).unwrap();
    let decoded: ForyTraceContext = fory.deserialize(&bytes).unwrap();

    assert_eq!(decoded.trace_id, original.trace_id);
    assert_eq!(decoded.span_id, original.span_id);
    assert_eq!(decoded.sampling, 0u8, "Unsampled should be 0");
}
