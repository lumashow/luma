//! Google's Antigravity CLI over the Agent Client Protocol: `agy acp`, JSON-RPC
//! on stdio. The unmodified CLI owns authentication and its model loop; Luma's
//! tools reach it through a loopback MCP server ([`super::mcp_http`]).
//!
//! Checked by hand against `agy` 1.5.10:
//!
//! - Tools arrive as `mcp_luma_<name>`, and every call asks permission first
//!   (`session/request_permission`). That request is the gate: Luma's tools
//!   are allowed and everything else — the CLI's shell and file edits — is
//!   refused. The CLI reads files inside the session's directory without
//!   asking; that directory holds only this thread's run state.
//! - The system prompt is appended to the CLI's own, and only when a session
//!   is created. A resumed session keeps the one it started with.
//! - A model's thinking level is part of its id: `gemini-4-carbon-high`.
//! - A prompt cannot be interrupted safely, so a steer waits for the turn's
//!   end and becomes the next prompt on the same session.

use super::{
    catalog::ModelChoice,
    mcp_http,
    process::{command, Cli, Process},
    protocol, AgentError, Event, Request, Usage,
};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

const AGY: Cli = Cli {
    name: "agy",
    label: "Antigravity CLI",
};

/// The JSON-RPC error an ACP agent answers with when nobody is signed in.
const AUTH_REQUIRED: i64 = -32000;

const INITIALIZE: u64 = 1;
const OPEN: u64 = 2;
const SET_MODEL: u64 = 3;
/// Prompts count up from here.
const FIRST_PROMPT: u64 = 10;

type Steers = Arc<Mutex<VecDeque<(String, Vec<String>)>>>;

pub(in crate::agent) struct Session {
    process: Process,
    request: Request,
    server: mcp_http::Server,
    id: Option<String>,
    model: Option<String>,
    /// The id of the prompt in flight. `None` until the first is sent: what a
    /// resumed session says before then is not this turn's.
    prompt: Option<u64>,
    pub(super) last_usage: Option<Usage>,
    pub(super) context_window: Option<u64>,
    steers: Steers,
    /// Events one frame produced beyond the one returned.
    ready: VecDeque<Event>,
}

impl Session {
    pub async fn start(request: Request) -> Result<Self, AgentError> {
        let mut cmd = command(AGY, &request.cwd)?;
        cmd.arg("acp");
        Self::connect(request, Process::start(cmd)?).await
    }

    async fn connect(request: Request, mut process: Process) -> Result<Self, AgentError> {
        let server = mcp_http::Server::start(&request.tools).await?;
        initialize(&mut process).await?;
        Ok(Self {
            process,
            request,
            server,
            id: None,
            model: None,
            prompt: None,
            last_usage: None,
            context_window: None,
            steers: Arc::default(),
            ready: VecDeque::new(),
        })
    }

    pub(super) fn steerer(&self) -> Steerer {
        Steerer(Arc::clone(&self.steers))
    }

    pub(super) fn replies(&self) -> mcp_http::Replies {
        self.server.replies()
    }

    pub async fn next(&mut self) -> Result<Event, AgentError> {
        loop {
            if let Some(event) = self.ready.pop_front() {
                return Ok(event);
            }
            let frame = tokio::select! {
                frame = self.process.read() => frame?,
                Some(call) = self.server.calls.recv() => {
                    return Ok(Event::Tool {
                        id: call.id,
                        name: call.name,
                        input: call.input,
                        reply: call.reply,
                    });
                }
            };
            let event = match (frame["method"].as_str(), frame.get("id")) {
                (Some(method), Some(id)) => {
                    self.asked(method, id.clone(), &frame["params"]).await?;
                    None
                }
                (Some("session/update"), None) if self.prompt.is_some() => {
                    self.updated(&frame["params"]["update"])
                }
                (Some(_), None) => None,
                (None, _) => self.answered(&frame).await?,
            };
            if let Some(event) = event {
                return Ok(event);
            }
        }
    }

    /// A request from the CLI. The only one Luma grants is permission to call
    /// one of its own tools.
    async fn asked(&mut self, method: &str, id: Value, params: &Value) -> Result<(), AgentError> {
        if method != "session/request_permission" {
            return self
                .process
                .send(json!({"jsonrpc":"2.0","id":id,"error":{
                    "code":-32601,"message":"Only Luma tools are available in this session"
                }}))
                .await;
        }
        let allowed = params["toolCall"]["name"]
            .as_str()
            .and_then(|name| name.strip_prefix("mcp_"))
            .and_then(|name| name.strip_prefix(mcp_http::NAME))
            .and_then(|name| name.strip_prefix('_'))
            .is_some_and(|name| self.request.tools.iter().any(|tool| tool.name == name));
        let wanted = if allowed { "allow_once" } else { "reject_once" };
        let option = params["options"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|option| option["kind"] == wanted)
            .and_then(|option| option["optionId"].as_str());
        let outcome = match option {
            Some(option) => json!({"outcome":"selected","optionId":option}),
            None => json!({"outcome":"cancelled"}),
        };
        self.process
            .send(json!({"jsonrpc":"2.0","id":id,"result":{"outcome":outcome}}))
            .await
    }

    /// What the CLI reports while a prompt runs.
    fn updated(&mut self, update: &Value) -> Option<Event> {
        let text = || update["content"]["text"].as_str().map(str::to_string);
        match update["sessionUpdate"].as_str().unwrap_or("") {
            "agent_message_chunk" => text().map(Event::Text),
            "agent_thought_chunk" => text().map(Event::Reasoning),
            // After each model request: what the conversation now holds, and
            // the window it is held in.
            "usage_update" => {
                if let Some(size) = update["size"].as_u64().filter(|size| *size > 0) {
                    self.context_window = Some(size);
                }
                let held = Usage {
                    input_tokens: update["used"].as_u64().filter(|used| *used > 0)?,
                    ..Usage::default()
                };
                if self.last_usage == Some(held) {
                    return None;
                }
                self.last_usage = Some(held);
                Some(Event::Step(held))
            }
            _ => None,
        }
    }

    /// The answer to a request this session made.
    async fn answered(&mut self, frame: &Value) -> Result<Option<Event>, AgentError> {
        let id = frame["id"].as_u64();
        if let Some(error) = frame.get("error") {
            if error["code"] == AUTH_REQUIRED {
                return Err(protocol(super::auth::SIGNED_OUT_ANTIGRAVITY));
            }
            return Err(protocol(format!(
                "Antigravity: {}",
                error["message"].as_str().unwrap_or("request failed")
            )));
        }
        let result = &frame["result"];
        match id {
            Some(INITIALIZE) => {
                let servers = json!([{
                    "type":"http","name":mcp_http::NAME,"url":self.server.url,
                    "headers":[{"name":"Authorization","value":format!("Bearer {}", self.server.token)}],
                }]);
                let mut params = json!({"cwd":self.request.cwd,"mcpServers":servers});
                let method = match &self.request.resume {
                    Some(resume) => {
                        params["sessionId"] = json!(resume.id);
                        "session/resume"
                    }
                    None => {
                        params["_meta"] =
                            json!({"antigravity":{"systemPrompt":{"append":self.request.system}}});
                        "session/new"
                    }
                };
                self.send(OPEN, method, params).await?;
                Ok(None)
            }
            Some(OPEN) => {
                let id = result["sessionId"]
                    .as_str()
                    .or(self
                        .request
                        .resume
                        .as_ref()
                        .map(|resume| resume.id.as_str()))
                    .ok_or_else(|| protocol("Antigravity did not return a session id"))?
                    .to_string();
                self.id = Some(id.clone());
                self.model = result["models"]["currentModelId"]
                    .as_str()
                    .map(str::to_string);
                let listed: Vec<_> = result["models"]["availableModels"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|model| model["modelId"].as_str())
                    .collect();
                match wire_model(&self.request, &listed) {
                    Some(model) if self.model.as_ref() != Some(&model) => {
                        self.model = Some(model.clone());
                        self.send(
                            SET_MODEL,
                            "session/set_model",
                            json!({"sessionId":id,"modelId":model}),
                        )
                        .await?;
                        Ok(None)
                    }
                    _ => self.begin().await.map(Some),
                }
            }
            Some(SET_MODEL) => self.begin().await.map(Some),
            Some(id) if Some(id) == self.prompt => {
                let usage = &result["usage"];
                let count = |key: &str| usage[key].as_u64().unwrap_or(0);
                self.ready.push_back(Event::Usage(Usage {
                    input_tokens: count("inputTokens"),
                    output_tokens: count("outputTokens"),
                    cache_creation_input_tokens: count("cachedWriteTokens"),
                    cache_read_input_tokens: count("cachedReadTokens"),
                }));
                // Steers that waited for this turn's end are the next prompt.
                let waiting: Vec<_> = self.steers.lock().expect("steers").drain(..).collect();
                if waiting.is_empty() {
                    self.ready.push_back(Event::Done);
                    return Ok(None);
                }
                let (ids, blocks): (Vec<_>, Vec<_>) = waiting.into_iter().unzip();
                let blocks: Vec<_> = blocks.into_iter().flatten().collect();
                self.prompt(id + 1, &blocks).await?;
                self.ready.push_back(Event::Steered(ids));
                Ok(None)
            }
            _ => Ok(None),
        }
    }

    /// The session is open on its model: send the turn's prompt.
    async fn begin(&mut self) -> Result<Event, AgentError> {
        let blocks = self.request.prompt.clone();
        self.prompt(FIRST_PROMPT, &blocks).await?;
        Ok(Event::Session {
            id: self.id.clone().expect("opened session"),
            model: self.model.clone(),
        })
    }

    async fn prompt(&mut self, id: u64, blocks: &[String]) -> Result<(), AgentError> {
        self.prompt = Some(id);
        let prompt: Vec<_> = blocks
            .iter()
            .map(|text| json!({"type":"text","text":text}))
            .collect();
        let session = self.id.clone().expect("opened session");
        self.send(
            id,
            "session/prompt",
            json!({"sessionId":session,"prompt":prompt}),
        )
        .await
    }

    async fn send(&mut self, id: u64, method: &str, params: Value) -> Result<(), AgentError> {
        self.process
            .send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
            .await
    }
}

/// Queues steers for the end of the running turn.
pub(in crate::agent) struct Steerer(Steers);

impl Steerer {
    pub fn steer(&self, id: &str, blocks: &[String]) {
        self.0
            .lock()
            .expect("steers")
            .push_back((id.to_owned(), blocks.to_vec()));
    }
}

async fn initialize(process: &mut Process) -> Result<(), AgentError> {
    process
        .send(json!({"jsonrpc":"2.0","id":INITIALIZE,"method":"initialize","params":{
            "protocolVersion":1,
            // No client filesystem or terminal: the CLI's tools get nothing
            // of Luma's to work through.
            "clientCapabilities":{"fs":{"readTextFile":false,"writeTextFile":false},"terminal":false},
            "clientInfo":{"name":"luma","version":env!("CARGO_PKG_VERSION")}
        }}))
        .await
}

/// The id the CLI knows the requested model by: the model, then its thinking
/// level. A selection that names no level (a thread saved without one) runs
/// the model's default among the ids the session `listed`: High, else Medium,
/// else the first, as [`ModelChoice::effort`] chooses.
fn wire_model(request: &Request, listed: &[&str]) -> Option<String> {
    let model = request.model.as_ref()?;
    if let Some(effort) = &request.effort {
        return Some(format!("{model}-{effort}"));
    }
    if listed.is_empty() || listed.contains(&model.as_str()) {
        return Some(model.clone());
    }
    let leveled = |level: &str| format!("{model}-{level}");
    let levels: Vec<_> = LEVELS
        .iter()
        .filter(|level| listed.contains(&leveled(level).as_str()))
        .collect();
    let level = ["high", "medium"]
        .iter()
        .find(|level| levels.contains(level))
        .or(levels.first().copied());
    Some(level.map_or_else(|| model.clone(), |level| leveled(level)))
}

/// Sign in through the CLI: it opens the browser itself and answers once the
/// login is saved, or right away when one already is.
pub(super) async fn sign_in(cwd: &std::path::Path) -> Result<(), AgentError> {
    let mut cmd = command(AGY, cwd)?;
    cmd.arg("acp");
    let mut process = Process::start(cmd)?;
    initialize(&mut process).await?;
    loop {
        let frame = process.read().await?;
        if frame.get("method").is_some() {
            continue;
        }
        if let Some(error) = frame.get("error") {
            return Err(protocol(format!(
                "Antigravity sign-in failed: {}",
                error["message"].as_str().unwrap_or("request failed")
            )));
        }
        match frame["id"].as_u64() {
            Some(INITIALIZE) => {
                let method = frame["result"]["authMethods"][0]["id"]
                    .as_str()
                    .unwrap_or("oauth-personal");
                process
                    .send(json!({"jsonrpc":"2.0","id":OPEN,"method":"authenticate",
                        "params":{"methodId":method}}))
                    .await?;
            }
            Some(OPEN) => return Ok(()),
            _ => {}
        }
    }
}

/// What the CLI's `/usage` command reports, asked in print mode: no turn
/// runs and no conversation is made.
pub(super) async fn usage(cwd: &std::path::Path) -> Result<Value, AgentError> {
    let mut cmd = command(AGY, cwd)?;
    cmd.args(["--print", "/usage", "--output-format", "json"])
        .kill_on_drop(true);
    let output = cmd
        .output()
        .await
        .map_err(|error| protocol(format!("Antigravity usage: {error}")))?;
    let mut answer: Value = serde_json::from_slice(&output.stdout).map_err(|_| {
        let said = String::from_utf8_lossy(&output.stderr);
        protocol(match said.trim() {
            said if super::auth::signed_out(said) => super::auth::SIGNED_OUT_ANTIGRAVITY.into(),
            said => format!("Antigravity usage: {said}"),
        })
    })?;
    Ok(answer["command"]["data"].take())
}

pub(super) async fn version(cwd: &std::path::Path) -> Option<String> {
    super::providers::version_output(command(AGY, cwd).ok()?).await
}

/// Whether the CLI is signed in. It names neither the account nor its plan
/// short of opening a session, so a signed-in CLI is an account with no
/// name: listing its models is the cheapest thing only a login can do.
pub(super) async fn account(
    cwd: &std::path::Path,
) -> Result<Option<super::providers::Account>, AgentError> {
    match models(cwd).await {
        Ok(_) => Ok(Some(super::providers::Account::default())),
        Err(error) if super::auth::signed_out(&error.to_string()) => Ok(None),
        Err(error) => Err(error),
    }
}

/// Thinking levels a model id can end in, lowest first.
const LEVELS: [&str; 6] = ["minimal", "low", "medium", "high", "xhigh", "max"];

/// The CLI's models, in its order, the one it starts new sessions on first.
pub(super) async fn models(cwd: &std::path::Path) -> Result<Vec<ModelChoice>, AgentError> {
    let mut cmd = command(AGY, cwd)?;
    cmd.arg("models").kill_on_drop(true);
    let output = cmd
        .output()
        .await
        .map_err(|error| protocol(format!("Antigravity models: {error}")))?;
    let listed = catalog(&String::from_utf8_lossy(&output.stdout));
    if listed.is_empty() {
        let said = String::from_utf8_lossy(&output.stderr);
        return Err(protocol(match said.trim() {
            "" => "Antigravity listed no models".to_string(),
            said if super::auth::signed_out(said) => super::auth::SIGNED_OUT_ANTIGRAVITY.into(),
            said => format!("Antigravity models: {said}"),
        }));
    }
    Ok(listed)
}

/// `agy models` prints one `id<TAB>name` per line, one line per thinking
/// level: `gemini-4-carbon-high<TAB>Gemini 4 Carbon (High)`. Lines that
/// differ only in level are one model with those levels.
fn catalog(listing: &str) -> Vec<ModelChoice> {
    let mut models: Vec<ModelChoice> = Vec::new();
    for line in listing.lines() {
        let Some((id, name)) = line.split_once('\t') else {
            continue;
        };
        let (id, name) = (id.trim(), name.trim());
        let leveled = LEVELS.iter().find_map(|level| {
            let id = id.strip_suffix(level)?.strip_suffix('-')?;
            let name = name.strip_suffix(')')?;
            let (name, said) = name.rsplit_once(" (")?;
            said.eq_ignore_ascii_case(level)
                .then_some((id, name, *level))
        });
        let (id, name, level) = match leveled {
            Some((id, name, level)) => (id, name, Some(level)),
            None => (id, name, None),
        };
        let at = match models.iter().position(|model| model.id == id) {
            Some(at) => at,
            None => {
                models.push(ModelChoice {
                    id: id.into(),
                    label: name.into(),
                    resolved_model: id.into(),
                    effort_levels: Vec::new(),
                    context_window: None,
                    price: None,
                });
                models.len() - 1
            }
        };
        if let Some(level) = level {
            models[at].effort_levels.push(level.into());
        }
    }
    let rank = |level: &String| LEVELS.iter().position(|known| known == level);
    for model in &mut models {
        model.effort_levels.sort_by_key(rank);
    }
    models
}

#[cfg(test)]
mod tests {
    use super::super::ToolOutcome;
    use super::*;

    /// Levels of one model are one row, lowest first; a model with no level
    /// in its id is a row with none, and so is one whose id merely ends like
    /// a level.
    #[test]
    fn levels_of_one_model_are_one_row() {
        let models = catalog(
            "Fetching available models...\n\
             gemini-4-carbon-medium\tGemini 4 Carbon (Medium)\n\
             gemini-4-carbon-high\tGemini 4 Carbon (High)\n\
             gemini-4-carbon-low\tGemini 4 Carbon (Low)\n\
             claude-sonnet-4-6\tClaude Sonnet 4.6 (Thinking)\n\
             gpt-oss-120b-medium\tGPT-OSS 120B (Medium)\n\
             odd-max\tOdd Max\n",
        );
        let rows: Vec<_> = models
            .iter()
            .map(|model| {
                (
                    model.id.as_str(),
                    model.label.as_str(),
                    model.effort_levels.join(","),
                )
            })
            .collect();
        assert_eq!(
            rows,
            [
                (
                    "gemini-4-carbon",
                    "Gemini 4 Carbon",
                    "low,medium,high".into()
                ),
                (
                    "claude-sonnet-4-6",
                    "Claude Sonnet 4.6 (Thinking)",
                    String::new()
                ),
                ("gpt-oss-120b", "GPT-OSS 120B", "medium".into()),
                ("odd-max", "Odd Max", String::new()),
            ]
        );
        // What a row and one of its levels send is an id the CLI listed.
        let cwd = std::path::Path::new(".");
        let mut request = super::super::tests::request(super::super::Engine::Antigravity, cwd);
        request.model = Some("gemini-4-carbon".into());
        request.effort = Some("high".into());
        let listed = [
            "gemini-4-carbon-low",
            "gemini-4-carbon-medium",
            "gemini-4-carbon-high",
            "claude-sonnet-4-6",
            "gpt-oss-120b-medium",
        ];
        let mut wire = |model: &str, effort: Option<&str>| {
            request.model = Some(model.into());
            request.effort = effort.map(str::to_string);
            let id = wire_model(&request, &listed).expect("a model was asked for");
            assert!(listed.contains(&id.as_str()), "{id}");
            id
        };
        assert_eq!(wire("gemini-4-carbon", Some("low")), "gemini-4-carbon-low");
        // No level asked for: the model's default, never an id without one.
        assert_eq!(wire("gemini-4-carbon", None), "gemini-4-carbon-high");
        assert_eq!(wire("gpt-oss-120b", None), "gpt-oss-120b-medium");
        assert_eq!(wire("claude-sonnet-4-6", None), "claude-sonnet-4-6");
    }

    fn fake(script: &str) -> Process {
        let mut command = tokio::process::Command::new("python3");
        command.args(["-c", script]);
        Process::start(command).unwrap()
    }

    /// The fake CLI's side of the tool server: what a real one does with the
    /// `mcpServers` entry it was given.
    const CALL_TOOL: &str = r#"
import json,sys,urllib.request
read=lambda: json.loads(sys.stdin.readline())
def send(x): print(json.dumps(x),flush=True)
def mcp(server, method, params):
    headers={h['name']:h['value'] for h in server['headers']}
    headers['Content-Type']='application/json'
    body=json.dumps({'jsonrpc':'2.0','id':1,'method':method,'params':params}).encode()
    with urllib.request.urlopen(urllib.request.Request(server['url'],body,headers)) as r:
        return json.loads(r.read())['result']
"#;

    /// One turn: the session opens on the requested model, the CLI asks
    /// before each tool, Luma's tool is allowed and the CLI's own refused,
    /// the call's result comes back through the tool server, and the turn
    /// ends with its usage.
    #[tokio::test]
    async fn tools_permissions_usage_and_completion() {
        let temp = tempfile::tempdir().unwrap();
        let script = format!(
            r#"{CALL_TOOL}
init=read()
assert init['method']=='initialize' and init['params']['clientCapabilities']['terminal']==False
send({{'jsonrpc':'2.0','id':1,'result':{{}}}})
new=read()
assert new['method']=='session/new', new
assert new['params']['_meta']['antigravity']['systemPrompt']['append']=='Use the echo tool.'
server=new['params']['mcpServers'][0]
assert server['type']=='http' and server['name']=='luma'
send({{'jsonrpc':'2.0','id':2,'result':{{'sessionId':'native','models':{{'currentModelId':'carbon-medium'}}}}}})
model=read()
assert model['method']=='session/set_model' and model['params']=={{'sessionId':'native','modelId':'carbon-high'}}, model
send({{'jsonrpc':'2.0','id':3,'result':{{}}}})
prompt=read()
assert prompt['method']=='session/prompt', prompt
assert prompt['params']=={{'sessionId':'native','prompt':[{{'type':'text','text':'Echo hi.'}}]}}, prompt
def update(u): send({{'jsonrpc':'2.0','method':'session/update','params':{{'sessionId':'native','update':u}}}})
def permit(n, name):
    options=[{{'optionId':'allow','kind':'allow_once'}},{{'optionId':'deny','kind':'reject_once'}}]
    send({{'jsonrpc':'2.0','id':n,'method':'session/request_permission','params':{{'toolCall':{{'name':name}},'options':options}}}})
    return read()['result']['outcome']
assert mcp(server,'tools/list',{{}})['tools'][0]['name']=='echo'
update({{'sessionUpdate':'usage_update','used':100,'size':1048576}})
assert permit('1','run_command')=={{'outcome':'selected','optionId':'deny'}}
assert permit('2','mcp_luma_missing')=={{'outcome':'selected','optionId':'deny'}}
assert permit('3','mcp_luma_echo')=={{'outcome':'selected','optionId':'allow'}}
result=mcp(server,'tools/call',{{'name':'echo','arguments':{{'value':'hi'}}}})
assert result=={{'content':[{{'type':'text','text':'hi'}}],'isError':False}}, result
send({{'jsonrpc':'2.0','id':'4','method':'fs/read_text_file','params':{{}}}})
assert read()['error']['code']==-32601
update({{'sessionUpdate':'agent_thought_chunk','content':{{'type':'text','text':'hm'}}}})
update({{'sessionUpdate':'agent_message_chunk','content':{{'type':'text','text':'done'}}}})
send({{'jsonrpc':'2.0','id':prompt['id'],'result':{{'stopReason':'end_turn','usage':{{'inputTokens':90,'outputTokens':10,'cachedReadTokens':40,'cachedWriteTokens':0}}}}}})
"#
        );
        let mut request =
            super::super::tests::request(super::super::Engine::Antigravity, temp.path());
        request.model = Some("carbon".into());
        request.effort = Some("high".into());
        let mut session = Session::connect(request, fake(&script)).await.unwrap();
        let replies = session.replies();
        assert!(matches!(session.next().await.unwrap(),
            Event::Session { id, model } if id == "native" && model.as_deref() == Some("carbon-high")));
        assert!(
            matches!(session.next().await.unwrap(), Event::Step(step) if step.input_tokens == 100)
        );
        assert_eq!(session.context_window, Some(1_048_576));
        let Event::Tool {
            name, input, reply, ..
        } = session.next().await.unwrap()
        else {
            panic!("tool")
        };
        assert_eq!((name.as_str(), &input), ("echo", &json!({"value":"hi"})));
        replies
            .reply(&reply, ToolOutcome::Text("hi".into()))
            .unwrap();
        assert!(matches!(session.next().await.unwrap(), Event::Reasoning(text) if text == "hm"));
        assert!(matches!(session.next().await.unwrap(), Event::Text(text) if text == "done"));
        let Event::Usage(usage) = session.next().await.unwrap() else {
            panic!("usage")
        };
        assert_eq!(
            (
                usage.input_tokens,
                usage.output_tokens,
                usage.cache_read_input_tokens
            ),
            (90, 10, 40)
        );
        assert!(matches!(session.next().await.unwrap(), Event::Done));
    }

    /// A resumed session is opened by id and keeps its model when that is
    /// already the one asked for. A steer waits for the turn's end and is
    /// the next prompt on the same session.
    #[tokio::test]
    async fn a_resumed_session_takes_steers_at_the_turns_end() {
        let temp = tempfile::tempdir().unwrap();
        let script = r#"
import json,sys
read=lambda: json.loads(sys.stdin.readline())
def send(x): print(json.dumps(x),flush=True)
read()
send({'jsonrpc':'2.0','id':1,'result':{}})
resume=read()
assert resume['method']=='session/resume' and resume['params']['sessionId']=='saved', resume
assert '_meta' not in resume['params']
send({'jsonrpc':'2.0','method':'session/update','params':{'update':{'sessionUpdate':'agent_message_chunk','content':{'type':'text','text':'old'}}}})
send({'jsonrpc':'2.0','id':2,'result':{'models':{'currentModelId':'carbon'}}})
first=read()
assert first['method']=='session/prompt' and first['params']['sessionId']=='saved', first
send({'jsonrpc':'2.0','method':'session/update','params':{'update':{'sessionUpdate':'agent_message_chunk','content':{'type':'text','text':'one'}}}})
sys.stdin.readline()
send({'jsonrpc':'2.0','id':first['id'],'result':{'stopReason':'end_turn','usage':{}}})
second=read()
assert second['method']=='session/prompt', second
assert second['params']['prompt']==[{'type':'text','text':'darker'},{'type':'text','text':'and blue'}], second
send({'jsonrpc':'2.0','id':second['id'],'result':{'stopReason':'end_turn','usage':{}}})
"#;
        let mut request =
            super::super::tests::request(super::super::Engine::Antigravity, temp.path());
        request.model = Some("carbon".into());
        request.resume = Some(super::super::state::NativeSession {
            id: "saved".into(),
            usage: Usage::default(),
        });
        let mut session = Session::connect(request, fake(script)).await.unwrap();
        let steerer = session.steerer();
        assert!(
            matches!(session.next().await.unwrap(), Event::Session { id, .. } if id == "saved")
        );
        // What the session said before this turn is not this turn's text.
        assert!(matches!(session.next().await.unwrap(), Event::Text(text) if text == "one"));
        steerer.steer("s1", &["darker".into()]);
        steerer.steer("s2", &["and blue".into()]);
        // The fake holds the turn's end until told the steers are queued.
        session.process.send(json!({})).await.unwrap();
        assert!(matches!(session.next().await.unwrap(), Event::Usage(_)));
        assert!(
            matches!(session.next().await.unwrap(), Event::Steered(ids) if ids == ["s1", "s2"])
        );
        assert!(matches!(session.next().await.unwrap(), Event::Usage(_)));
        assert!(matches!(session.next().await.unwrap(), Event::Done));
    }

    /// With nobody signed in the CLI refuses to open a session, and the turn
    /// fails saying so in words the sign-in prompt recognizes.
    #[tokio::test]
    async fn a_signed_out_cli_fails_the_turn_as_signed_out() {
        let temp = tempfile::tempdir().unwrap();
        let script = r#"
import json,sys
sys.stdin.readline()
print(json.dumps({'jsonrpc':'2.0','id':1,'result':{}}),flush=True)
sys.stdin.readline()
print(json.dumps({'jsonrpc':'2.0','id':2,'error':{'code':-32000,'message':'Authentication required'}}),flush=True)
"#;
        let request = super::super::tests::request(super::super::Engine::Antigravity, temp.path());
        let mut session = Session::connect(request, fake(script)).await.unwrap();
        let error = session.next().await.err().expect("signed out").to_string();
        assert!(super::super::auth::signed_out(&error), "{error}");
    }
}
