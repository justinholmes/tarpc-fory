//! Multiplex stress test for `tarpc-fory`.
//!
//! Proves that request-ID correlation works when many calls are in flight
//! simultaneously on a single connection. Sends 100 requests without waiting
//! for individual replies, then drains all 100 responses and checks that every
//! (id, payload) pair round-trips correctly regardless of server response order.

use fory::Fory;
use futures::{SinkExt as _, StreamExt as _};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tarpc::{ClientMessage, Request, Response, context};
use tarpc_fory::{
    ForyClientMessage, ForyRequest, ForyResponse, ForyResult, ForyServerError, ForyTraceContext,
};

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

const N: u64 = 100;

/// Send all N requests on one connection without waiting for replies, then
/// drain all N responses. The server introduces a variable per-request delay
/// (based on request_id % 10) so responses are likely to arrive out of
/// send-order, exercising the ID-correlation path.
///
/// Asserts:
///   - all N unique request_ids appear exactly once in the response set
///   - every response payload equals `format!("echo:{}", request_id)`
#[tokio::test]
async fn hundred_concurrent_calls_one_connection() {
    let fory = make_fory();

    let mut listener: tarpc_fory::Incoming<String, String> =
        tarpc_fory::listen("127.0.0.1:0", fory.clone())
            .await
            .unwrap();
    let addr = listener.local_addr();

    // Server: accept one connection; for each request, sleep a variable amount
    // to simulate out-of-order processing, then echo back.
    tokio::spawn(async move {
        if let Some(Ok(mut transport)) = listener.next().await {
            // Collect all N requests first, then reply with staggered delays.
            // Use an mpsc channel so the send-side and spawned handlers share
            // the single Sink safely.
            let (tx, mut rx) =
                tokio::sync::mpsc::channel::<Response<String>>(N as usize + 4);

            // Receiver loop: read requests off the stream.
            let recv_tx = tx.clone();
            let recv_handle: tokio::task::JoinHandle<_> = tokio::spawn(async move {
                let mut count = 0u64;
                while let Some(Ok(msg)) = transport.next().await {
                    if let ClientMessage::Request(req) = msg {
                        count += 1;
                        let ch = recv_tx.clone();
                        tokio::spawn(async move {
                            // Variable delay: 0..45 ms — makes responses arrive
                            // out of send-order so ID correlation is exercised.
                            let delay_ms = (req.id % 10) * 5;
                            if delay_ms > 0 {
                                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                            }
                            let resp = Response {
                                request_id: req.id,
                                message: Ok(format!("echo:{}", req.id)),
                            };
                            let _ = ch.send(resp).await;
                        });
                    }
                    if count == N {
                        break;
                    }
                }
                // Signal that no more responses will be produced.
                drop(recv_tx);
                transport
            });

            // Sender loop: forward responses from mpsc onto the transport.
            let mut transport = recv_handle.await.unwrap();
            while let Some(resp) = rx.recv().await {
                if transport.send(resp).await.is_err() {
                    break;
                }
            }
        }
    });

    // Client: connect, send all N requests without waiting for replies, then
    // collect all N responses into a HashMap keyed by request_id.
    let mut transport = tarpc_fory::connect::<_, String, String>(addr, fory)
        .await
        .unwrap();

    // Send all 100 requests.
    for i in 0..N {
        let mut ctx = context::current();
        ctx.deadline = Instant::now() + Duration::from_secs(30);
        let req = ClientMessage::Request(Request {
            context: ctx,
            id: i,
            message: format!("user{}", i),
        });
        transport.send(req).await.unwrap();
    }

    // Drain all 100 responses (order is arbitrary due to server delays).
    let mut received: HashMap<u64, String> = HashMap::with_capacity(N as usize);
    for _ in 0..N {
        let resp = transport
            .next()
            .await
            .expect("stream closed early")
            .expect("transport error");
        let payload = resp.message.expect("server returned an error");
        received.insert(resp.request_id, payload);
    }

    // Verify every request round-tripped with the correct payload.
    assert_eq!(
        received.len(),
        N as usize,
        "expected {N} unique response IDs, got {}",
        received.len()
    );
    for i in 0..N {
        let got = received
            .get(&i)
            .unwrap_or_else(|| panic!("missing response for request_id={i}"));
        assert_eq!(
            got,
            &format!("echo:{}", i),
            "wrong payload for request_id={i}: {got:?}"
        );
    }
}
