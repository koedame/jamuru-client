//! Asking the router to open the audio port (UPnP IGD, ADR-113)
//!
//! A router that speaks UPnP is stood in for by a small server on loopback: it answers the
//! multicast search, serves its description and takes the SOAP requests that add and remove
//! a port forward. It can be told to refuse, to keep only mappings with no end, to forward the
//! requested port to someone else already, or to sit behind another NAT. The real thing
//! (miniupnpd forwarding packets) is `tests/upnp/run.sh`.

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use jamjam::network::port_mapping::{open_with, Options, PortMappingError};
use jamjam::network::{gather_candidates_mapped, AddressCandidate, CandidateType};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};

const MAGIC_COOKIE: u32 = 0x2112A442;

/// The address the fake router has on the internet
const OUTSIDE: &str = "203.0.113.9";

/// What the fake router does when asked to add a mapping
#[derive(Clone, Copy, PartialEq)]
enum Mood {
    Accepts,
    /// Error 501 for every action that changes anything
    Refuses,
    /// Error 725 unless the lease is 0
    KeepsOnlyPermanent,
    /// Error 718 for the ports in the list, which another machine has
    PortTaken,
    /// Its outside address is a home address: it is behind another router
    BehindAnotherNat,
}

#[derive(Debug, Clone, PartialEq)]
struct Forward {
    external_port: u16,
    internal: SocketAddr,
    lease_secs: u32,
}

struct FakeRouter {
    ssdp: SocketAddr,
    forwards: Mutex<HashMap<u16, Forward>>,
    adds: Mutex<Vec<Forward>>,
    taken: Mutex<Vec<u16>>,
    mood: Mood,
}

impl FakeRouter {
    async fn start(mood: Mood) -> Arc<Self> {
        let ssdp_socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let http = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let http_addr = http.local_addr().unwrap();
        let router = Arc::new(Self {
            ssdp: ssdp_socket.local_addr().unwrap(),
            forwards: Mutex::default(),
            adds: Mutex::default(),
            taken: Mutex::default(),
            mood,
        });

        tokio::spawn(async move {
            let mut buf = [0u8; 1500];
            while let Ok((len, from)) = ssdp_socket.recv_from(&mut buf).await {
                if !buf[..len].starts_with(b"M-SEARCH") {
                    continue;
                }
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nCACHE-CONTROL: max-age=120\r\nLOCATION: http://{http_addr}/rootDesc.xml\r\n\
                     ST: urn:schemas-upnp-org:device:InternetGatewayDevice:1\r\nUSN: uuid:fake::igd\r\n\r\n"
                );
                let _ = ssdp_socket.send_to(reply.as_bytes(), from).await;
            }
        });

        let serving = router.clone();
        tokio::spawn(async move {
            while let Ok((stream, _)) = http.accept().await {
                let serving = serving.clone();
                tokio::spawn(async move { serving.serve(stream).await });
            }
        });
        router
    }

    fn options(&self, lease_secs: u32) -> Options {
        let mut options = Options::default();
        options.search.broadcast_address = self.ssdp;
        // A search trusts the address its answer came from, and never loopback
        options.search.allowed_gateway_ips = vec![Ipv4Addr::LOCALHOST.into()];
        options.search.timeout = Some(Duration::from_millis(500));
        options.search.single_search_timeout = Some(Duration::from_millis(500));
        options.lease_secs = lease_secs;
        options
    }

    fn forwards(&self) -> Vec<Forward> {
        let mut forwards: Vec<_> = self.forwards.lock().unwrap().values().cloned().collect();
        forwards.sort_by_key(|f| f.external_port);
        forwards
    }

    fn adds(&self) -> Vec<Forward> {
        self.adds.lock().unwrap().clone()
    }

    async fn serve(&self, mut stream: TcpStream) {
        let mut request = Vec::new();
        let mut buf = [0u8; 2048];
        let (head_len, content_length) = loop {
            let n = stream.read(&mut buf).await.unwrap_or(0);
            if n == 0 {
                return;
            }
            request.extend_from_slice(&buf[..n]);
            if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                let head = String::from_utf8_lossy(&request[..end]).to_lowercase();
                let length = head
                    .lines()
                    .find_map(|l| l.strip_prefix("content-length:"))
                    .and_then(|v| v.trim().parse::<usize>().ok())
                    .unwrap_or(0);
                break (end + 4, length);
            }
        };
        while request.len() < head_len + content_length {
            let n = stream.read(&mut buf).await.unwrap_or(0);
            if n == 0 {
                return;
            }
            request.extend_from_slice(&buf[..n]);
        }
        let text = String::from_utf8_lossy(&request).to_string();
        let first_line = text.lines().next().unwrap_or_default().to_string();
        let body = &text[head_len..];

        let (status, xml) = if first_line.contains("/rootDesc.xml") {
            (200, ROOT_DESCRIPTION.to_string())
        } else if first_line.contains("/WANIPCn.xml") {
            (200, SERVICE_DESCRIPTION.to_string())
        } else if text.contains("#GetExternalIPAddress") {
            let outside = if self.mood == Mood::BehindAnotherNat {
                "192.168.100.2"
            } else {
                OUTSIDE
            };
            (
                200,
                soap_ok(
                    "GetExternalIPAddress",
                    &format!("<NewExternalIPAddress>{outside}</NewExternalIPAddress>"),
                ),
            )
        } else if text.contains("#AddPortMapping") {
            self.add(body)
        } else if text.contains("#DeletePortMapping") {
            let port = tag(body, "NewExternalPort").parse().unwrap();
            self.forwards.lock().unwrap().remove(&port);
            (200, soap_ok("DeletePortMapping", ""))
        } else {
            (404, String::new())
        };
        let reply = format!(
            "HTTP/1.1 {status} X\r\nContent-Type: text/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{xml}",
            xml.len()
        );
        let _ = stream.write_all(reply.as_bytes()).await;
        let _ = stream.shutdown().await;
    }

    fn add(&self, body: &str) -> (u16, String) {
        let forward = Forward {
            external_port: tag(body, "NewExternalPort").parse().unwrap(),
            internal: SocketAddr::new(
                tag(body, "NewInternalClient").parse().unwrap(),
                tag(body, "NewInternalPort").parse().unwrap(),
            ),
            lease_secs: tag(body, "NewLeaseDuration").parse().unwrap(),
        };
        self.adds.lock().unwrap().push(forward.clone());
        match self.mood {
            Mood::Refuses => return (500, soap_fault(501, "Action Failed")),
            Mood::KeepsOnlyPermanent if forward.lease_secs != 0 => {
                return (500, soap_fault(725, "OnlyPermanentLeasesSupported"))
            }
            Mood::PortTaken if self.taken.lock().unwrap().contains(&forward.external_port) => {
                return (500, soap_fault(718, "ConflictInMappingEntry"))
            }
            _ => {}
        }
        self.forwards
            .lock()
            .unwrap()
            .insert(forward.external_port, forward);
        (200, soap_ok("AddPortMapping", ""))
    }
}

fn tag(body: &str, name: &str) -> String {
    let open = format!("<{name}>");
    let start = body.find(&open).unwrap() + open.len();
    let end = body[start..].find('<').unwrap();
    body[start..start + end].to_string()
}

fn soap_ok(action: &str, inner: &str) -> String {
    format!(
        r#"<?xml version="1.0"?><s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/" s:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/"><s:Body><u:{action}Response xmlns:u="urn:schemas-upnp-org:service:WANIPConnection:1">{inner}</u:{action}Response></s:Body></s:Envelope>"#
    )
}

fn soap_fault(code: u16, description: &str) -> String {
    format!(
        r#"<?xml version="1.0"?><s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/" s:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/"><s:Body><s:Fault><faultcode>s:Client</faultcode><faultstring>UPnPError</faultstring><detail><UPnPError xmlns="urn:schemas-upnp-org:control-1-0"><errorCode>{code}</errorCode><errorDescription>{description}</errorDescription></UPnPError></detail></s:Fault></s:Body></s:Envelope>"#
    )
}

const ROOT_DESCRIPTION: &str = r#"<?xml version="1.0"?>
<root xmlns="urn:schemas-upnp-org:device-1-0"><specVersion><major>1</major><minor>0</minor></specVersion>
<device><deviceType>urn:schemas-upnp-org:device:InternetGatewayDevice:1</deviceType><friendlyName>Fake</friendlyName>
<deviceList><device><deviceType>urn:schemas-upnp-org:device:WANDevice:1</deviceType><friendlyName>WANDevice</friendlyName>
<deviceList><device><deviceType>urn:schemas-upnp-org:device:WANConnectionDevice:1</deviceType><friendlyName>WANConnectionDevice</friendlyName>
<serviceList><service><serviceType>urn:schemas-upnp-org:service:WANIPConnection:1</serviceType><serviceId>urn:upnp-org:serviceId:WANIPConn1</serviceId>
<controlURL>/ctl/IPConn</controlURL><eventSubURL>/evt/IPConn</eventSubURL><SCPDURL>/WANIPCn.xml</SCPDURL></service></serviceList>
</device></deviceList></device></deviceList></device></root>"#;

const SERVICE_DESCRIPTION: &str = r#"<?xml version="1.0"?>
<scpd xmlns="urn:schemas-upnp-org:service-1-0"><specVersion><major>1</major><minor>0</minor></specVersion>
<actionList>
<action><name>GetExternalIPAddress</name><argumentList><argument><name>NewExternalIPAddress</name><direction>out</direction></argument></argumentList></action>
<action><name>AddPortMapping</name><argumentList>
<argument><name>NewRemoteHost</name><direction>in</direction></argument>
<argument><name>NewExternalPort</name><direction>in</direction></argument>
<argument><name>NewProtocol</name><direction>in</direction></argument>
<argument><name>NewInternalPort</name><direction>in</direction></argument>
<argument><name>NewInternalClient</name><direction>in</direction></argument>
<argument><name>NewEnabled</name><direction>in</direction></argument>
<argument><name>NewPortMappingDescription</name><direction>in</direction></argument>
<argument><name>NewLeaseDuration</name><direction>in</direction></argument></argumentList></action>
<action><name>DeletePortMapping</name><argumentList>
<argument><name>NewRemoteHost</name><direction>in</direction></argument>
<argument><name>NewExternalPort</name><direction>in</direction></argument>
<argument><name>NewProtocol</name><direction>in</direction></argument></argumentList></action>
</actionList></scpd>"#;

/// Answers STUN binding requests with a fixed public address, whatever the sender
async fn spawn_stun_server(answer: SocketAddr) -> SocketAddr {
    let socket = UdpSocket::bind("127.0.0.3:0").await.unwrap();
    let addr = socket.local_addr().unwrap();
    tokio::spawn(async move {
        let mut buf = [0u8; 576];
        while let Ok((len, from)) = socket.recv_from(&mut buf).await {
            if len < 20 || buf[0..2] != [0x00, 0x01] {
                continue;
            }
            let SocketAddr::V4(answer) = answer else {
                continue;
            };
            let mut reply = Vec::with_capacity(32);
            reply.extend_from_slice(&0x0101u16.to_be_bytes());
            reply.extend_from_slice(&12u16.to_be_bytes());
            reply.extend_from_slice(&MAGIC_COOKIE.to_be_bytes());
            reply.extend_from_slice(&buf[8..20]);
            reply.extend_from_slice(&0x0020u16.to_be_bytes());
            reply.extend_from_slice(&8u16.to_be_bytes());
            reply.extend_from_slice(&[0x00, 0x01]);
            reply.extend_from_slice(&(answer.port() ^ (MAGIC_COOKIE >> 16) as u16).to_be_bytes());
            reply.extend_from_slice(&(u32::from(*answer.ip()) ^ MAGIC_COOKIE).to_be_bytes());
            let _ = socket.send_to(&reply, from).await;
        }
    });
    addr
}

async fn audio_socket() -> Arc<UdpSocket> {
    Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap())
}

fn outside(port: u16) -> SocketAddr {
    SocketAddr::new(OUTSIDE.parse().unwrap(), port)
}

fn reflexive(candidates: &[AddressCandidate]) -> Vec<SocketAddr> {
    candidates
        .iter()
        .filter(|c| c.candidate_type == CandidateType::ServerReflexive)
        .map(|c| c.address)
        .collect()
}

async fn eventually(what: &str, mut ok: impl FnMut() -> bool) {
    for _ in 0..100 {
        if ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("never happened: {what}");
}

/// Verifies: REQ-CON-165
#[tokio::test]
async fn router_forwards_the_port_then_peers_are_offered_its_public_address() {
    let router = FakeRouter::start(Mood::Accepts).await;
    let audio = audio_socket().await;
    let port = audio.local_addr().unwrap().port();

    let mapping = open_with(router.options(3600), port).await.unwrap();

    assert_eq!(mapping.external_addr(), outside(port));
    assert_eq!(
        router.forwards(),
        vec![Forward {
            external_port: port,
            internal: audio.local_addr().unwrap(),
            lease_secs: 3600,
        }]
    );
}

/// Verifies: REQ-CON-165
#[tokio::test]
async fn router_forwards_the_port_then_the_address_is_a_candidate_ahead_of_the_one_stun_saw() {
    let router = FakeRouter::start(Mood::Accepts).await;
    let audio = audio_socket().await;
    let port = audio.local_addr().unwrap().port();
    let stun_saw = "198.51.100.7:40000".parse().unwrap();
    let stun = spawn_stun_server(stun_saw).await;

    let options = router.options(3600);
    let gathered = gather_candidates_mapped(
        &audio,
        &[&stun.to_string()],
        Some(|p| open_with(options, p)),
    )
    .await;

    assert!(gathered.port_mapping.is_some());
    assert_eq!(
        reflexive(&gathered.candidates),
        vec![outside(port), stun_saw]
    );
}

/// Verifies: REQ-CON-165
#[tokio::test]
async fn router_already_gives_the_stun_address_then_it_is_offered_once() {
    let router = FakeRouter::start(Mood::Accepts).await;
    let audio = audio_socket().await;
    let port = audio.local_addr().unwrap().port();
    let stun = spawn_stun_server(outside(port)).await;

    let options = router.options(3600);
    let gathered = gather_candidates_mapped(
        &audio,
        &[&stun.to_string()],
        Some(|p| open_with(options, p)),
    )
    .await;

    assert_eq!(reflexive(&gathered.candidates), vec![outside(port)]);
}

/// Verifies: REQ-CON-165
#[tokio::test]
async fn mapping_is_closed_then_the_router_no_longer_forwards() {
    let router = FakeRouter::start(Mood::Accepts).await;
    let mapping = open_with(router.options(3600), 41001).await.unwrap();
    assert_eq!(router.forwards().len(), 1);

    mapping.close().await;

    assert!(router.forwards().is_empty());
}

/// Verifies: REQ-CON-165
#[tokio::test]
async fn mapping_is_dropped_then_the_router_stops_forwarding() {
    let router = FakeRouter::start(Mood::Accepts).await;
    let mapping = open_with(router.options(3600), 41002).await.unwrap();
    assert_eq!(router.forwards().len(), 1);

    drop(mapping);

    eventually("the router drops the forward", || {
        router.forwards().is_empty()
    })
    .await;
}

/// Verifies: REQ-CON-165
#[tokio::test]
async fn mapping_is_kept_then_it_is_renewed_before_the_lease_ends() {
    let router = FakeRouter::start(Mood::Accepts).await;
    let mapping = open_with(router.options(2), 41003).await.unwrap();

    eventually("the forward is added again", || router.adds().len() >= 3).await;

    assert!(router
        .adds()
        .iter()
        .all(|f| f.external_port == 41003 && f.lease_secs == 2));
    mapping.close().await;
}

/// Verifies: REQ-CON-165
#[tokio::test]
async fn router_keeps_only_permanent_mappings_then_none_is_made() {
    let router = FakeRouter::start(Mood::KeepsOnlyPermanent).await;

    let result = open_with(router.options(3600), 41004).await;

    assert!(
        matches!(result, Err(PortMappingError::Refused(_))),
        "{result:?}"
    );
    assert!(router.forwards().is_empty());
}

/// Verifies: REQ-CON-165
#[tokio::test]
async fn router_forwards_the_port_to_another_machine_then_another_port_is_used() {
    let router = FakeRouter::start(Mood::PortTaken).await;
    router.taken.lock().unwrap().push(41005);

    let mapping = open_with(router.options(3600), 41005).await.unwrap();

    let outside_port = mapping.external_addr().port();
    assert!((49152..49202).contains(&outside_port), "{outside_port}");
    assert_eq!(router.forwards()[0].internal.port(), 41005);
}

/// Verifies: REQ-CON-165
#[tokio::test]
async fn router_refuses_then_only_the_candidates_stun_gave_are_offered() {
    let router = FakeRouter::start(Mood::Refuses).await;
    let audio = audio_socket().await;
    let stun_saw = "198.51.100.7:40000".parse().unwrap();
    let stun = spawn_stun_server(stun_saw).await;

    let options = router.options(3600);
    let gathered = gather_candidates_mapped(
        &audio,
        &[&stun.to_string()],
        Some(|p| open_with(options, p)),
    )
    .await;

    assert!(gathered.port_mapping.is_none());
    assert_eq!(reflexive(&gathered.candidates), vec![stun_saw]);
    assert!(router.forwards().is_empty());
}

/// Verifies: REQ-CON-165
#[tokio::test]
async fn router_refuses_then_opening_says_it_was_refused() {
    let router = FakeRouter::start(Mood::Refuses).await;

    let result = open_with(router.options(3600), 41006).await;

    assert!(
        matches!(result, Err(PortMappingError::Refused(_))),
        "{result:?}"
    );
}

/// Verifies: REQ-CON-165
#[tokio::test]
async fn no_router_answers_then_only_the_candidates_stun_gave_are_offered() {
    let silent = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let mut options = Options::default();
    options.search.broadcast_address = silent.local_addr().unwrap();
    options.search.timeout = Some(Duration::from_millis(300));
    options.search.single_search_timeout = Some(Duration::from_millis(300));
    let audio = audio_socket().await;
    let stun_saw = "198.51.100.7:40000".parse().unwrap();
    let stun = spawn_stun_server(stun_saw).await;

    let gathered = gather_candidates_mapped(
        &audio,
        &[&stun.to_string()],
        Some(|p| open_with(options, p)),
    )
    .await;

    assert!(gathered.port_mapping.is_none());
    assert_eq!(reflexive(&gathered.candidates), vec![stun_saw]);
}

/// Verifies: REQ-CON-165
#[tokio::test]
async fn no_router_answers_then_opening_says_there_is_none() {
    let silent = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let mut options = Options::default();
    options.search.broadcast_address = silent.local_addr().unwrap();
    options.search.timeout = Some(Duration::from_millis(300));
    options.search.single_search_timeout = Some(Duration::from_millis(300));

    let result = open_with(options, 41007).await;

    assert!(
        matches!(result, Err(PortMappingError::NoRouter(_))),
        "{result:?}"
    );
}

/// Verifies: REQ-CON-165
#[tokio::test]
async fn router_is_behind_another_nat_then_nothing_is_forwarded() {
    let router = FakeRouter::start(Mood::BehindAnotherNat).await;

    let result = open_with(router.options(3600), 41008).await;

    assert!(
        matches!(result, Err(PortMappingError::NotPublic(_))),
        "{result:?}"
    );
    assert!(router.adds().is_empty());
}

/// Verifies: REQ-CON-165
#[tokio::test]
async fn the_router_is_not_asked_when_the_setting_is_off() {
    let router = FakeRouter::start(Mood::Accepts).await;
    let audio = audio_socket().await;

    let gathered =
        gather_candidates_mapped(
            &audio,
            &[],
            None::<
                fn(
                    u16,
                )
                    -> std::future::Ready<Result<jamjam::network::PortMapping, PortMappingError>>,
            >,
        )
        .await;

    assert!(gathered.port_mapping.is_none());
    assert!(router.adds().is_empty());
}
