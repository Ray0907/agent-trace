use anyhow::{anyhow, Result};
use futures::{SinkExt, StreamExt};
use serde::Serialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::{sleep, Duration};
use tokio_tungstenite::accept_hdr_async;
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
use tokio_tungstenite::tungstenite::Message;

use crate::state::AppState;

pub async fn serve(listener: TcpListener, state: AppState) -> Result<()> {
    loop {
        let (stream, _) = listener.accept().await?;
        let state = state.clone();

        tokio::spawn(async move {
            if let Err(error) = handle_connection(stream, state).await {
                eprintln!("connection error: {error}");
            }
        });
    }
}

async fn handle_connection(mut stream: TcpStream, state: AppState) -> Result<()> {
    let request = inspect_request(&stream).await?;

    if request.is_websocket_upgrade && request.path == "/api/stream" {
        let callback = |_request: &Request, mut response: Response| {
            response.headers_mut().insert(
                "Access-Control-Allow-Origin",
                "*".parse().expect("valid header"),
            );
            Ok(response)
        };

        let websocket = accept_hdr_async(stream, callback).await?;
        stream_socket(websocket, state).await;
        return Ok(());
    }

    consume_request_head(&mut stream, request.header_len).await?;

    match (request.method.as_str(), request.path.as_str()) {
        ("OPTIONS", _) => write_response(&mut stream, HttpStatus::NoContent, None).await?,
        ("GET", "/api/health") => {
            write_json(
                &mut stream,
                HttpStatus::Ok,
                &serde_json::json!({ "ok": true }),
            )
            .await?
        }
        ("GET", "/api/sessions") => {
            let sessions = state.list_summaries().await;
            write_json(&mut stream, HttpStatus::Ok, &sessions).await?;
        }
        ("GET", path) => match parse_session_route(path) {
            Some((session_id, SessionRoute::Trace)) => match state.trace_response(session_id).await
            {
                Some(trace) => write_json(&mut stream, HttpStatus::Ok, &trace).await?,
                None => {
                    write_json(
                        &mut stream,
                        HttpStatus::NotFound,
                        &serde_json::json!({ "error": "session not found" }),
                    )
                    .await?
                }
            },
            Some((session_id, SessionRoute::Cost)) => match state.cost_response(session_id).await {
                Some(cost) => write_json(&mut stream, HttpStatus::Ok, &cost).await?,
                None => {
                    write_json(
                        &mut stream,
                        HttpStatus::NotFound,
                        &serde_json::json!({ "error": "session not found" }),
                    )
                    .await?
                }
            },
            None => {
                write_json(
                    &mut stream,
                    HttpStatus::NotFound,
                    &serde_json::json!({ "error": "not found" }),
                )
                .await?
            }
        },
        _ => {
            write_json(
                &mut stream,
                HttpStatus::MethodNotAllowed,
                &serde_json::json!({ "error": "method not allowed" }),
            )
            .await?
        }
    }

    Ok(())
}

#[derive(Debug)]
struct ParsedRequest {
    method: String,
    path: String,
    header_len: usize,
    is_websocket_upgrade: bool,
}

async fn inspect_request(stream: &TcpStream) -> Result<ParsedRequest> {
    let mut buffer = vec![0_u8; 8192];

    for _ in 0..50 {
        let bytes_read = stream.peek(&mut buffer).await?;
        if bytes_read == 0 {
            return Err(anyhow!(
                "connection closed before request headers were available"
            ));
        }

        let Some(header_len) = find_header_end(&buffer[..bytes_read]) else {
            sleep(Duration::from_millis(10)).await;
            continue;
        };

        let mut headers = [httparse::EMPTY_HEADER; 64];
        let mut request = httparse::Request::new(&mut headers);
        request.parse(&buffer[..header_len])?;

        let method = request
            .method
            .ok_or_else(|| anyhow!("request missing method"))?
            .to_string();
        let path = request
            .path
            .ok_or_else(|| anyhow!("request missing path"))?
            .to_string();

        let is_websocket_upgrade = header_value(request.headers, "Upgrade")
            .is_some_and(|value| value.eq_ignore_ascii_case("websocket"));

        return Ok(ParsedRequest {
            method,
            path,
            header_len,
            is_websocket_upgrade,
        });
    }

    Err(anyhow!(
        "request headers were not fully received within timeout"
    ))
}

async fn consume_request_head(stream: &mut TcpStream, header_len: usize) -> Result<()> {
    let mut buffer = vec![0_u8; header_len];
    stream.read_exact(&mut buffer).await?;
    Ok(())
}

fn find_header_end(buffer: &[u8]) -> Option<usize> {
    buffer
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|position| position + 4)
}

fn header_value<'a>(headers: &'a [httparse::Header<'a>], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|header| header.name.eq_ignore_ascii_case(name))
        .and_then(|header| std::str::from_utf8(header.value).ok())
}

#[derive(Debug, Clone, Copy)]
enum SessionRoute {
    Trace,
    Cost,
}

fn parse_session_route(path: &str) -> Option<(&str, SessionRoute)> {
    let parts: Vec<_> = path.trim_matches('/').split('/').collect();
    if parts.len() != 4 || parts[0] != "api" || parts[1] != "sessions" {
        return None;
    }

    match parts[3] {
        "trace" => Some((parts[2], SessionRoute::Trace)),
        "cost" => Some((parts[2], SessionRoute::Cost)),
        _ => None,
    }
}

async fn stream_socket(mut socket: tokio_tungstenite::WebSocketStream<TcpStream>, state: AppState) {
    let mut receiver = state.subscribe();

    loop {
        tokio::select! {
            event = receiver.recv() => {
                match event {
                    Ok(event) => {
                        let Ok(payload) = serde_json::to_string(&event) else {
                            continue;
                        };
                        if socket.send(Message::Text(payload.into())).await.is_err() {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
            incoming = socket.next() => {
                match incoming {
                    Some(Ok(Message::Ping(payload))) => {
                        if socket.send(Message::Pong(payload)).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(_)) => {}
                    Some(Err(_)) => break,
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum HttpStatus {
    Ok,
    NoContent,
    NotFound,
    MethodNotAllowed,
}

impl HttpStatus {
    fn as_status_line(self) -> &'static str {
        match self {
            Self::Ok => "200 OK",
            Self::NoContent => "204 No Content",
            Self::NotFound => "404 Not Found",
            Self::MethodNotAllowed => "405 Method Not Allowed",
        }
    }
}

async fn write_json<T: Serialize>(
    stream: &mut TcpStream,
    status: HttpStatus,
    payload: &T,
) -> Result<()> {
    let body = serde_json::to_vec(payload)?;
    write_response(stream, status, Some((&body, "application/json"))).await
}

async fn write_response(
    stream: &mut TcpStream,
    status: HttpStatus,
    body: Option<(&[u8], &str)>,
) -> Result<()> {
    let (body_bytes, content_type) = match body {
        Some((bytes, content_type)) => (bytes, content_type),
        None => (&[][..], "text/plain"),
    };

    let response_head = format!(
        "HTTP/1.1 {}\r\nContent-Length: {}\r\nContent-Type: {}\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: GET, OPTIONS\r\nAccess-Control-Allow-Headers: *\r\nConnection: close\r\n\r\n",
        status.as_status_line(),
        body_bytes.len(),
        content_type,
    );

    stream.write_all(response_head.as_bytes()).await?;
    if !body_bytes.is_empty() {
        stream.write_all(body_bytes).await?;
    }
    stream.shutdown().await?;
    Ok(())
}
