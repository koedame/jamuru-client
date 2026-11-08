//! Each way of the round trip is timed by the clocks of both ends, over a real encrypted link
//! (REQ-LAT-133).

mod common;

use std::time::Duration;

use jamjam::network::{Connection, ConnectionState, ReconnectConfig};

const CONFIG: ReconnectConfig = ReconnectConfig {
    keep_alive_interval: Duration::from_millis(100),
    detect_after: Duration::from_millis(300),
    give_up_after: Duration::from_secs(1),
    check_interval: Duration::from_millis(50),
};

/// Both ends of a link on one machine share a clock, so the two ways come out as the real
/// ones: each is tiny and the two add up to the round trip.
///
/// Verifies: REQ-LAT-133
#[tokio::test]
async fn when_two_ends_share_a_clock_the_two_ways_add_up_to_the_round_trip() {
    let mut a = Connection::new("127.0.0.1:0").await.unwrap();
    let mut b = Connection::new("127.0.0.1:0").await.unwrap();
    common::trust_each_other(&mut a, &mut b);
    a.set_reconnect_config(CONFIG);
    b.set_reconnect_config(CONFIG);
    let (to_a, to_b) = (a.local_addr(), b.local_addr());
    let (ra, rb) = tokio::join!(a.connect(to_b), b.connect(to_a));
    ra.unwrap();
    rb.unwrap();
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(a.state(), ConnectionState::Connected);

    for end in [&a, &b] {
        let stats = end.stats();
        let rtt = stats.rtt_ms.expect("pings were answered");
        let one_way = stats.one_way.expect("the answers carried the clock times");
        assert!(
            (one_way.up_ms + one_way.down_ms - rtt).abs() < 1.0,
            "up {} + down {} should be the round trip {}",
            one_way.up_ms,
            one_way.down_ms,
            rtt
        );
        assert!(
            one_way.up_ms.abs() < 5.0 && one_way.down_ms.abs() < 5.0,
            "{one_way:?}"
        );
    }
}
