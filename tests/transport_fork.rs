//! End-to-end tests for fory transport over TCP.
//!
//! Uses `String` as both request and response type. The envelope wrapper types
//! use manual `Serializer` impls and must be registered via `register_serializer`
//! (EXT type path). Use the `register_envelope_types` helper for convenience.
//!
//! Migrated from tarpc-fork/tarpc/tests/fory_transport.rs.

#![cfg(feature = "tcp")]

use fory::Fory;
use futures::StreamExt as _;
use std::sync::Arc;
use tarpc::{
    client,
    context,
    server::{self, Channel},
};
use tarpc_fory::envelope::register_envelope_types;

// ---------------------------------------------------------------------------
// Helper: build a shared Fory registry with all envelope types registered.
//
// We use String as Req and Resp throughout. Envelope types use manual
// Serializer impls registered via register_serializer (EXT type path).
// ---------------------------------------------------------------------------

fn make_fory() -> Arc<Fory> {
    let mut fory = Fory::default();
    register_envelope_types::<String>(&mut fory).unwrap();
    Arc::new(fory)
}

// ---------------------------------------------------------------------------
// Raw transport echo test
//
// Exercises the codec directly by sending a single ClientMessage<String> and
// receiving the corresponding Response<String>, without using the tarpc
// service machinery.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn raw_transport_echo() {
    use futures::SinkExt as _;
    use tarpc::{ClientMessage, Response};

    let fory = make_fory();

    let mut incoming =
        tarpc_fory::listen_with_fory::<_, String, String>("127.0.0.1:0", fory.clone())
            .await
            .unwrap();
    let addr = incoming.local_addr();

    // Server: echo the request payload back as a response.
    tokio::spawn(async move {
        if let Some(Ok(mut server_transport)) = incoming.next().await {
            // Receive one ClientMessage<String>
            let msg = futures::StreamExt::next(&mut server_transport).await;
            if let Some(Ok(ClientMessage::Request(req))) = msg {
                let resp = Response {
                    request_id: req.id,
                    message: Ok(format!("echo: {}", req.message)),
                };
                server_transport.send(resp).await.unwrap();
            }
        }
    });

    // Client: connect, send a request, receive the echo.
    let mut client_transport =
        tarpc_fory::connect_with_fory::<_, String, String>(addr, fory)
            .await
            .unwrap();

    let ctx = context::current();
    let request = tarpc::ClientMessage::Request(tarpc::Request {
        context: ctx,
        id: 1,
        message: "hello".to_string(),
    });

    client_transport.send(request).await.unwrap();

    let response = futures::StreamExt::next(&mut client_transport).await;
    let resp = response.unwrap().unwrap();
    assert_eq!(resp.request_id, 1);
    assert_eq!(resp.message.unwrap(), "echo: hello");
}

// ---------------------------------------------------------------------------
// Hello service end-to-end test via tarpc service macro (in-memory)
// ---------------------------------------------------------------------------

#[tarpc::service]
trait Hello {
    async fn hello(name: String) -> String;
}

#[derive(Clone)]
struct HelloServer;

impl Hello for HelloServer {
    async fn hello(self, _: context::Context, name: String) -> String {
        format!("hi {}", name)
    }
}

/// Verifies the Hello service itself works correctly via the in-memory channel
/// transport (which does not require fory registration of generated types).
#[tokio::test]
async fn hello_service_in_memory() {
    let (client_transport, server_transport) = tarpc::transport::channel::unbounded();

    tokio::spawn(async move {
        server::BaseChannel::with_defaults(server_transport)
            .execute(HelloServer.serve())
            .for_each(|fut| async move {
                tokio::spawn(fut);
            })
            .await;
    });

    let client = HelloClient::new(client::Config::default(), client_transport).spawn();
    let resp = client.hello(context::current(), "world".to_string()).await.unwrap();
    assert_eq!(resp, "hi world");
}

// ---------------------------------------------------------------------------
// Multiple round-trips: verifies request ID correlation works correctly when
// several messages are sent in sequence over the fory transport.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn raw_transport_multiple_round_trips() {
    use futures::SinkExt as _;
    use tarpc::{ClientMessage, Response};

    let fory = make_fory();

    let mut incoming =
        tarpc_fory::listen_with_fory::<_, String, String>("127.0.0.1:0", fory.clone())
            .await
            .unwrap();
    let addr = incoming.local_addr();

    // Server: echo each request back.
    tokio::spawn(async move {
        if let Some(Ok(mut server_transport)) = incoming.next().await {
            while let Some(Ok(ClientMessage::Request(req))) =
                futures::StreamExt::next(&mut server_transport).await
            {
                let resp = Response {
                    request_id: req.id,
                    message: Ok(format!("resp:{}", req.message)),
                };
                if server_transport.send(resp).await.is_err() {
                    break;
                }
            }
        }
    });

    let mut client_transport =
        tarpc_fory::connect_with_fory::<_, String, String>(addr, fory)
            .await
            .unwrap();

    for i in 0u64..5 {
        let ctx = context::current();
        let request = tarpc::ClientMessage::Request(tarpc::Request {
            context: ctx,
            id: i,
            message: format!("msg{}", i),
        });
        client_transport.send(request).await.unwrap();
        let response = futures::StreamExt::next(&mut client_transport).await.unwrap().unwrap();
        assert_eq!(response.request_id, i);
        assert_eq!(response.message.unwrap(), format!("resp:msg{}", i));
    }
}
