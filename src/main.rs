use serde_json::{json, Map, Value};
use std::env;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::process;

#[derive(serde::Serialize, serde::Deserialize, Clone)]
struct Msg {
    role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
}

fn new_session() -> String {
    let mut b = [0u8; 16];
    if let Ok(mut f) = fs::File::open("/dev/urandom") {
        let _ = f.read_exact(&mut b);
    }
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7], b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]
    )
}

fn die(s: &str) -> ! {
    eprintln!("{s}");
    process::exit(1);
}

fn read(p: &str) -> String {
    fs::read_to_string(p).unwrap_or_else(|e| die(&format!("{p}: {e}")))
}

fn log_line(path: Option<&str>, s: &str) {
    let Some(p) = path else {
        return;
    };
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(p)
        .unwrap_or_else(|e| die(&format!("{p}: {e}")));
    let _ = writeln!(f, "{s}\n");
}

fn take(m: &mut Map<String, Value>, k: &str) -> Option<Value> {
    m.remove(k)
}

fn expand(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = String::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'$' {
            let start = i + 1;
            let mut j = start;
            while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_') {
                j += 1;
            }
            if j > start {
                let name = &s[start..j];
                out.push_str(&env::var(name).unwrap_or_default());
                i = j;
                continue;
            }
        }
        out.push(b[i] as char);
        i += 1;
    }
    out
}

fn take_headers(m: &mut Map<String, Value>) -> Vec<(String, String)> {
    let Some(v) = take(m, "headers") else {
        return Vec::new();
    };
    let Some(o) = v.as_object() else {
        die("headers must be a yaml map");
    };
    o.iter()
        .map(|(k, v)| {
            let s = match v {
                Value::String(s) => expand(s),
                Value::Number(n) => n.to_string(),
                Value::Bool(b) => b.to_string(),
                _ => die("header values must be strings"),
            };
            (k.clone(), s)
        })
        .collect()
}

fn take_str(m: &mut Map<String, Value>, k: &str) -> Option<String> {
    take(m, k).and_then(|v| match v {
        Value::String(s) => Some(s),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    })
}

fn chat_url(base: &str) -> String {
    let b = base.trim_end_matches('/');
    if b.ends_with("/chat/completions") {
        b.to_string()
    } else {
        format!("{b}/chat/completions")
    }
}

fn enc(s: &str) -> String {
    let mut o = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => o.push(b as char),
            _ => o.push_str(&format!("%{b:02X}")),
        }
    }
    o
}

fn qs(args: &Value) -> String {
    let Some(o) = args.as_object() else {
        return String::new();
    };
    let mut p = Vec::new();
    for (k, v) in o {
        let s = match v {
            Value::Null => continue,
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        p.push(format!("{}={}", enc(k), enc(&s)));
    }
    p.join("&")
}

fn read_res(r: Result<ureq::http::Response<ureq::Body>, ureq::Error>) -> String {
    match r {
        Ok(mut r) => r.body_mut().read_to_string().unwrap_or_default(),
        Err(e) => format!("tool error: {e}"),
    }
}

fn join_url(base: &str, path: &str) -> String {
    if path.starts_with("http://") || path.starts_with("https://") {
        return path.to_string();
    }
    format!("{}{}", base.trim_end_matches('/'), path)
}

fn run_tool(
    exec: &str,
    spec: Option<&Value>,
    arguments: &Value,
    extra: &[(String, String)],
) -> String {
    let method = spec
        .and_then(|s| s.get("method"))
        .and_then(|v| v.as_str())
        .unwrap_or("POST")
        .to_ascii_uppercase();
    let path = spec
        .and_then(|s| s.get("url"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let url = if path.is_empty() { exec } else { path };
    let url_owned;
    let url = if !path.is_empty() && !path.starts_with("http") {
        url_owned = join_url(exec, path);
        url_owned.as_str()
    } else {
        url
    };
    let getish = matches!(method.as_str(), "GET" | "HEAD" | "DELETE");
    let dest = if getish {
        let q = qs(arguments);
        if q.is_empty() {
            url.to_string()
        } else if url.contains('?') {
            format!("{url}&{q}")
        } else {
            format!("{url}?{q}")
        }
    } else {
        url.to_string()
    };
    match method.as_str() {
        "GET" => {
            let mut r = ureq::get(&dest);
            for (k, v) in extra {
                r = r.header(k, v);
            }
            read_res(r.call())
        }
        "HEAD" => {
            let mut r = ureq::head(&dest);
            for (k, v) in extra {
                r = r.header(k, v);
            }
            read_res(r.call())
        }
        "DELETE" => {
            let mut r = ureq::delete(&dest);
            for (k, v) in extra {
                r = r.header(k, v);
            }
            read_res(r.call())
        }
        "PUT" => {
            let mut r = ureq::put(&dest);
            for (k, v) in extra {
                r = r.header(k, v);
            }
            read_res(r.send_json(arguments))
        }
        "PATCH" => {
            let mut r = ureq::patch(&dest);
            for (k, v) in extra {
                r = r.header(k, v);
            }
            read_res(r.send_json(arguments))
        }
        _ => {
            let mut r = ureq::post(&dest);
            for (k, v) in extra {
                r = r.header(k, v);
            }
            read_res(r.send_json(arguments))
        }
    }
}

fn load_tools(v: Value) -> (Value, Map<String, Value>) {
    let raw = match v {
        Value::String(p) => serde_json::from_str(&read(&p)).unwrap_or_else(|e| die(&format!("{p}: {e}"))),
        other => other,
    };
    let mut specs = Map::new();
    let cleaned = match raw {
        Value::Array(items) => Value::Array(
            items
                .into_iter()
                .map(|item| {
                    let mut o = match item {
                        Value::Object(o) => o,
                        other => return other,
                    };
                    let name = o
                        .get("function")
                        .and_then(|f| f.get("name"))
                        .and_then(|n| n.as_str())
                        .unwrap_or("")
                        .to_string();
                    let method = o.remove("method");
                    let tool_url = o.remove("url");
                    if !name.is_empty() {
                        let mut s = Map::new();
                        if let Some(m) = method {
                            s.insert("method".into(), m);
                        }
                        if let Some(u) = tool_url {
                            s.insert("url".into(), u);
                        }
                        specs.insert(name, Value::Object(s));
                    }
                    Value::Object(o)
                })
                .collect(),
        ),
        other => other,
    };
    (cleaned, specs)
}

fn chars(msgs: &[Msg]) -> usize {
    msgs.iter().map(|m| {
        m.role.len()
            + m.content.as_ref().map(|c| c.to_string().len()).unwrap_or(0)
            + m.tool_calls.as_ref().map(|t| t.to_string().len()).unwrap_or(0)
    }).sum()
}

#[derive(Clone)]
struct Cfg {
    url: String,
    headers: Vec<(String, String)>,
    steps: u64,
    keep: usize,
    budget: u64,
    exec: String,
    log: Option<String>,
    map: Map<String, Value>,
    specs: Map<String, Value>,
    inst: Option<String>,
    listen: Option<String>,
    exec_headers: Vec<(String, String)>,
}

fn pull_user(
    inbox: &mut impl FnMut() -> Vec<String>,
    messages: &mut Vec<Msg>,
    emit: &mut impl FnMut(&Value) -> Result<(), String>,
) -> Result<bool, String> {
    let q = inbox();
    if q.is_empty() {
        return Ok(false);
    }
    for p in q {
        emit(&json!({"t": "user", "x": p}))?;
        messages.push(Msg {
            role: "user".into(),
            content: Some(Value::String(p)),
            tool_calls: None,
            tool_call_id: None,
        });
    }
    Ok(true)
}

fn turn(
    cfg: &Cfg,
    agent: &ureq::Agent,
    messages: &mut Vec<Msg>,
    pin: usize,
    extra: &[(String, String)],
    emit: &mut impl FnMut(&Value) -> Result<(), String>,
    inbox: &mut impl FnMut() -> Vec<String>,
) -> Result<String, String> {
    let endpoint = chat_url(&cfg.url);
    let mut used: Option<u64> = None;
    let mut last = String::new();
    for _ in 0..cfg.steps {
        let fat = match used {
            Some(t) => t > cfg.budget,
            None => chars(messages) as u64 > cfg.budget * 3 / 4,
        };
        let mut body = Value::Object(cfg.map.clone());
        body["messages"] = serde_json::to_value(if fat {
            send(pin, cfg.keep, messages)
        } else {
            messages.clone()
        })
        .unwrap();
        let mut req = agent.post(&endpoint);
        let mut has_ct = false;
        for (k, v) in &cfg.headers {
            if k.eq_ignore_ascii_case("content-type") {
                has_ct = true;
            }
            req = req.header(k, v);
        }
        if !has_ct {
            req = req.header("Content-Type", "application/json");
        }
        let mut resp = match req.send_json(&body) {
            Ok(r) => r,
            Err(e) => {
                let extra = e.to_string();
                log_line(cfg.log.as_deref(), &format!("chat fail {extra}"));
                return Err(format!("chat: {extra}"));
            }
        };
        let v: Value = resp
            .body_mut()
            .read_json()
            .map_err(|e| format!("chat json: {e}"))?;
        if let Some(err) = v.get("error") {
            return Err(format!("api: {err}"));
        }
        used = v["usage"]["prompt_tokens"].as_u64();
        let choice = &v["choices"][0];
        let msg = &choice["message"];
        let finish = choice["finish_reason"].as_str().unwrap_or("");
        let tool_calls = &msg["tool_calls"];
        let has_tools = tool_calls.as_array().map(|a| !a.is_empty()).unwrap_or(false);
        log_line(
            cfg.log.as_deref(),
            &format!("chat finish={finish} used={used:?} {msg}"),
        );
        if let Some(t) = msg.get("reasoning_content").and_then(|x| x.as_str()) {
            if !t.is_empty() {
                emit(&json!({"t": "think", "x": t}))?;
            }
        }
        if finish == "tool_calls" || has_tools {
            let calls = tool_calls.as_array().cloned().unwrap_or_default();
            messages.push(Msg {
                role: "assistant".into(),
                content: msg.get("content").cloned(),
                tool_calls: Some(tool_calls.clone()),
                tool_call_id: None,
            });
            for c in calls {
                let id = c["id"].as_str().unwrap_or("");
                let name = c["function"]["name"].as_str().unwrap_or("");
                let args_raw = c["function"]["arguments"].clone();
                let args = match args_raw {
                    Value::String(s) => serde_json::from_str(&s).unwrap_or(json!({})),
                    other => other,
                };
                emit(&json!({"t": "call", "n": name, "a": args}))?;
                let out = run_tool(&cfg.exec, cfg.specs.get(name), &args, extra);
                log_line(
                    cfg.log.as_deref(),
                    &format!("tool {name} {args} -> {out}"),
                );
                emit(&json!({"t": "out", "n": name, "x": out}))?;
                messages.push(Msg {
                    role: "tool".into(),
                    content: Some(Value::String(out)),
                    tool_calls: None,
                    tool_call_id: Some(id.into()),
                });
            }
            let _ = pull_user(inbox, messages, emit)?;
            continue;
        }
        let final_txt = msg["content"].as_str().unwrap_or("").to_string();
        log_line(cfg.log.as_deref(), &format!("out {final_txt}"));
        last = final_txt.clone();
        emit(&json!({"t": "say", "x": final_txt}))?;
        messages.push(Msg {
            role: "assistant".into(),
            content: msg.get("content").cloned(),
            tool_calls: None,
            tool_call_id: None,
        });
        if pull_user(inbox, messages, emit)? {
            continue;
        }
        return Ok(last);
    }
    Err("step limit".into())
}

fn send(pin: usize, keep: usize, msgs: &[Msg]) -> Vec<Msg> {
    let tail = msgs.len().saturating_sub(keep);
    if tail <= pin {
        return msgs.to_vec();
    }
    let mut out = msgs[..pin].to_vec();
    for m in &msgs[pin..tail] {
        if m.role == "tool" {
            out.push(Msg {
                role: "tool".into(),
                content: Some(Value::String(String::new())),
                tool_calls: None,
                tool_call_id: m.tool_call_id.clone(),
            });
        } else {
            out.push(m.clone());
        }
    }
    out.extend_from_slice(&msgs[tail..]);
    out
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let mut cfg_path = "min.yaml".to_string();
    let mut prompt: Vec<String> = Vec::new();
    let mut n = 0;
    while n < args.len() {
        match args[n].as_str() {
            "-c" => {
                n += 1;
                cfg_path = args.get(n).cloned().unwrap_or_else(|| die("-c needs a file"));
            }
            "-h" | "--help" => {
                eprintln!("min [-c min.yaml] [prompt]");
                process::exit(0);
            }
            s if s.starts_with('-') => die(&format!("unknown flag {s}")),
            _ => prompt.push(args[n].clone()),
        }
        n += 1;
    }
    let prompt = prompt.join(" ");

    let raw: Value = serde_yaml::from_str(&read(&cfg_path)).unwrap_or_else(|e| die(&format!("{cfg_path}: {e}")));
    let mut map = match raw {
        Value::Object(m) => m,
        _ => die("config must be a yaml map"),
    };

    let url = take_str(&mut map, "url").unwrap_or_else(|| die("config needs url"));
    take(&mut map, "key");
    let headers = take_headers(&mut map);
    let exec_headers = match take(&mut map, "exec_headers") {
        Some(Value::Object(o)) => o
            .iter()
            .map(|(k, v)| {
                let s = match v {
                    Value::String(s) => expand(s),
                    _ => die("exec_headers values must be strings"),
                };
                (k.clone(), s)
            })
            .collect(),
        None => Vec::new(),
        _ => die("exec_headers must be a yaml map"),
    };
    let steps = take(&mut map, "steps")
        .and_then(|v| v.as_u64())
        .unwrap_or(27);
    let keep = take(&mut map, "keep")
        .and_then(|v| v.as_u64())
        .unwrap_or(9) as usize;
    let window = take(&mut map, "window")
        .and_then(|v| v.as_u64())
        .unwrap_or(1_000_000);
    let budget = take(&mut map, "budget")
        .and_then(|v| v.as_u64())
        .unwrap_or(window * 54 / 100);
    let exec = take_str(&mut map, "exec").unwrap_or_else(|| die("config needs exec"));
    let log_path = take_str(&mut map, "log");
    let listen = take_str(&mut map, "listen");
    let inst = take_str(&mut map, "agents.md").or_else(|| take_str(&mut map, "agents"));
    let mut specs = Map::new();
    if let Some(t) = take(&mut map, "tools") {
        let (cleaned, s) = load_tools(t);
        specs = s;
        map.insert("tools".into(), cleaned);
    }
    take(&mut map, "stream");
    map.remove("messages");

    if map.get("model").and_then(|v| v.as_str()).unwrap_or("").is_empty() {
        die("config needs model");
    }

    let cfg = Cfg {
        url,
        headers,
        steps,
        keep,
        budget,
        exec,
        log: log_path,
        map,
        specs,
        inst,
        listen,
        exec_headers,
    };
    let agent = ureq::Agent::new_with_defaults();

    if let Some(addr) = cfg.listen.clone() {
        serve(cfg, agent, &addr);
        return;
    }
    if prompt.is_empty() {
        die("usage: min [-c min.yaml] [prompt]");
    }
    let mut messages: Vec<Msg> = Vec::new();
    if let Some(p) = &cfg.inst {
        messages.push(Msg {
            role: "system".into(),
            content: Some(Value::String(read(p))),
            tool_calls: None,
            tool_call_id: None,
        });
    }
    log_line(cfg.log.as_deref(), &format!("prompt {prompt}"));
    messages.push(Msg {
        role: "user".into(),
        content: Some(Value::String(prompt)),
        tool_calls: None,
        tool_call_id: None,
    });
    let pin = messages.len();
    match turn(
        &cfg,
        &agent,
        &mut messages,
        pin,
        &cfg.exec_headers,
        &mut |_| Ok(()),
        &mut || Vec::new(),
    ) {
        Ok(s) => println!("{s}"),
        Err(e) => die(&e),
    }
}

fn serve(cfg: Cfg, agent: ureq::Agent, addr: &str) {
    use std::collections::HashMap;
    use std::sync::{mpsc, Arc, Mutex};
    use std::thread;

    let n = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    let (tx, rx) = mpsc::sync_channel::<tiny_http::Request>(n * 4);
    let rx = Arc::new(Mutex::new(rx));
    let sessions: Arc<Mutex<HashMap<String, Sess>>> = Arc::new(Mutex::new(HashMap::new()));
    let cfg = Arc::new(cfg);
    let agent = Arc::new(agent);
    for _ in 0..n {
        let rx = rx.clone();
        let cfg = cfg.clone();
        let agent = agent.clone();
        let sessions = sessions.clone();
        thread::spawn(move || loop {
            let req = {
                let rx = rx.lock().unwrap();
                match rx.recv() {
                    Ok(r) => r,
                    Err(_) => return,
                }
            };
            handle(&cfg, &agent, &sessions, req);
        });
    }
    let binds = listen_binds(addr);
    let mut servers = Vec::new();
    for b in &binds {
        let s = tiny_http::Server::http(b.as_str()).unwrap_or_else(|e| die(&format!("listen {b}: {e}")));
        servers.push((b.clone(), s));
    }
    eprintln!("listen {} workers={n}", binds.join(" "));
    if servers.len() == 1 {
        for req in servers.remove(0).1.incoming_requests() {
            if tx.send(req).is_err() {
                return;
            }
        }
        return;
    }
    let (a_name, a) = servers.remove(0);
    let (b_name, b) = servers.remove(0);
    let tx2 = tx.clone();
    thread::spawn(move || {
        let _ = a_name;
        for req in a.incoming_requests() {
            if tx2.send(req).is_err() {
                break;
            }
        }
    });
    for req in b.incoming_requests() {
        if tx.send(req).is_err() {
            return;
        }
    }
}

fn listen_binds(addr: &str) -> Vec<String> {
    let Some((host, port)) = addr.rsplit_once(':') else {
        return vec![addr.to_string()];
    };
    let host = host.trim_matches(['[', ']']);
    if matches!(host, "127.0.0.1" | "localhost" | "::1") {
        return vec![format!("127.0.0.1:{port}"), format!("[::1]:{port}")];
    }
    vec![addr.to_string()]
}

struct Sess {
    messages: Vec<Msg>,
    inbox: Vec<String>,
    busy: bool,
}

fn handle(
    cfg: &Cfg,
    agent: &ureq::Agent,
    sessions: &std::sync::Mutex<std::collections::HashMap<String, Sess>>,
    mut req: tiny_http::Request,
) {
    let mut raw = String::new();
    if req.as_reader().read_to_string(&mut raw).is_err() {
        let _ = req.respond(tiny_http::Response::from_string("bad body").with_status_code(400));
        return;
    }
    let v: Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            let _ = req.respond(
                tiny_http::Response::from_string(format!("bad json: {e}")).with_status_code(400),
            );
            return;
        }
    };
    let mut session = v
        .get("session")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let prompt = v
        .get("prompt")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    if prompt.is_empty() {
        let _ = req.respond(
            tiny_http::Response::from_string("need prompt").with_status_code(400),
        );
        return;
    }
    if session.is_empty() {
        session = new_session();
    }
    let mut extra = cfg.exec_headers.clone();
    if let Some(o) = v.get("headers").and_then(|x| x.as_object()) {
        for (k, val) in o {
            if let Some(s) = val.as_str() {
                if !s.is_empty() {
                    extra.push((k.clone(), s.to_string()));
                }
            }
        }
    }
    let queued = {
        let mut g = sessions.lock().unwrap();
        let s = g.entry(session.clone()).or_insert_with(|| {
            let mut m = Vec::new();
            if let Some(p) = &cfg.inst {
                m.push(Msg {
                    role: "system".into(),
                    content: Some(Value::String(read(p))),
                    tool_calls: None,
                    tool_call_id: None,
                });
            }
            Sess {
                messages: m,
                inbox: Vec::new(),
                busy: false,
            }
        });
        if s.busy {
            s.inbox.push(prompt.clone());
            true
        } else {
            s.busy = true;
            false
        }
    };
    if queued {
        log_line(cfg.log.as_deref(), &format!("session {session} queue {prompt}"));
        let body = json!({"s": session, "q": true}).to_string();
        let _ = req.respond(
            tiny_http::Response::from_string(body)
                .with_header(
                    tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
                        .unwrap(),
                ),
        );
        return;
    }
    let mut messages = {
        let mut g = sessions.lock().unwrap();
        g.get_mut(&session).map(|s| std::mem::take(&mut s.messages)).unwrap_or_default()
    };
    let pin = if messages.first().map(|m| m.role.as_str()) == Some("system") {
        2
    } else {
        1
    };
    log_line(cfg.log.as_deref(), &format!("session {session} prompt {prompt}"));
    messages.push(Msg {
        role: "user".into(),
        content: Some(Value::String(prompt)),
        tool_calls: None,
        tool_call_id: None,
    });
    let pin = pin.min(messages.len());
    let mut w = req.into_writer();
    let _ = write!(
        w,
        "HTTP/1.1 200 OK\r\nContent-Type: application/x-ndjson\r\nCache-Control: no-cache\r\nTransfer-Encoding: chunked\r\n\r\n",
    );
    let mut chunk = |line: &str| -> Result<(), String> {
        write!(w, "{:x}\r\n{line}\r\n", line.len()).map_err(|e| e.to_string())?;
        w.flush().map_err(|e| e.to_string())
    };
    let start = format!("{}\n", json!({"s": session}));
    if let Err(e) = chunk(&start) {
        let mut g = sessions.lock().unwrap();
        if let Some(s) = g.get_mut(&session) {
            s.messages = messages;
            s.busy = false;
        }
        let _ = e;
        return;
    }
    let sid = session.clone();
    let mut emit = |v: &Value| {
        let line = format!("{v}\n");
        chunk(&line)
    };
    let mut take_inbox = || {
        let mut g = sessions.lock().unwrap();
        g.get_mut(&sid)
            .map(|s| std::mem::take(&mut s.inbox))
            .unwrap_or_default()
    };
    loop {
        let out = turn(cfg, agent, &mut messages, pin, &extra, &mut emit, &mut take_inbox);
        match out {
            Err(e) => {
                let _ = emit(&json!({"t": "err", "x": e}));
                break;
            }
            Ok(_) => {
                if !pull_user(&mut take_inbox, &mut messages, &mut emit).unwrap_or(false) {
                    break;
                }
            }
        }
    }
    let _ = write!(w, "0\r\n\r\n");
    let _ = w.flush();
    drop(w);
    let mut g = sessions.lock().unwrap();
    if let Some(s) = g.get_mut(&session) {
        s.messages = messages;
        s.busy = false;
    }
}
