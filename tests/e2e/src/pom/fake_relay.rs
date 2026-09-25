//! A stand-in for the server's relay, for scenarios about remote operation
//! (ADR-044).
//!
//! It answers the app's enrollment question and accepts its WebSocket, then
//! plays the operator: it sends requests and reads what comes back. It knows
//! only the wire protocol, not how the real server pairs or authenticates
//! anyone - that is the server's own tests. Everything is on loopback.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
use tokio_tungstenite::tungstenite::Message;

use super::driver::DriverResult;

#[derive(Default)]
struct Shared {
    enrollment_requests: AtomicUsize,
    connections: AtomicUsize,
    /// The `X-Device-Id` of each WebSocket handshake, in order.
    device_ids: Mutex<Vec<String>>,
    /// The connections waiting to be picked up by the scenario.
    targets: Mutex<Vec<Target>>,
}

/// A running fake relay.
pub struct FakeRelay {
    port: u16,
    shared: Arc<Shared>,
    /// Kept so the server stops with the scenario.
    _runtime: tokio::runtime::Runtime,
}

/// The app's end of a connection, seen from the operator's side.
pub struct Target {
    to_app: tokio::sync::mpsc::UnboundedSender<String>,
    from_app: Mutex<mpsc::Receiver<String>>,
}

impl FakeRelay {
    /// Starts a relay that says the app is (or is not) enrolled.
    pub fn start(enrolled: bool) -> DriverResult<Self> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .map_err(|e| format!("could not start the fake relay's runtime: {}", e))?;
        let listener = runtime
            .block_on(TcpListener::bind("127.0.0.1:0"))
            .map_err(|e| format!("could not bind the fake relay: {}", e))?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        let shared = Arc::new(Shared::default());

        let accepted = shared.clone();
        runtime.spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                tokio::spawn(serve(stream, port, enrolled, accepted.clone()));
            }
        });

        Ok(Self {
            port,
            shared,
            _runtime: runtime,
        })
    }

    /// The jamjam server URL to give the app.
    pub fn server_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// How many times the app has asked whether it is enrolled.
    pub fn enrollment_requests(&self) -> usize {
        self.shared.enrollment_requests.load(Ordering::SeqCst)
    }

    /// How many WebSocket connections the app has opened.
    pub fn connections(&self) -> usize {
        self.shared.connections.load(Ordering::SeqCst)
    }

    /// The `X-Device-Id` the app presented on each connection.
    pub fn device_ids(&self) -> Vec<String> {
        self.shared.device_ids.lock().unwrap().clone()
    }

    /// Waits for the app to connect, and returns its end of the connection.
    pub fn wait_for_target(&self, timeout: Duration) -> DriverResult<Target> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(target) = self.shared.targets.lock().unwrap().pop() {
                return Ok(target);
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "the app did not open a relay connection within {:?} (asked about \
                     enrollment {} times)",
                    timeout,
                    self.enrollment_requests()
                ));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Target {
    /// Sends one text frame to the app.
    pub fn send(&self, text: &str) {
        let _ = self.to_app.send(text.to_string());
    }

    /// The next frame from the app, as JSON.
    pub fn recv(&self, timeout: Duration) -> DriverResult<Value> {
        let text = self
            .from_app
            .lock()
            .unwrap()
            .recv_timeout(timeout)
            .map_err(|e| format!("no frame from the app within {:?}: {}", timeout, e))?;
        serde_json::from_str(&text).map_err(|e| format!("frame is not JSON ({}): {}", e, text))
    }

    /// Calls `method` and waits for its answer: `Ok` with the value, or `Err`
    /// with the error the app answered (`code: message`). Frames that are not
    /// this call's answer (events, another call's) are skipped.
    pub fn call(
        &self,
        id: u64,
        method: &str,
        params: Value,
    ) -> DriverResult<Result<Value, String>> {
        self.send(&serde_json::json!({ "id": id, "method": method, "params": params }).to_string());
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let frame = self.recv(left)?;
            if frame.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(error) = frame.get("error") {
                return Ok(Err(format!("{}: {}", error["code"], error["message"])));
            }
            return Ok(Ok(frame["ok"].clone()));
        }
    }

    /// Ends the connection from the relay's side.
    pub fn hang_up(self) {
        drop(self.to_app);
    }
}

/// One accepted TCP connection: the enrollment question or the relay's
/// WebSocket, told apart by the first line of the request.
async fn serve(stream: TcpStream, port: u16, enrolled: bool, shared: Arc<Shared>) {
    let mut head = [0u8; 2048];
    let Ok(read) = stream.peek(&mut head).await else {
        return;
    };
    let head = String::from_utf8_lossy(&head[..read]).to_string();

    if head.starts_with("GET /api/v1/remote/enrollment") {
        shared.enrollment_requests.fetch_add(1, Ordering::SeqCst);
        answer_enrollment(stream, port, enrolled).await;
    } else if head.starts_with("GET /v1/remote/target") {
        relay(stream, shared).await;
    }
}

async fn answer_enrollment(mut stream: TcpStream, port: u16, enrolled: bool) {
    // Read the request off the socket so the client sees a clean close.
    let mut request = [0u8; 4096];
    let _ = stream.read(&mut request).await;
    let body = serde_json::json!({
        "enrolled": enrolled,
        "url": format!("ws://127.0.0.1:{}/v1/remote/target", port),
    })
    .to_string();
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;
}

// The handshake callback's error type is tungstenite's, and large.
#[allow(clippy::result_large_err)]
async fn relay(stream: TcpStream, shared: Arc<Shared>) {
    let device_ids = Arc::new(Mutex::new(None::<String>));
    let seen = device_ids.clone();
    let accepted = tokio_tungstenite::accept_hdr_async(
        stream,
        move |request: &Request, response: Response| {
            let id = request
                .headers()
                .get("x-device-id")
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_string();
            *seen.lock().unwrap() = Some(id);
            Ok(response)
        },
    )
    .await;
    let Ok(socket) = accepted else {
        return;
    };
    shared.connections.fetch_add(1, Ordering::SeqCst);
    if let Some(id) = device_ids.lock().unwrap().take() {
        shared.device_ids.lock().unwrap().push(id);
    }

    let (mut sink, mut stream) = socket.split();
    let (to_app, mut outgoing) = tokio::sync::mpsc::unbounded_channel::<String>();
    let (to_scenario, from_app) = mpsc::channel::<String>();
    shared.targets.lock().unwrap().push(Target {
        to_app,
        from_app: Mutex::new(from_app),
    });

    // The relay tells both ends a pair has formed; the app answers with hello.
    let _ = sink
        .send(Message::Text(r#"{"relay":"paired"}"#.to_string().into()))
        .await;
    loop {
        tokio::select! {
            frame = stream.next() => match frame {
                Some(Ok(Message::Text(text))) => {
                    if to_scenario.send(text.to_string()).is_err() {
                        break;
                    }
                }
                Some(Ok(_)) => {}
                _ => break,
            },
            text = outgoing.recv() => match text {
                Some(text) => {
                    if sink.send(Message::Text(text.into())).await.is_err() {
                        break;
                    }
                }
                None => {
                    let _ = sink.close().await;
                    break;
                }
            },
        }
    }
}
