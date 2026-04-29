//! Multi-client test: 5 clients connect to one listener concurrently.
//!
//! Each client sends a unique request and waits for its own response.
//! Verifies that:
//!   - Multiple TCP connections are accepted independently.
//!   - Payloads are not cross-contaminated across connections.
//!   - All 5 clients complete successfully when run in parallel.

use fory::Fory;
use futures::{SinkExt as _, StreamExt as _};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tarpc::{ClientMessage, Request, Response, context};
use tarpc_fory::{
    ForyClientMessage, ForyRequest, ForyResponse, ForyServerError, ForyTraceContext,
};

fn make_fory() -> Arc<Fory> {
    let mut fory = Fory::default();
    fory.register_serializer::<ForyTraceContext>(2).unwrap();
    fory.register_serializer::<ForyServerError>(3).unwrap();
    fory.register_serializer::<ForyRequest<String>>(5).unwrap();
    fory.register_serializer::<ForyResponse<String>>(6).unwrap();
    fory.register_serializer::<ForyClientMessage<String>>(7).unwrap();
    Arc::new(fory)
}

const NUM_CLIENTS: usize = 5;

/// Accepts one connection, echoes all requests until the client disconnects.
async fn run_echo_server(fory: Arc<Fory>) -> SocketAddr {
    let mut listener: tarpc_fory::Incoming<String, String> =
        tarpc_fory::listen_with_fory("127.0.0.1:0", fory).await.unwrap();
    let addr = listener.local_addr();

    tokio::spawn(async move {
        // Accept NUM_CLIENTS connections, each handled in its own task.
        for _ in 0..NUM_CLIENTS {
            if let Some(Ok(mut transport)) = listener.next().await {
                tokio::spawn(async move {
                    while let Some(Ok(ClientMessage::Request(req))) = transport.next().await {
                        let resp = Response {
                            request_id: req.id,
                            message: Ok(format!("echo:{}", req.message)),
                        };
                        if transport.send(resp).await.is_err() {
                            break;
                        }
                    }
                });
            }
        }
    });

    addr
}

#[tokio::test]
async fn five_clients_concurrent_no_cross_contamination() {
    let fory = make_fory();
    let addr = run_echo_server(fory.clone()).await;

    // Spawn 5 client tasks, each with a unique message and request_id.
    let handles: Vec<_> = (0..NUM_CLIENTS)
        .map(|i| {
            let fory = fory.clone();
            tokio::spawn(async move {
                let mut transport = tarpc_fory::connect_with_fory::<_, String, String>(addr, fory)
                    .await
                    .unwrap();

                let payload = format!("client{}", i);
                let req_id = i as u64 * 100 + 1;

                transport
                    .send(ClientMessage::Request(Request {
                        context: context::current(),
                        id: req_id,
                        message: payload.clone(),
                    }))
                    .await
                    .unwrap();

                let resp = tokio::time::timeout(Duration::from_secs(5), transport.next())
                    .await
                    .unwrap_or_else(|_| panic!("client{i} timed out"))
                    .expect("stream ended")
                    .expect("transport error");

                assert_eq!(resp.request_id, req_id, "client{i}: request_id mismatch");
                let body = resp.message.expect("server returned error");
                assert_eq!(
                    body,
                    format!("echo:{}", payload),
                    "client{i}: payload mismatch"
                );

                i
            })
        })
        .collect();

    // All clients must complete successfully.
    let mut completed: Vec<usize> = futures::future::join_all(handles)
        .await
        .into_iter()
        .map(|r| r.expect("client task panicked"))
        .collect();

    completed.sort_unstable();
    assert_eq!(
        completed,
        (0..NUM_CLIENTS).collect::<Vec<_>>(),
        "not all clients completed"
    );
}
