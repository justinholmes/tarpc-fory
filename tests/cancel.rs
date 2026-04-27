//! Tests for `ClientMessage::Cancel` transmission and reception.
//!
//! Verifies that a Cancel message is correctly serialized by the client-side
//! codec, transmitted over TCP, and deserialized by the server-side codec with
//! the `trace_context` and `request_id` fields intact.
//!
//! Also tests the combined sequence: client sends Request → client sends
//! Cancel → server reads Request → server reads Cancel → server sends error
//! response → client reads response.

use fory::Fory;
use futures::{SinkExt as _, StreamExt as _};
use std::sync::Arc;
use std::time::Duration;
use tarpc::{ClientMessage, Request, Response, ServerError, context, trace};
use tarpc_fory::{
    ForyClientMessage, ForyRequest, ForyResponse, ForyResult, ForyServerError, ForyTraceContext,
};

fn make_fory() -> Arc<Fory> {
    let mut fory = Fory::default();
    fory.register_serializer::<ForyTraceContext>(2).unwrap();
    fory.register_serializer::<ForyServerError>(3).unwrap();
    fory.register_serializer::<ForyResult<String>>(4).unwrap();
    fory.register_serializer::<ForyRequest<String>>(5).unwrap();
    fory.register_serializer::<ForyResponse<String>>(6).unwrap();
    fory.register_serializer::<ForyClientMessage<String>>(7).unwrap();
    Arc::new(fory)
}

/// Client sends a bare Cancel (no prior Request).  Server reads it and
/// asserts the variant is Cancel with the expected `request_id`.
#[tokio::test]
async fn cancel_message_received_by_server() {
    let fory = make_fory();

    let mut listener: tarpc_fory::Incoming<String, String> =
        tarpc_fory::listen("127.0.0.1:0", fory.clone()).await.unwrap();
    let addr = listener.local_addr();

    let trace_id_raw: u128 = 0xDEAD_BEEF_CAFE_0001_0002_0003_0004_0005_u128;
    let span_id_raw: u64 = 0xA1B2_C3D4_E5F6_0001_u64;

    let (tx, rx) = tokio::sync::oneshot::channel::<(u128, u64, u64)>();

    // Server: read one message, assert it is Cancel, send back the fields.
    tokio::spawn(async move {
        if let Some(Ok(mut transport)) = listener.next().await {
            if let Some(Ok(ClientMessage::Cancel { trace_context, request_id })) =
                transport.next().await
            {
                let _ = tx.send((
                    u128::from(trace_context.trace_id),
                    u64::from(trace_context.span_id),
                    request_id,
                ));
            }
        }
    });

    let mut transport = tarpc_fory::connect::<_, String, String>(addr, fory)
        .await
        .unwrap();

    let cancel = ClientMessage::<String>::Cancel {
        trace_context: trace::Context {
            trace_id: trace::TraceId::from(trace_id_raw),
            span_id: trace::SpanId::from(span_id_raw),
            sampling_decision: trace::SamplingDecision::Sampled,
        },
        request_id: 42,
    };
    transport.send(cancel).await.unwrap();

    let (got_trace_id, got_span_id, got_req_id) =
        tokio::time::timeout(Duration::from_secs(5), rx)
            .await
            .expect("timeout")
            .expect("server task panicked");

    assert_eq!(got_trace_id, trace_id_raw, "trace_id mismatch");
    assert_eq!(got_span_id, span_id_raw, "span_id mismatch");
    assert_eq!(got_req_id, 42, "request_id mismatch");
}

/// Full sequence: Request → Cancel → server reads both → server sends error
/// response → client reads response.
///
/// Ordering: client sends Request first, then Cancel.  Server reads Request
/// (message 1), processes it (sends back an error), then reads Cancel (message 2).
/// We verify all three messages flow correctly.
#[tokio::test]
async fn request_then_cancel_full_sequence() {
    let fory = make_fory();

    let mut listener: tarpc_fory::Incoming<String, String> =
        tarpc_fory::listen("127.0.0.1:0", fory.clone()).await.unwrap();
    let addr = listener.local_addr();

    // Channel to report the Cancel fields the server saw.
    let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<u64>();

    tokio::spawn(async move {
        if let Some(Ok(mut transport)) = listener.next().await {
            // Message 1: Request
            if let Some(Ok(ClientMessage::Request(req))) = transport.next().await {
                // Reply with an error to signal processing happened.
                let resp: Response<String> = Response {
                    request_id: req.id,
                    message: Err(ServerError::new(
                        std::io::ErrorKind::Other,
                        "cancelled".to_string(),
                    )),
                };
                transport.send(resp).await.unwrap();
            }
            // Message 2: Cancel
            if let Some(Ok(ClientMessage::Cancel { request_id, .. })) = transport.next().await {
                let _ = cancel_tx.send(request_id);
            }
        }
    });

    let mut transport = tarpc_fory::connect::<_, String, String>(addr, fory)
        .await
        .unwrap();

    let req_id = 99u64;
    let trace_ctx = context::current().trace_context;

    // Send Request.
    transport
        .send(ClientMessage::Request(Request {
            context: context::current(),
            id: req_id,
            message: "do work".to_string(),
        }))
        .await
        .unwrap();

    // Send Cancel for the same request_id.
    transport
        .send(ClientMessage::<String>::Cancel {
            trace_context: trace_ctx,
            request_id: req_id,
        })
        .await
        .unwrap();

    // Receive the error response the server sent for the Request.
    let resp = tokio::time::timeout(Duration::from_secs(5), transport.next())
        .await
        .expect("timeout waiting for response")
        .expect("stream ended")
        .expect("transport error");

    assert_eq!(resp.request_id, req_id);
    let err = resp.message.unwrap_err();
    assert_eq!(err.detail, "cancelled");

    // Verify the server actually received the Cancel.
    let cancel_req_id = tokio::time::timeout(Duration::from_secs(5), cancel_rx)
        .await
        .expect("timeout waiting for cancel signal")
        .expect("server task dropped");
    assert_eq!(cancel_req_id, req_id, "Cancel request_id mismatch");
}
