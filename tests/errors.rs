//! Round-trip tests for error responses through the tarpc-fory transport.
//!
//! Server returns `Response { message: Err(ServerError) }` for each variant of
//! `io::ErrorKind`. Client receives it and verifies `kind` and `detail` are
//! preserved after the full TCP round-trip.
//!
//! Uses `String` as the generic parameter throughout to avoid compile-time
//! type-index collisions between the test crate and the tarpc lib crate.
//! See `fory_envelope.rs` in tarpc-fork for a detailed explanation.

use fory::Fory;
use futures::{SinkExt as _, StreamExt as _};
use std::io;
use std::sync::Arc;
use std::time::Duration;
use tarpc::{ClientMessage, Request, Response, ServerError, context};
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

/// Spin up a server that reads one request and responds with a ServerError of
/// the given kind.  Returns the bound address.
async fn server_returns_error(
    fory: Arc<Fory>,
    kind: io::ErrorKind,
    detail: &'static str,
    request_id_out: u64,
) -> std::net::SocketAddr {
    let mut listener: tarpc_fory::Incoming<String, String> =
        tarpc_fory::listen("127.0.0.1:0", fory).await.unwrap();
    let addr = listener.local_addr();

    tokio::spawn(async move {
        if let Some(Ok(mut transport)) = listener.next().await {
            if let Some(Ok(ClientMessage::Request(req))) = transport.next().await {
                let resp: Response<String> = Response {
                    request_id: req.id,
                    message: Err(ServerError::new(kind, detail.to_string())),
                };
                transport.send(resp).await.unwrap();
            }
        }
    });

    let _ = request_id_out; // used by caller for assertion
    addr
}

#[tokio::test]
async fn server_error_not_found_round_trip() {
    let fory = make_fory();
    let addr = server_returns_error(fory.clone(), io::ErrorKind::NotFound, "not found", 1).await;

    let mut transport = tarpc_fory::connect::<_, String, String>(addr, fory)
        .await
        .unwrap();
    transport
        .send(ClientMessage::Request(Request {
            context: context::current(),
            id: 1,
            message: "x".to_string(),
        }))
        .await
        .unwrap();

    let resp = tokio::time::timeout(Duration::from_secs(5), transport.next())
        .await
        .expect("timeout")
        .expect("stream ended")
        .expect("transport error");

    assert_eq!(resp.request_id, 1);
    let err = resp.message.unwrap_err();
    assert_eq!(err.kind, io::ErrorKind::NotFound, "kind mismatch");
    assert_eq!(err.detail, "not found");
}

#[tokio::test]
async fn server_error_permission_denied_round_trip() {
    let fory = make_fory();
    let addr =
        server_returns_error(fory.clone(), io::ErrorKind::PermissionDenied, "denied", 2).await;

    let mut transport = tarpc_fory::connect::<_, String, String>(addr, fory)
        .await
        .unwrap();
    transport
        .send(ClientMessage::Request(Request {
            context: context::current(),
            id: 2,
            message: "y".to_string(),
        }))
        .await
        .unwrap();

    let resp = tokio::time::timeout(Duration::from_secs(5), transport.next())
        .await
        .expect("timeout")
        .expect("stream ended")
        .expect("transport error");

    assert_eq!(resp.request_id, 2);
    let err = resp.message.unwrap_err();
    assert_eq!(err.kind, io::ErrorKind::PermissionDenied);
    assert_eq!(err.detail, "denied");
}

#[tokio::test]
async fn server_error_timed_out_round_trip() {
    let fory = make_fory();
    let addr =
        server_returns_error(fory.clone(), io::ErrorKind::TimedOut, "deadline exceeded", 3).await;

    let mut transport = tarpc_fory::connect::<_, String, String>(addr, fory)
        .await
        .unwrap();
    transport
        .send(ClientMessage::Request(Request {
            context: context::current(),
            id: 3,
            message: "z".to_string(),
        }))
        .await
        .unwrap();

    let resp = tokio::time::timeout(Duration::from_secs(5), transport.next())
        .await
        .expect("timeout")
        .expect("stream ended")
        .expect("transport error");

    assert_eq!(resp.request_id, 3);
    let err = resp.message.unwrap_err();
    assert_eq!(err.kind, io::ErrorKind::TimedOut);
    assert_eq!(err.detail, "deadline exceeded");
}

#[tokio::test]
async fn server_error_other_round_trip() {
    let fory = make_fory();
    let addr =
        server_returns_error(fory.clone(), io::ErrorKind::Other, "something went wrong", 4).await;

    let mut transport = tarpc_fory::connect::<_, String, String>(addr, fory)
        .await
        .unwrap();
    transport
        .send(ClientMessage::Request(Request {
            context: context::current(),
            id: 4,
            message: "w".to_string(),
        }))
        .await
        .unwrap();

    let resp = tokio::time::timeout(Duration::from_secs(5), transport.next())
        .await
        .expect("timeout")
        .expect("stream ended")
        .expect("transport error");

    assert_eq!(resp.request_id, 4);
    let err = resp.message.unwrap_err();
    assert_eq!(err.kind, io::ErrorKind::Other);
    assert_eq!(err.detail, "something went wrong");
}

/// Verify request_id correlation is preserved even on the error path.
#[tokio::test]
async fn server_error_request_id_correlation() {
    let fory = make_fory();

    let mut listener: tarpc_fory::Incoming<String, String> =
        tarpc_fory::listen("127.0.0.1:0", fory.clone()).await.unwrap();
    let addr = listener.local_addr();

    tokio::spawn(async move {
        if let Some(Ok(mut transport)) = listener.next().await {
            // Echo three requests back with errors bearing the original request_id.
            for _ in 0..3 {
                if let Some(Ok(ClientMessage::Request(req))) = transport.next().await {
                    let resp: Response<String> = Response {
                        request_id: req.id,
                        message: Err(ServerError::new(
                            io::ErrorKind::NotFound,
                            format!("missing:{}", req.id),
                        )),
                    };
                    if transport.send(resp).await.is_err() {
                        break;
                    }
                }
            }
        }
    });

    let mut transport = tarpc_fory::connect::<_, String, String>(addr, fory)
        .await
        .unwrap();

    for i in 10u64..13 {
        transport
            .send(ClientMessage::Request(Request {
                context: context::current(),
                id: i,
                message: format!("req{}", i),
            }))
            .await
            .unwrap();
        let resp = tokio::time::timeout(Duration::from_secs(5), transport.next())
            .await
            .expect("timeout")
            .expect("stream ended")
            .expect("transport error");

        assert_eq!(resp.request_id, i, "request_id mismatch for i={i}");
        let err = resp.message.unwrap_err();
        assert_eq!(err.kind, io::ErrorKind::NotFound);
        assert_eq!(err.detail, format!("missing:{}", i));
    }
}
