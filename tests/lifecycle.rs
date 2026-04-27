//! Lifecycle / connection-teardown tests.
//!
//! Scenario A — Server drops mid-call:
//!   Server accepts a connection, reads a Request, then drops the transport
//!   without sending a response.  The client's `transport.next()` should yield
//!   `None` or an `Err` (not hang or panic).
//!
//! Scenario B — Client drops mid-call:
//!   Client sends a Request then drops the transport.  When the server tries to
//!   `send()` the response, it should get `Err` rather than panic.

use fory::Fory;
use futures::{SinkExt as _, StreamExt as _};
use std::sync::Arc;
use std::time::Duration;
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

/// Scenario A: Server drops the transport after reading the request.
/// The client should see the stream end (None) or a transport error — not hang.
#[tokio::test]
async fn server_drops_after_read_client_gets_eof() {
    let fory = make_fory();

    let mut listener: tarpc_fory::Incoming<String, String> =
        tarpc_fory::listen("127.0.0.1:0", fory.clone()).await.unwrap();
    let addr = listener.local_addr();

    // Signal: server has read the request and is about to drop.
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel::<()>();

    tokio::spawn(async move {
        if let Some(Ok(mut transport)) = listener.next().await {
            // Read one request …
            if let Some(Ok(ClientMessage::Request(_))) = transport.next().await {
                // … then signal and drop without responding.
                let _ = ready_tx.send(());
                drop(transport); // explicit drop — connection closed here
            }
        }
    });

    let mut client_transport = tarpc_fory::connect::<_, String, String>(addr, fory)
        .await
        .unwrap();

    // Send request.
    client_transport
        .send(ClientMessage::Request(Request {
            context: context::current(),
            id: 1,
            message: "no reply expected".to_string(),
        }))
        .await
        .unwrap();

    // Wait until the server has read the request and closed the socket.
    tokio::time::timeout(Duration::from_secs(5), ready_rx)
        .await
        .expect("server never signalled")
        .unwrap();

    // Client should observe EOF or an error — it must not hang.
    let result = tokio::time::timeout(Duration::from_secs(5), client_transport.next())
        .await
        .expect("client transport hung after server drop");

    match result {
        None => { /* clean EOF: stream ended — acceptable */ }
        Some(Ok(_)) => panic!("client received a response despite server dropping the transport"),
        Some(Err(_)) => { /* transport error on EOF — acceptable */ }
    }
}

/// Scenario B: Client drops the transport; the server detects the disconnection.
///
/// After the client drops, the server tries to read another message from the
/// transport.  Because the underlying TCP connection is gone, that read must
/// return `None` or `Err` — it must not block indefinitely.
///
/// Note on `send` reliability: a single TCP `send()` can succeed even after
/// the remote end is closed if the data is still in the kernel send-buffer.
/// We therefore test the disconnect via a *subsequent read* on the server side,
/// which gives the OS time to propagate the FIN/RST and is the idiomatic way
/// to detect a closed peer in a stream-oriented protocol.
#[tokio::test]
async fn client_drops_server_detects_disconnect() {
    let fory = make_fory();

    let mut listener: tarpc_fory::Incoming<String, String> =
        tarpc_fory::listen("127.0.0.1:0", fory.clone()).await.unwrap();
    let addr = listener.local_addr();

    // Channel to signal that the client has dropped.
    let (client_dropped_tx, client_dropped_rx) = tokio::sync::oneshot::channel::<()>();
    // Channel so server can report whether it detected the disconnect.
    let (disconnect_tx, disconnect_rx) = tokio::sync::oneshot::channel::<bool>();

    tokio::spawn(async move {
        if let Some(Ok(mut transport)) = listener.next().await {
            // Read the request.
            if let Some(Ok(ClientMessage::Request(req))) = transport.next().await {
                // Wait until the client drops.
                let _ = client_dropped_rx.await;

                // Optionally send a response (may or may not succeed — that's fine).
                let resp = Response {
                    request_id: req.id,
                    message: Ok("too late".to_string()),
                };
                let _ = transport.send(resp).await;

                // Now try to read again — with the client gone, this must
                // return None (EOF) or Err (broken pipe), not block.
                let next = transport.next().await;
                let disconnected = match next {
                    None => true,       // clean EOF
                    Some(Ok(_)) => false, // still alive — unexpected but not panic
                    Some(Err(_)) => true, // transport error — disconnected
                };
                let _ = disconnect_tx.send(disconnected);
            }
        }
    });

    let mut client_transport = tarpc_fory::connect::<_, String, String>(addr, fory)
        .await
        .unwrap();

    // Send request.
    client_transport
        .send(ClientMessage::Request(Request {
            context: context::current(),
            id: 2,
            message: "drop me".to_string(),
        }))
        .await
        .unwrap();

    // Drop the client transport.
    drop(client_transport);

    // Signal the server that the client has dropped.
    let _ = client_dropped_tx.send(());

    // Server must detect the disconnect within 5 seconds.
    let disconnected = tokio::time::timeout(Duration::from_secs(5), disconnect_rx)
        .await
        .expect("server task did not complete within 5s")
        .expect("server task dropped sender");

    assert!(disconnected, "server did not detect client disconnect");
}
