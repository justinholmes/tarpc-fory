//! In-memory round-trip test for every io::ErrorKind through ServerError.
//!
//! Verifies the u32 ↔ io::ErrorKind mapping is bijective for all 18 documented
//! error kinds. No TCP transport involved — this purely exercises the
//! `ForyServerError` encoding and the `From` conversions.
//!
//! Algorithm per variant:
//!   native ServerError { kind, detail } → ForyServerError (via From)
//!   → fory bytes → ForyServerError (via deserialize) → ServerError (via Into)
//!   → assert kind and detail match

use fory::Fory;
use std::io;
use tarpc::ServerError;
use tarpc_fory::ForyServerError;

fn make_fory() -> Fory {
    let mut fory = Fory::default();
    fory.register_serializer::<ForyServerError>(3).unwrap();
    fory
}

/// All 18 error kinds from the documented wire table in `fory_envelope.rs`.
const ALL_KINDS: &[io::ErrorKind] = &[
    io::ErrorKind::NotFound,
    io::ErrorKind::PermissionDenied,
    io::ErrorKind::ConnectionRefused,
    io::ErrorKind::ConnectionReset,
    io::ErrorKind::ConnectionAborted,
    io::ErrorKind::NotConnected,
    io::ErrorKind::AddrInUse,
    io::ErrorKind::AddrNotAvailable,
    io::ErrorKind::BrokenPipe,
    io::ErrorKind::AlreadyExists,
    io::ErrorKind::WouldBlock,
    io::ErrorKind::InvalidInput,
    io::ErrorKind::InvalidData,
    io::ErrorKind::TimedOut,
    io::ErrorKind::WriteZero,
    io::ErrorKind::Interrupted,
    io::ErrorKind::Other,
    io::ErrorKind::UnexpectedEof,
];

#[test]
fn all_error_kinds_bijective_through_fory_encoding() {
    let fory = make_fory();

    for &kind in ALL_KINDS {
        let native = ServerError::new(kind, "test".to_string());

        // native → ForyServerError
        let wrapper = ForyServerError::from(&native);
        assert_eq!(wrapper.detail, "test", "detail lost for {kind:?}");

        // ForyServerError → fory bytes → ForyServerError
        let bytes = fory
            .serialize(&wrapper)
            .unwrap_or_else(|e| panic!("serialize failed for {kind:?}: {e:?}"));
        let decoded: ForyServerError = fory
            .deserialize(&bytes)
            .unwrap_or_else(|e| panic!("deserialize failed for {kind:?}: {e:?}"));

        // ForyServerError → native ServerError
        let round_tripped = ServerError::from(decoded);

        assert_eq!(round_tripped.kind, kind, "kind mismatch for {kind:?}");
        assert_eq!(round_tripped.detail, "test", "detail mismatch for {kind:?}");
    }
}

/// Verify the 18 documented u32 discriminants match the table exactly.
#[test]
fn error_kind_discriminants_match_table() {
    use tarpc::serde_transport::fory_envelope::error_kind_to_u32;

    let table: &[(io::ErrorKind, u32)] = &[
        (io::ErrorKind::NotFound, 0),
        (io::ErrorKind::PermissionDenied, 1),
        (io::ErrorKind::ConnectionRefused, 2),
        (io::ErrorKind::ConnectionReset, 3),
        (io::ErrorKind::ConnectionAborted, 4),
        (io::ErrorKind::NotConnected, 5),
        (io::ErrorKind::AddrInUse, 6),
        (io::ErrorKind::AddrNotAvailable, 7),
        (io::ErrorKind::BrokenPipe, 8),
        (io::ErrorKind::AlreadyExists, 9),
        (io::ErrorKind::WouldBlock, 10),
        (io::ErrorKind::InvalidInput, 11),
        (io::ErrorKind::InvalidData, 12),
        (io::ErrorKind::TimedOut, 13),
        (io::ErrorKind::WriteZero, 14),
        (io::ErrorKind::Interrupted, 15),
        (io::ErrorKind::Other, 16),
        (io::ErrorKind::UnexpectedEof, 17),
    ];

    for &(kind, expected) in table {
        let got = error_kind_to_u32(kind);
        assert_eq!(got, expected, "wrong u32 for {kind:?}: expected {expected}, got {got}");
    }

    // 18 unique discriminants — no duplicates.
    let discriminants: Vec<u32> = table.iter().map(|&(k, _)| error_kind_to_u32(k)).collect();
    let unique: std::collections::HashSet<u32> = discriminants.iter().copied().collect();
    assert_eq!(unique.len(), 18, "discriminants are not unique — bijection violated");
}

/// Unknown u32 values (>= 18) should decode as Other (the fallback).
#[test]
fn unknown_discriminant_decodes_as_other() {
    use tarpc::serde_transport::fory_envelope::u32_to_error_kind;

    for unknown in [18u32, 99, 255, 1000] {
        let kind = u32_to_error_kind(unknown);
        assert_eq!(
            kind,
            io::ErrorKind::Other,
            "u32={unknown} should map to Other, got {kind:?}"
        );
    }
}
