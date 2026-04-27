//! End-to-end integration tests for `tarpc-fory`.
//!
//! Exercises the public re-export surface (`tarpc_fory::connect`,
//! `tarpc_fory::listen`, `tarpc_fory::Incoming`, and the envelope types) over
//! a real TCP loopback connection. Uses `String` as both request and response
//! type because built-in types are registered by fory internally and do not
//! require explicit `register` calls, avoiding type-index collisions.

use fory::Fory;
use futures::{SinkExt as _, StreamExt as _};
use std::sync::Arc;
use tarpc::{ClientMessage, Request, Response, context};
use tarpc_fory::{
    ForyClientMessage, ForyRequest, ForyResponse, ForyResult, ForyServerError, ForyTraceContext,
};

// ---------------------------------------------------------------------------
// Helper: build a shared Fory registry with all envelope types registered.
// ---------------------------------------------------------------------------

fn make_fory() -> Arc<Fory> {
    let mut fory = Fory::default();
    fory.register::<ForyTraceContext>(2).unwrap();
    fory.register::<ForyServerError>(3).unwrap();
    fory.register::<ForyResult<String>>(4).unwrap();
    fory.register::<ForyRequest<String>>(5).unwrap();
    fory.register::<ForyResponse<String>>(6).unwrap();
    fory.register::<ForyClientMessage<String>>(7).unwrap();
    Arc::new(fory)
}

// ---------------------------------------------------------------------------
// Test 1: single raw round-trip
//
// Sends one `ClientMessage<String>` from client to server and receives the
// corresponding `Response<String>` back. Validates:
//   - `tarpc_fory::listen` binds and yields an `Incoming` stream
//   - `tarpc_fory::connect` establishes a typed transport
//   - `ForyEnvelopeCodec` can serialize/deserialize `ClientMessage<String>`
//   - `ForyEnvelopeCodec` can serialize/deserialize `Response<String>`
//   - request-id correlation is preserved
// ---------------------------------------------------------------------------

#[tokio::test]
async fn raw_string_round_trip_over_tarpc_fory() {
    let fory = make_fory();

    let mut listener: tarpc_fory::Incoming<String, String> =
        tarpc_fory::listen("127.0.0.1:0", fory.clone())
            .await
            .unwrap();
    let addr = listener.local_addr();

    // Server: accept one connection, receive one request, send echo response.
    tokio::spawn(async move {
        if let Some(Ok(mut transport)) = listener.next().await {
            if let Some(Ok(ClientMessage::Request(req))) = transport.next().await {
                let resp = Response {
                    request_id: req.id,
                    message: Ok(format!("hi {}", req.message)),
                };
                transport.send(resp).await.unwrap();
            }
        }
    });

    // Client: connect, send request, receive response.
    let mut transport = tarpc_fory::connect::<_, String, String>(addr, fory)
        .await
        .unwrap();

    let request = ClientMessage::Request(Request {
        context: context::current(),
        id: 99,
        message: "world".to_string(),
    });

    transport.send(request).await.unwrap();

    let resp = transport.next().await.unwrap().unwrap();
    assert_eq!(resp.request_id, 99);
    assert_eq!(resp.message.unwrap(), "hi world");
}

// ---------------------------------------------------------------------------
// Test 2: multiple sequential round-trips
//
// Verifies request-ID correlation when several messages are sent in sequence.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn raw_string_multiple_round_trips_over_tarpc_fory() {
    let fory = make_fory();

    let mut listener: tarpc_fory::Incoming<String, String> =
        tarpc_fory::listen("127.0.0.1:0", fory.clone())
            .await
            .unwrap();
    let addr = listener.local_addr();

    // Server: echo each request back until client disconnects.
    tokio::spawn(async move {
        if let Some(Ok(mut transport)) = listener.next().await {
            while let Some(Ok(ClientMessage::Request(req))) = transport.next().await {
                let resp = Response {
                    request_id: req.id,
                    message: Ok(format!("resp:{}", req.message)),
                };
                if transport.send(resp).await.is_err() {
                    break;
                }
            }
        }
    });

    let mut transport = tarpc_fory::connect::<_, String, String>(addr, fory)
        .await
        .unwrap();

    for i in 0u64..5 {
        let request = ClientMessage::Request(Request {
            context: context::current(),
            id: i,
            message: format!("msg{}", i),
        });
        transport.send(request).await.unwrap();

        let resp = transport.next().await.unwrap().unwrap();
        assert_eq!(resp.request_id, i);
        assert_eq!(resp.message.unwrap(), format!("resp:msg{}", i));
    }
}
