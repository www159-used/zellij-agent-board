//! HTTP/1 over private Unix sockets. Hyper owns framing and parsing.
//! A bounded queue connects async HTTP I/O to the single database owner.
use std::cell::RefCell;
use std::fs;
use std::io;
use std::os::unix::net::UnixListener;
use std::path::Path;
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread;
use std::time::Duration;

use http_body_util::{BodyExt, Full, Limited};
use hyper::body::{Bytes, Incoming};
use hyper::header::{ALLOW, CONTENT_TYPE};
use hyper::service::service_fn;
use hyper::{Method, Request as HttpRequest, Response, StatusCode};
use hyper_util::rt::TokioIo;
use serde::Deserialize;
use tokio::runtime::{Builder, Runtime};
use tokio::sync::oneshot;
use tokio::task::JoinSet;

use crate::daemon::Request;
use crate::database::Snapshot;

const REQUEST_LIMIT: usize = 64 * 1024;
const RESPONSE_LIMIT: usize = 16 * 1024 * 1024;
const CLIENT_TIMEOUT: Duration = Duration::from_millis(500);
const CONNECTION_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_CONNECTIONS: usize = 32;
type HttpResponse = Response<Full<Bytes>>;

pub(crate) struct Call {
    pub request: Request,
    pub reply: oneshot::Sender<io::Result<Option<Snapshot>>>,
}

pub(crate) struct Server {
    pub incoming: Receiver<Call>,
    stop: Option<oneshot::Sender<()>>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Server {
    pub fn bind(path: &Path) -> io::Result<Self> {
        let listener = UnixListener::bind(path)?;
        listener.set_nonblocking(true)?;
        let runtime = runtime()?;
        let (send, incoming) = mpsc::sync_channel(MAX_CONNECTIONS);
        let (stop, stopped) = oneshot::channel();
        let thread = thread::spawn(move || {
            runtime.block_on(async move {
                if let Err(error) = serve(listener, send, stopped).await {
                    log::error!("http_server_failed err={error}");
                }
            });
        });
        Ok(Self {
            incoming,
            stop: Some(stop),
            thread: Some(thread),
        })
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

async fn serve(
    listener: UnixListener,
    send: SyncSender<Call>,
    mut stopped: oneshot::Receiver<()>,
) -> io::Result<()> {
    let listener = tokio::net::UnixListener::from_std(listener)?;
    let mut connections = JoinSet::new();
    loop {
        tokio::select! {
            _ = &mut stopped => break,
            _ = connections.join_next(), if !connections.is_empty() => {},
            accepted = listener.accept(), if connections.len() < MAX_CONNECTIONS => {
                let (stream, _) = match accepted {
                    Ok(value) => value,
                    Err(error) if matches!(error.kind(), io::ErrorKind::ConnectionAborted | io::ErrorKind::Interrupted) => continue,
                    Err(error) => return Err(error),
                };
                let send = send.clone();
                connections.spawn(async move {
                    let service = service_fn(move |request| {
                        let send = send.clone();
                        async move { Ok::<_, std::convert::Infallible>(handle(request, send).await) }
                    });
                    // Every connection handles one request, with an overall deadline.
                    // Incomplete headers/bodies cannot pin the database owner.
                    let result = tokio::time::timeout(CONNECTION_TIMEOUT,
                        hyper::server::conn::http1::Builder::new()
                            .keep_alive(false).max_buf_size(16 * 1024)
                            .serve_connection(TokioIo::new(stream), service)
                    ).await;
                    if let Ok(Err(error)) = result { log::debug!("http_connection_closed err={error}"); }
                });
            }
        }
    }
    // Let the shutdown response finish before dropping its socket. Remaining
    // slow clients are bounded by the same connection deadline.
    drop(listener);
    if tokio::time::timeout(CONNECTION_TIMEOUT, async {
        while connections.join_next().await.is_some() {}
    })
    .await
    .is_err()
    {
        connections.abort_all();
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RefreshBody {
    home: String,
}

async fn handle(request: HttpRequest<Incoming>, send: SyncSender<Call>) -> HttpResponse {
    let expected = match request.uri().path() {
        "/v1/snapshot" => Method::GET,
        "/v1/refresh" | "/v1/shutdown" => Method::POST,
        _ => return error(StatusCode::NOT_FOUND, "unknown route or API version"),
    };
    if request.method() != expected {
        let mut response = error(StatusCode::METHOD_NOT_ALLOWED, "method not allowed");
        response
            .headers_mut()
            .insert(ALLOW, expected.as_str().parse().unwrap());
        return response;
    }
    let path = request.uri().path().to_owned();
    if path == "/v1/refresh"
        && request
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_none_or(|value| {
                !value
                    .split(';')
                    .next()
                    .unwrap_or_default()
                    .trim()
                    .eq_ignore_ascii_case("application/json")
            })
    {
        return error(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "refresh requires application/json",
        );
    }
    let body = match Limited::new(request.into_body(), REQUEST_LIMIT)
        .collect()
        .await
    {
        Ok(body) => body.to_bytes(),
        Err(error_value) => {
            return error(
                if error_value.is::<http_body_util::LengthLimitError>() {
                    StatusCode::PAYLOAD_TOO_LARGE
                } else {
                    StatusCode::BAD_REQUEST
                },
                "invalid or oversized request body",
            );
        }
    };
    let command = match path.as_str() {
        "/v1/snapshot" | "/v1/shutdown" if !body.is_empty() => {
            return error(StatusCode::BAD_REQUEST, "this route does not accept a body")
        }
        "/v1/snapshot" => Request::Snapshot,
        "/v1/shutdown" => Request::Shutdown,
        _ => match serde_json::from_slice::<RefreshBody>(&body) {
            Ok(body) => Request::Refresh { home: body.home },
            Err(_) => {
                return error(
                    StatusCode::BAD_REQUEST,
                    "expected JSON object with a home string",
                )
            }
        },
    };
    let (reply, received) = oneshot::channel();
    if send
        .try_send(Call {
            request: command,
            reply,
        })
        .is_err()
    {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "daemon unavailable or busy",
        );
    }
    match received.await {
        Ok(Ok(Some(snapshot))) => json(StatusCode::OK, &snapshot),
        Ok(Ok(None)) => json(StatusCode::ACCEPTED, &serde_json::json!({"accepted": true})),
        Ok(Err(reason)) => error(StatusCode::INTERNAL_SERVER_ERROR, &reason.to_string()),
        Err(_) => error(StatusCode::SERVICE_UNAVAILABLE, "daemon stopped"),
    }
}

fn error(status: StatusCode, message: &str) -> HttpResponse {
    json(status, &serde_json::json!({"error": message}))
}

fn json(status: StatusCode, value: &impl serde::Serialize) -> HttpResponse {
    match serde_json::to_vec(value) {
        Ok(bytes) if bytes.len() <= RESPONSE_LIMIT => Response::builder()
            .status(status)
            .header(CONTENT_TYPE, "application/json")
            .body(Full::new(Bytes::from(bytes)))
            .unwrap(),
        _ => Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .header(CONTENT_TYPE, "application/json")
            .body(Full::new(Bytes::from_static(
                b"{\"error\":\"response serialization failed or exceeds limit\"}",
            )))
            .unwrap(),
    }
}

fn runtime() -> io::Result<Runtime> {
    Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()
}

thread_local! {
    // Synchronous TUI callers reuse a current-thread runtime; no client worker
    // thread, TLS stack, DNS, proxy or TCP listener is needed.
    static CLIENT: RefCell<Option<Runtime>> = const { RefCell::new(None) };
}

pub(crate) fn request_at(dir: &Path, command: Request) -> io::Result<Option<Snapshot>> {
    let socket = fs::read_to_string(dir.join("daemon.endpoint"))?;
    CLIENT.with(|client| {
        let mut runtime_ref = client.borrow_mut();
        if runtime_ref.is_none() {
            *runtime_ref = Some(runtime()?);
        }
        runtime_ref.as_ref().unwrap().block_on(async {
            tokio::time::timeout(CLIENT_TIMEOUT, request(&socket, command))
                .await
                .map_err(|_| {
                    io::Error::new(io::ErrorKind::TimedOut, "daemon HTTP request timed out")
                })?
        })
    })
}

struct Connection(tokio::task::JoinHandle<()>);
impl Drop for Connection {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn request(socket: &str, command: Request) -> io::Result<Option<Snapshot>> {
    let (method, path, body) = match command {
        Request::Snapshot => (Method::GET, "/v1/snapshot", Vec::new()),
        Request::Refresh { home } => (
            Method::POST,
            "/v1/refresh",
            serde_json::to_vec(&serde_json::json!({"home": home}))?,
        ),
        Request::Shutdown => (Method::POST, "/v1/shutdown", Vec::new()),
    };
    let expects_snapshot = path == "/v1/snapshot";
    if body.len() > REQUEST_LIMIT {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "request body exceeds limit",
        ));
    }
    let stream = tokio::net::UnixStream::connect(socket).await?;
    let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .map_err(io::Error::other)?;
    let _connection = Connection(tokio::spawn(async move {
        let _ = connection.await;
    }));
    let request = HttpRequest::builder()
        .method(method)
        .uri(path)
        .header("Host", "localhost")
        .header("Connection", "close")
        .header(CONTENT_TYPE, "application/json")
        .body(Full::new(Bytes::from(body)))
        .map_err(io::Error::other)?;
    let response = sender
        .send_request(request)
        .await
        .map_err(io::Error::other)?;
    let status = response.status();
    let body = Limited::new(response.into_body(), RESPONSE_LIMIT)
        .collect()
        .await
        .map_err(io::Error::other)?
        .to_bytes();
    let expected = if expects_snapshot {
        StatusCode::OK
    } else {
        StatusCode::ACCEPTED
    };
    if status != expected {
        return Err(io::Error::other(format!(
            "daemon HTTP {status}: {}",
            String::from_utf8_lossy(&body)
        )));
    }
    if expects_snapshot {
        Ok(Some(serde_json::from_slice(&body)?))
    } else {
        let accepted: serde_json::Value = serde_json::from_slice(&body)?;
        if accepted.get("accepted").and_then(|value| value.as_bool()) != Some(true) {
            return Err(io::Error::other("daemon did not acknowledge request"));
        }
        Ok(None)
    }
}
