//! Dual-stack IPv4/IPv6 connectivity tests
//!
//! Tests for the Happy Eyeballs-style connection establishment
//! with multiple address candidates.

use std::net::SocketAddr;
use std::sync::Arc;

use jamjam::network::{
    candidates_to_addrs, gather_candidates_using, AddressCandidate, CandidateType, Connection,
    NetworkError, PeerInfo,
};
use uuid::Uuid;

/// Binds an IPv4 audio socket on an OS-assigned port and gathers its host
/// candidates. No STUN servers are given, so nothing leaves the machine.
async fn gather_host_candidates_of_new_socket() -> (u16, Vec<AddressCandidate>) {
    let socket = Arc::new(
        tokio::net::UdpSocket::bind("127.0.0.1:0")
            .await
            .expect("bind audio socket"),
    );
    let port = socket.local_addr().unwrap().port();
    (port, gather_candidates_using(&socket, &[]).await)
}

/// Test: every gathered candidate is well formed and ordered by priority
///
/// The list may be empty on a CI runner with no usable interface, so its length
/// is not asserted; every candidate that *is* returned must be connectable.
#[tokio::test]
async fn test_gathered_candidates_are_well_formed() {
    let (port, candidates) = gather_host_candidates_of_new_socket().await;

    for candidate in &candidates {
        // The socket's own port must be carried through.
        assert_eq!(
            candidate.address.port(),
            port,
            "candidate {:?} does not carry the socket's port",
            candidate
        );
        assert!(
            candidate.priority > 0,
            "candidate {:?} has no priority and cannot be ordered",
            candidate
        );
    }

    // Candidates are handed to the connection logic in preference order.
    for pair in candidates.windows(2) {
        assert!(
            pair[0].priority >= pair[1].priority,
            "candidates must be sorted by descending priority, got {:?} before {:?}",
            pair[0],
            pair[1]
        );
    }
}

/// Test: an IPv4 audio socket is offered IPv4 candidates only
///
/// The socket cannot send to or receive from an IPv6 address, so advertising
/// one hands the peer a path that never works.
#[tokio::test]
async fn test_gather_candidates_match_socket_family() {
    let (_, candidates) = gather_host_candidates_of_new_socket().await;

    for c in &candidates {
        assert!(c.address.is_ipv4(), "IPv4 socket was offered {:?}", c);
    }
}

/// Test: AddressCandidate host priority is higher than server reflexive
#[test]
fn test_candidate_priority_host_over_srflx() {
    let host = AddressCandidate::host("192.168.1.100:5000".parse().unwrap());
    let srflx = AddressCandidate::server_reflexive("203.0.113.50:5000".parse().unwrap());

    assert!(
        host.priority > srflx.priority,
        "Host candidate should have higher priority than server reflexive"
    );
}

/// Test: IPv6 candidates have slightly higher priority than IPv4
#[test]
fn test_candidate_priority_ipv6_over_ipv4() {
    let ipv4 = AddressCandidate::host("192.168.1.100:5000".parse().unwrap());
    let ipv6 = AddressCandidate::host("[::1]:5000".parse().unwrap());

    assert!(
        ipv6.priority > ipv4.priority,
        "IPv6 should have slightly higher priority than IPv4"
    );
}

/// Test: candidates_to_addrs preserves order
#[test]
fn test_candidates_to_addrs_order() {
    let candidates = vec![
        AddressCandidate::host("192.168.1.1:5000".parse().unwrap()),
        AddressCandidate::host("192.168.1.2:5000".parse().unwrap()),
        AddressCandidate::server_reflexive("203.0.113.1:5000".parse().unwrap()),
    ];

    let addrs = candidates_to_addrs(&candidates);

    assert_eq!(addrs.len(), 3);
    assert_eq!(addrs[0], "192.168.1.1:5000".parse::<SocketAddr>().unwrap());
    assert_eq!(addrs[1], "192.168.1.2:5000".parse::<SocketAddr>().unwrap());
    assert_eq!(addrs[2], "203.0.113.1:5000".parse::<SocketAddr>().unwrap());
}

/// Test: connect_with_candidates fails gracefully with empty list
#[tokio::test]
async fn test_connect_with_candidates_empty_fails() {
    let mut conn = Connection::new("127.0.0.1:0")
        .await
        .expect("Failed to create connection");

    let result = conn.connect_with_candidates(&[]).await;

    assert!(result.is_err());
    match result {
        Err(NetworkError::NoCandidates) => {}
        other => panic!("Expected NoCandidates error, got: {:?}", other),
    }
}

/// Test: connect_with_candidates succeeds with valid loopback address
#[tokio::test]
async fn test_connect_with_candidates_single_loopback() {
    let mut conn1 = Connection::new("127.0.0.1:0")
        .await
        .expect("Failed to create connection 1");
    let conn2 = Connection::new("127.0.0.1:0")
        .await
        .expect("Failed to create connection 2");

    let candidates = vec![conn2.local_addr()];
    let result = conn1.connect_with_candidates(&candidates).await;

    assert!(result.is_ok(), "Should connect to single valid candidate");
    assert!(conn1.is_connected());
}

/// Test: connect_with_candidates selects working candidate from multiple options
#[tokio::test]
async fn test_connect_with_candidates_selects_working() {
    let mut conn1 = Connection::new("127.0.0.1:0")
        .await
        .expect("Failed to create connection 1");
    let conn2 = Connection::new("127.0.0.1:0")
        .await
        .expect("Failed to create connection 2");

    // Mix of invalid and valid candidates
    let candidates = vec![
        "127.0.0.1:59998".parse().unwrap(), // Invalid - no listener
        "127.0.0.1:59999".parse().unwrap(), // Invalid - no listener
        conn2.local_addr(),                 // Valid - will respond
    ];

    let result = conn1.connect_with_candidates(&candidates).await;

    assert!(
        result.is_ok(),
        "Should find and connect to working candidate"
    );
    assert!(conn1.is_connected());
}

/// Test: PeerInfo backward compatibility - old format without candidates
#[test]
fn test_peer_info_backward_compat_no_candidates() {
    // Simulate old PeerInfo JSON without candidates field
    let json = r#"{
        "id": "550e8400-e29b-41d4-a716-446655440000",
        "name": "LegacyPeer",
        "public_addr": "192.168.1.100:5000",
        "local_addr": "192.168.1.100:5000"
    }"#;

    let peer: PeerInfo = serde_json::from_str(json).expect("Should parse legacy format");

    assert_eq!(peer.name, "LegacyPeer");
    assert!(
        peer.candidates.is_empty(),
        "Candidates should default to empty"
    );
    assert!(
        peer.public_addr.is_some(),
        "Legacy public_addr should be present"
    );
}

/// Test: PeerInfo with candidates serialization roundtrip
#[test]
fn test_peer_info_with_candidates_roundtrip() {
    let original = PeerInfo {
        id: Uuid::new_v4(),
        name: "NewPeer".to_string(),
        candidates: vec![
            AddressCandidate::host("192.168.1.100:5000".parse().unwrap()),
            AddressCandidate::server_reflexive("203.0.113.50:5000".parse().unwrap()),
        ],
        public_addr: Some("203.0.113.50:5000".parse().unwrap()),
        local_addr: Some("192.168.1.100:5000".parse().unwrap()),
        joined_at: 0,
        // Never crosses the wire (skip_serializing, ADR-024) - a peer's
        // device identifier is not shared with other participants.
        features: vec![],
    };

    let json = serde_json::to_string(&original).expect("Should serialize");
    let deserialized: PeerInfo = serde_json::from_str(&json).expect("Should deserialize");

    assert_eq!(deserialized.name, original.name);
    assert_eq!(deserialized.candidates.len(), 2);
    assert_eq!(
        deserialized.candidates[0].candidate_type,
        CandidateType::Host
    );
    assert_eq!(
        deserialized.candidates[1].candidate_type,
        CandidateType::ServerReflexive
    );
}

/// Test: Mixed IPv4/IPv6 candidate list sorting
#[test]
fn test_mixed_ipv4_ipv6_candidates_sorting() {
    let mut candidates = [
        AddressCandidate::server_reflexive("203.0.113.50:5000".parse().unwrap()),
        AddressCandidate::host("192.168.1.100:5000".parse().unwrap()),
        AddressCandidate::host("[::1]:5000".parse().unwrap()),
        AddressCandidate::server_reflexive("[2001:db8::1]:5000".parse().unwrap()),
    ];

    // Sort by priority (highest first)
    candidates.sort_by_key(|c| std::cmp::Reverse(c.priority));

    // Expected order: IPv6 Host > IPv4 Host > IPv6 SRFLX > IPv4 SRFLX
    assert_eq!(candidates[0].candidate_type, CandidateType::Host);
    assert!(candidates[0].address.is_ipv6());

    assert_eq!(candidates[1].candidate_type, CandidateType::Host);
    assert!(candidates[1].address.is_ipv4());

    assert_eq!(candidates[2].candidate_type, CandidateType::ServerReflexive);
    assert!(candidates[2].address.is_ipv6());

    assert_eq!(candidates[3].candidate_type, CandidateType::ServerReflexive);
    assert!(candidates[3].address.is_ipv4());
}

/// Test: Connection state during connect_with_candidates
#[tokio::test]
async fn test_connect_with_candidates_state_transitions() {
    let mut conn1 = Connection::new("127.0.0.1:0")
        .await
        .expect("Failed to create connection");
    let conn2 = Connection::new("127.0.0.1:0")
        .await
        .expect("Failed to create connection");

    // Initial state
    assert!(!conn1.is_connected());

    // Connect
    let candidates = vec![conn2.local_addr()];
    conn1
        .connect_with_candidates(&candidates)
        .await
        .expect("Should connect");

    // Final state
    assert!(conn1.is_connected());
}

/// Test: Preventing double connection with connect_with_candidates
#[tokio::test]
async fn test_connect_with_candidates_prevents_double_connect() {
    let mut conn1 = Connection::new("127.0.0.1:0")
        .await
        .expect("Failed to create connection");
    let conn2 = Connection::new("127.0.0.1:0")
        .await
        .expect("Failed to create connection");

    let candidates = vec![conn2.local_addr()];

    // First connection should succeed
    conn1
        .connect_with_candidates(&candidates)
        .await
        .expect("First connect should succeed");

    // Second connection should fail with AlreadyConnected
    let result = conn1.connect_with_candidates(&candidates).await;
    assert!(result.is_err());
    match result {
        Err(NetworkError::AlreadyConnected) => {}
        other => panic!("Expected AlreadyConnected error, got: {:?}", other),
    }
}
