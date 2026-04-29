//! Custom user-type serialization tests.
//!
//! Exercises `#[derive(fory::ForyObject)]` on user-defined structs and enums,
//! verifying that complex nested types, `Option<T>`, `Vec<T>`, tuple structs,
//! and C-style enums all survive a fory serialize → deserialize round-trip.
//!
//! ## Why in-memory only?
//!
//! The `ForyObject` derive macro assigns each derived type a compile-time index
//! starting from 0 within the crate where the derive is used.  The tarpc lib
//! crate's `ForyTraceContext` also starts at index 0.  Mixing user-derived
//! types with the full transport (which registers `ForyTraceContext` at explicit
//! ID 2) would require the user types to occupy the same compile-time indices,
//! causing a registration collision at runtime.
//!
//! The solution is to test custom type serialization in isolation (no TCP
//! transport) with their own fresh `Fory::default()` registries, then exercise
//! the transport separately using built-in types (`String`, `Vec<u8>`, `u32`)
//! that do not occupy the derive-macro index table.
//!
//! The transport round-trips below use `String` as `T` and carry a
//! JSON-like serialized representation of the custom type as a string payload,
//! proving the full pipeline works without triggering the index collision.

use fory::Fory;
use fory::ForyObject;
use futures::{SinkExt as _, StreamExt as _};
use std::sync::Arc;
use std::time::Duration;
use tarpc::{ClientMessage, Request, Response, context};
use tarpc_fory::{
    ForyClientMessage, ForyRequest, ForyResponse, ForyServerError, ForyTraceContext,
};

// ---------------------------------------------------------------------------
// User-defined types
// ---------------------------------------------------------------------------

/// Plain struct with primitive and String fields.
#[derive(Debug, Clone, PartialEq, ForyObject)]
struct Point {
    x: i32,
    y: i32,
    label: String,
}

/// Nested struct.
#[derive(Debug, Clone, PartialEq, ForyObject)]
struct Segment {
    start: Point,
    end: Point,
    weight: f64,
}

/// Struct with `Vec<Point>` and `Option<String>`.
#[derive(Debug, Clone, PartialEq, ForyObject)]
struct PointCloud {
    points: Vec<Point>,
    name: Option<String>,
    version: u32,
}

/// Tuple struct.
#[derive(Debug, Clone, PartialEq, ForyObject)]
struct Color(u8, u8, u8);

/// C-style enum.
#[derive(Debug, Clone, PartialEq, ForyObject)]
enum Direction {
    North,
    South,
    East,
    West,
}

// ---------------------------------------------------------------------------
// In-memory round-trip tests (no transport)
// ---------------------------------------------------------------------------

/// Register all user types in a fresh Fory instance.
/// Uses IDs 100..=104 to avoid any collision with the transport envelope
/// registration range (2..=7) even if later combined.
fn make_user_fory() -> Fory {
    let mut fory = Fory::default();
    fory.register::<Point>(100).unwrap();
    fory.register::<Segment>(101).unwrap();
    fory.register::<PointCloud>(102).unwrap();
    fory.register::<Color>(103).unwrap();
    fory.register::<Direction>(104).unwrap();
    fory
}

#[test]
fn point_round_trip() {
    let fory = make_user_fory();
    let original = Point { x: 42, y: -7, label: "origin".to_string() };
    let bytes = fory.serialize(&original).unwrap();
    let decoded: Point = fory.deserialize(&bytes).unwrap();
    assert_eq!(decoded, original);
}

#[test]
fn segment_with_nested_structs_round_trip() {
    let fory = make_user_fory();
    let original = Segment {
        start: Point { x: 0, y: 0, label: "A".to_string() },
        end: Point { x: 100, y: 200, label: "B".to_string() },
        weight: 1.234_567_89,
    };
    let bytes = fory.serialize(&original).unwrap();
    let decoded: Segment = fory.deserialize(&bytes).unwrap();
    assert_eq!(decoded.start, original.start);
    assert_eq!(decoded.end, original.end);
    // f64 comparison with tolerance.
    assert!((decoded.weight - original.weight).abs() < 1e-9, "weight mismatch");
}

#[test]
fn point_cloud_with_vec_and_option_some_round_trip() {
    let fory = make_user_fory();
    let original = PointCloud {
        points: vec![
            Point { x: 1, y: 2, label: "p1".to_string() },
            Point { x: 3, y: 4, label: "p2".to_string() },
            Point { x: 5, y: 6, label: "p3".to_string() },
        ],
        name: Some("my cloud".to_string()),
        version: 7,
    };
    let bytes = fory.serialize(&original).unwrap();
    let decoded: PointCloud = fory.deserialize(&bytes).unwrap();
    assert_eq!(decoded.points.len(), 3, "wrong number of points");
    assert_eq!(decoded.points[0].x, 1);
    assert_eq!(decoded.points[2].label, "p3");
    assert_eq!(decoded.name, Some("my cloud".to_string()));
    assert_eq!(decoded.version, 7);
}

#[test]
fn point_cloud_with_option_none_round_trip() {
    let fory = make_user_fory();
    let original = PointCloud {
        points: vec![],
        name: None,
        version: 0,
    };
    let bytes = fory.serialize(&original).unwrap();
    let decoded: PointCloud = fory.deserialize(&bytes).unwrap();
    assert_eq!(decoded.name, None);
    assert!(decoded.points.is_empty());
}

#[test]
fn tuple_struct_round_trip() {
    let fory = make_user_fory();
    let original = Color(255, 128, 0);
    let bytes = fory.serialize(&original).unwrap();
    let decoded: Color = fory.deserialize(&bytes).unwrap();
    assert_eq!(decoded, original);
}

#[test]
fn c_style_enum_all_variants_round_trip() {
    let fory = make_user_fory();
    for variant in [Direction::North, Direction::South, Direction::East, Direction::West] {
        let bytes = fory.serialize(&variant).unwrap();
        let decoded: Direction = fory.deserialize(&bytes).unwrap();
        assert_eq!(decoded, variant, "Direction::{variant:?} mismatch");
    }
}

// ---------------------------------------------------------------------------
// Transport round-trips using built-in String as T.
//
// We encode the custom-type data into a String payload (using Display-like
// formatting), send it through the real TCP transport, and decode it on the
// other side — proving the full pipeline works for arbitrary string-encoded
// data.
// ---------------------------------------------------------------------------

fn make_transport_fory() -> Arc<Fory> {
    let mut fory = Fory::default();
    fory.register_serializer::<ForyTraceContext>(2).unwrap();
    fory.register_serializer::<ForyServerError>(3).unwrap();
    fory.register_serializer::<ForyRequest<String>>(5).unwrap();
    fory.register_serializer::<ForyResponse<String>>(6).unwrap();
    fory.register_serializer::<ForyClientMessage<String>>(7).unwrap();
    Arc::new(fory)
}

/// Transport round-trip: custom struct data encoded as a string.
#[tokio::test]
async fn custom_struct_data_via_string_transport() {
    let fory = make_transport_fory();

    let mut listener: tarpc_fory::Incoming<String, String> =
        tarpc_fory::listen_with_fory("127.0.0.1:0", fory.clone()).await.unwrap();
    let addr = listener.local_addr();

    tokio::spawn(async move {
        if let Some(Ok(mut transport)) = listener.next().await {
            if let Some(Ok(ClientMessage::Request(req))) = transport.next().await {
                let resp = Response {
                    request_id: req.id,
                    message: Ok(format!("received:{}", req.message)),
                };
                transport.send(resp).await.unwrap();
            }
        }
    });

    // Encode a custom Point as a string and send it through the transport.
    let point = Point { x: 42, y: -7, label: "test".to_string() };
    let encoded = format!("{}:{}:{}", point.x, point.y, point.label);

    let mut transport = tarpc_fory::connect_with_fory::<_, String, String>(addr, fory).await.unwrap();
    transport
        .send(ClientMessage::Request(Request {
            context: context::current(),
            id: 1,
            message: encoded.clone(),
        }))
        .await
        .unwrap();

    let resp = tokio::time::timeout(Duration::from_secs(5), transport.next())
        .await
        .expect("timeout")
        .expect("stream ended")
        .expect("transport error");

    assert_eq!(resp.request_id, 1);
    let body = resp.message.unwrap();
    assert_eq!(body, format!("received:{}", encoded));
}

/// Transport round-trip: Vec of custom struct data encoded as a string.
#[tokio::test]
async fn custom_vec_data_via_string_transport() {
    let fory = make_transport_fory();

    let mut listener: tarpc_fory::Incoming<String, String> =
        tarpc_fory::listen_with_fory("127.0.0.1:0", fory.clone()).await.unwrap();
    let addr = listener.local_addr();

    tokio::spawn(async move {
        if let Some(Ok(mut transport)) = listener.next().await {
            if let Some(Ok(ClientMessage::Request(req))) = transport.next().await {
                // Parse out point count from the payload prefix "N:..."
                let count: usize = req.message.split(':').next().unwrap().parse().unwrap();
                let resp = Response {
                    request_id: req.id,
                    message: Ok(format!("count:{}", count)),
                };
                transport.send(resp).await.unwrap();
            }
        }
    });

    let points: Vec<Point> = (0..5)
        .map(|i| Point { x: i, y: i * 2, label: format!("p{}", i) })
        .collect();
    // Encode as "N:x0,y0,label0;x1,y1,label1;..."
    let encoded = format!(
        "{}:{}",
        points.len(),
        points
            .iter()
            .map(|p| format!("{},{},{}", p.x, p.y, p.label))
            .collect::<Vec<_>>()
            .join(";")
    );

    let mut transport = tarpc_fory::connect_with_fory::<_, String, String>(addr, fory).await.unwrap();
    transport
        .send(ClientMessage::Request(Request {
            context: context::current(),
            id: 2,
            message: encoded,
        }))
        .await
        .unwrap();

    let resp = tokio::time::timeout(Duration::from_secs(5), transport.next())
        .await
        .expect("timeout")
        .expect("stream ended")
        .expect("transport error");

    assert_eq!(resp.request_id, 2);
    let body = resp.message.unwrap();
    assert_eq!(body, "count:5");
}
