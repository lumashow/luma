//! A loopback MCP server for an engine that takes its tools over HTTP.
//!
//! Claude asks for Luma's tools over its own control channel and Codex takes
//! them as dynamic tools, so neither needs this. Antigravity's ACP server
//! takes tools only as MCP servers it connects to itself. This is that
//! server: one per session, bound to 127.0.0.1 on a port the system picks,
//! answering only requests that carry the session's bearer token. A
//! `tools/call` becomes a [`Call`] for the session to hand to the turn, and
//! its HTTP response waits for [`Replies::reply`].

use super::{content, protocol, AgentError, ContentBlock, ToolOutcome, ToolSpec};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream},
    sync::{mpsc, oneshot},
};

/// The name the engine knows the server by. It prefixes every tool the model
/// sees.
pub(super) const NAME: &str = "luma";

/// A request larger than this is not a tool call.
const MAX_BODY: usize = 64 * 1024 * 1024;

pub(super) struct Server {
    pub url: String,
    pub token: String,
    pub calls: mpsc::UnboundedReceiver<Call>,
    replies: Replies,
    accept: tokio::task::JoinHandle<()>,
}

/// One `tools/call` waiting for its result.
pub(super) struct Call {
    pub id: String,
    pub name: String,
    pub input: Value,
    /// What [`Replies::reply`] takes to answer it.
    pub reply: Value,
}

/// Answers calls. A handle of its own, so a reply never waits on the
/// session's read.
#[derive(Clone, Default)]
pub(super) struct Replies(Arc<Mutex<Waiting>>);

#[derive(Default)]
struct Waiting {
    next: u64,
    calls: HashMap<u64, oneshot::Sender<ToolOutcome>>,
}

impl Replies {
    pub fn reply(&self, id: &Value, outcome: ToolOutcome) -> Result<(), AgentError> {
        let waiting = id
            .as_u64()
            .and_then(|id| self.0.lock().expect("mcp replies").calls.remove(&id))
            .ok_or_else(|| protocol("tool reply names no waiting call"))?;
        // The engine hung up on a call it had cancelled: nobody to tell.
        let _ = waiting.send(outcome);
        Ok(())
    }

    fn wait(&self) -> (u64, oneshot::Receiver<ToolOutcome>) {
        let (send, receive) = oneshot::channel();
        let mut waiting = self.0.lock().expect("mcp replies");
        waiting.next += 1;
        let id = waiting.next;
        waiting.calls.insert(id, send);
        (id, receive)
    }
}

impl Server {
    pub async fn start(tools: &[ToolSpec]) -> Result<Self, AgentError> {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .map_err(|e| protocol(format!("could not open the tool server: {e}")))?;
        let port = listener
            .local_addr()
            .map_err(|e| protocol(format!("could not open the tool server: {e}")))?
            .port();
        let token = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        let (send, calls) = mpsc::unbounded_channel();
        let replies = Replies::default();
        let shared = Arc::new(Shared {
            token: token.clone(),
            tools: json!({ "tools": super::tool_definitions(tools) }),
            calls: send,
            replies: replies.clone(),
        });
        let accept = tokio::spawn(async move {
            // Owns every connection, so aborting this task closes them all.
            let mut connections = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let Ok((stream, _)) = accepted else { break };
                        connections.spawn(serve(stream, Arc::clone(&shared)));
                    }
                    Some(_) = connections.join_next() => {}
                }
            }
        });
        Ok(Self {
            url: format!("http://127.0.0.1:{port}/mcp"),
            token,
            calls,
            replies,
            accept,
        })
    }

    pub fn replies(&self) -> Replies {
        self.replies.clone()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.accept.abort();
    }
}

struct Shared {
    token: String,
    tools: Value,
    calls: mpsc::UnboundedSender<Call>,
    replies: Replies,
}

/// One connection: requests in turn until the client closes it.
async fn serve(stream: TcpStream, shared: Arc<Shared>) {
    let (read, mut write) = stream.into_split();
    let mut read = BufReader::new(read);
    while let Ok(Some(request)) = read_request(&mut read).await {
        let (status, body) = respond(request, &shared).await;
        let head = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
            body.len()
        );
        if write.write_all(head.as_bytes()).await.is_err() || write.write_all(&body).await.is_err()
        {
            break;
        }
    }
}

struct HttpRequest {
    method: String,
    authorization: Option<String>,
    body: Vec<u8>,
}

/// `Ok(None)` when the client closed the connection or sent something that
/// is not a request this server reads.
async fn read_request(
    read: &mut BufReader<tokio::net::tcp::OwnedReadHalf>,
) -> std::io::Result<Option<HttpRequest>> {
    let mut line = String::new();
    if read.read_line(&mut line).await? == 0 {
        return Ok(None);
    }
    let method = line.split(' ').next().unwrap_or_default().to_string();
    let mut authorization = None;
    let mut length = 0;
    loop {
        line.clear();
        if read.read_line(&mut line).await? == 0 {
            return Ok(None);
        }
        let header = line.trim_end();
        if header.is_empty() {
            break;
        }
        let Some((name, value)) = header.split_once(':') else {
            return Ok(None);
        };
        let value = value.trim();
        if name.eq_ignore_ascii_case("content-length") {
            match value.parse::<usize>() {
                Ok(given) if given <= MAX_BODY => length = given,
                _ => return Ok(None),
            }
        } else if name.eq_ignore_ascii_case("authorization") {
            authorization = Some(value.to_string());
        } else if name.eq_ignore_ascii_case("transfer-encoding") {
            // Chunked bodies are not read; the client sends lengths.
            return Ok(None);
        }
    }
    let mut body = vec![0; length];
    read.read_exact(&mut body).await?;
    Ok(Some(HttpRequest {
        method,
        authorization,
        body,
    }))
}

async fn respond(request: HttpRequest, shared: &Shared) -> (&'static str, Vec<u8>) {
    if request.authorization.as_deref() != Some(&format!("Bearer {}", shared.token)) {
        return ("401 Unauthorized", Vec::new());
    }
    // The server opens no stream of its own, which is what a GET asks for.
    if request.method != "POST" {
        return ("405 Method Not Allowed", Vec::new());
    }
    let Ok(message) = serde_json::from_slice::<Value>(&request.body) else {
        return ("400 Bad Request", Vec::new());
    };
    let Some(id) = message.get("id").filter(|id| !id.is_null()).cloned() else {
        return ("202 Accepted", Vec::new());
    };
    let answer = match answer(&message, shared).await {
        Ok(result) => json!({"jsonrpc":"2.0","id":id,"result":result}),
        Err((code, message)) => {
            json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})
        }
    };
    ("200 OK", serde_json::to_vec(&answer).unwrap_or_default())
}

async fn answer(message: &Value, shared: &Shared) -> Result<Value, (i64, String)> {
    let params = &message["params"];
    match message["method"].as_str().unwrap_or("") {
        "initialize" => Ok(json!({
            "protocolVersion": params["protocolVersion"].as_str().unwrap_or("2025-06-18"),
            "capabilities": {"tools": {}},
            "serverInfo": {"name": NAME, "version": env!("CARGO_PKG_VERSION")},
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(shared.tools.clone()),
        "tools/call" => {
            let name = params["name"]
                .as_str()
                .ok_or((-32602, "tool call has no name".to_string()))?;
            let (id, result) = shared.replies.wait();
            let call = Call {
                id: format!("antigravity-{id}"),
                name: name.to_string(),
                input: params["arguments"].clone(),
                reply: json!(id),
            };
            let gone = || (-32603, "the session ended".to_string());
            shared.calls.send(call).map_err(|_| gone())?;
            Ok(tool_result(result.await.map_err(|_| gone())?))
        }
        // A client probes for newer protocol methods and falls back on this.
        _ => Err((-32601, "Method not found".to_string())),
    }
}

fn tool_result(outcome: ToolOutcome) -> Value {
    let (blocks, failed) = content(outcome);
    let content: Vec<_> = blocks
        .into_iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(json!({"type":"text","text":text})),
            ContentBlock::Image { media_type, data } => {
                Some(json!({"type":"image","mimeType":media_type,"data":data}))
            }
            _ => None,
        })
        .collect();
    json!({"content":content,"isError":failed})
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn post(url: &str, token: &str, body: Value) -> (u16, Value) {
        let response = reqwest::Client::new()
            .post(url)
            .bearer_auth(token)
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = response.status().as_u16();
        let text = response.text().await.unwrap();
        (status, serde_json::from_str(&text).unwrap_or(Value::Null))
    }

    fn echo() -> Vec<ToolSpec> {
        vec![ToolSpec {
            name: "echo".into(),
            description: "Echo input".into(),
            schema: json!({"type":"object"}),
        }]
    }

    /// A client lists the tools, calls one, and gets what the session
    /// replied, images included. A notification is accepted without a body
    /// and a method the server lacks is an error the client can fall back on.
    #[tokio::test]
    async fn a_call_waits_for_the_sessions_reply() {
        let mut server = Server::start(&echo()).await.unwrap();
        let (url, token) = (server.url.clone(), server.token.clone());
        let request = |id: u64, method: &str, params: Value| {
            post(
                &url,
                &token,
                json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}),
            )
        };
        let (status, listed) = request(1, "tools/list", json!({})).await;
        assert_eq!(status, 200);
        assert_eq!(listed["result"]["tools"][0]["name"], "echo");
        let notification = json!({"jsonrpc":"2.0","method":"notifications/initialized"});
        assert_eq!(post(&url, &token, notification).await.0, 202);
        let (_, unknown) = request(2, "server/discover", json!({})).await;
        assert_eq!(unknown["error"]["code"], -32601);

        let replies = server.replies();
        let call = request(
            3,
            "tools/call",
            json!({"name":"echo","arguments":{"value":"hi"}}),
        );
        let session = async {
            let call = server.calls.recv().await.unwrap();
            assert_eq!(call.name, "echo");
            assert_eq!(call.input, json!({"value":"hi"}));
            let image = ContentBlock::Image {
                media_type: "image/png".into(),
                data: "aGVsbG8=".into(),
            };
            let outcome = ToolOutcome::Content(vec![ContentBlock::Text("hi".into()), image]);
            replies.reply(&call.reply, outcome).unwrap();
            // Answered once: the same call has nobody waiting any more.
            assert!(replies
                .reply(&call.reply, ToolOutcome::Text("again".into()))
                .is_err());
        };
        let ((status, called), ()) = tokio::join!(call, session);
        assert_eq!(status, 200);
        let result = &called["result"];
        assert_eq!(result["isError"], false);
        assert_eq!(result["content"][0], json!({"type":"text","text":"hi"}));
        assert_eq!(
            result["content"][1],
            json!({"type":"image","mimeType":"image/png","data":"aGVsbG8="})
        );
    }

    #[tokio::test]
    async fn a_request_without_the_sessions_token_is_refused() {
        let server = Server::start(&echo()).await.unwrap();
        let list = json!({"jsonrpc":"2.0","id":1,"method":"tools/list"});
        assert_eq!(post(&server.url, "wrong", list.clone()).await.0, 401);
        assert_eq!(post(&server.url, &server.token, list).await.0, 200);
    }
}
