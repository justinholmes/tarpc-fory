use fory::Fory;
use futures::{SinkExt, StreamExt};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tarpc::{context, ClientMessage, Request, Response};

fn build_fory() -> Arc<Fory> {
    let mut fory = Fory::default();
    // Vec<u8> is a fory builtin (Vec is a builtin container; u8 is primitive)
    fory.register::<tarpc_fory::ForyTraceContext>(2).unwrap();
    fory.register::<tarpc_fory::ForyServerError>(3).unwrap();
    fory.register::<tarpc_fory::ForyResult<Vec<u8>>>(4).unwrap();
    fory.register::<tarpc_fory::ForyRequest<Vec<u8>>>(5).unwrap();
    fory.register::<tarpc_fory::ForyResponse<Vec<u8>>>(6).unwrap();
    fory.register::<tarpc_fory::ForyClientMessage<Vec<u8>>>(7).unwrap();
    Arc::new(fory)
}

#[tokio::test]
async fn sixteen_mib_payload_round_trip() {
    let fory = build_fory();
    let payload = vec![0xABu8; 16 * 1024 * 1024];
    let payload_len = payload.len();

    // Server: echo Vec<u8> back as-is
    let mut listener: tarpc_fory::Incoming<Vec<u8>, Vec<u8>> =
        tarpc_fory::listen("127.0.0.1:0", fory.clone()).await.unwrap();
    let addr = listener.local_addr();

    let server = tokio::spawn(async move {
        let mut transport = listener.next().await.unwrap().unwrap();
        if let Some(Ok(ClientMessage::Request(req))) = transport.next().await {
            let resp = Response {
                request_id: req.id,
                message: Ok(req.message),
            };
            transport.send(resp).await.unwrap();
        }
    });

    // Client
    let mut transport = tarpc_fory::connect::<_, Vec<u8>, Vec<u8>>(addr, fory)
        .await
        .unwrap();

    let mut ctx = context::current();
    ctx.deadline = Instant::now() + Duration::from_secs(60);

    let req = ClientMessage::Request(Request {
        context: ctx,
        id: 1,
        message: payload.clone(),
    });

    transport.send(req).await.unwrap();
    let resp = transport.next().await.unwrap().unwrap();

    let bytes = resp.message.unwrap();
    assert_eq!(bytes.len(), payload_len, "length mismatch");
    assert_eq!(bytes[0], 0xAB, "first byte mismatch");
    assert_eq!(bytes[bytes.len() - 1], 0xAB, "last byte mismatch");
    // Spot check middle to detect interior corruption
    let mid = bytes.len() / 2;
    assert_eq!(bytes[mid], 0xAB, "middle byte mismatch");

    server.await.unwrap();
}
