use serde_json::{json, Map, Value};
use std::env;
use std::fs::{self, OpenOptions};
use std::io::Write;
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
    let _ = writeln!(f, "{s}");
}

fn take(m: &mut Map<String, Value>, k: &str) -> Option<Value> {
    m.remove(k)
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

fn run_tool(exec: &str, spec: Option<&Value>, arguments: &Value) -> String {
    let method = spec
        .and_then(|s| s.get("method"))
        .and_then(|v| v.as_str())
        .unwrap_or("POST")
        .to_ascii_uppercase();
    let url = exec;
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
        "GET" => read_res(ureq::get(&dest).call()),
        "HEAD" => read_res(ureq::head(&dest).call()),
        "DELETE" => read_res(ureq::delete(&dest).call()),
        "PUT" => read_res(ureq::put(&dest).send_json(arguments)),
        "PATCH" => read_res(ureq::patch(&dest).send_json(arguments)),
        _ => read_res(ureq::post(&dest).send_json(arguments)),
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
                    o.remove("url");
                    if !name.is_empty() {
                        let mut s = Map::new();
                        if let Some(m) = method {
                            s.insert("method".into(), m);
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
    let mut i_cli: Option<String> = None;
    let mut t_cli: Option<Value> = None;
    let mut exec_cli: Option<String> = None;
    let mut log_cli: Option<String> = None;
    let mut prompt: Vec<String> = Vec::new();
    let mut n = 0;
    while n < args.len() {
        match args[n].as_str() {
            "-c" => {
                n += 1;
                cfg_path = args.get(n).cloned().unwrap_or_else(|| die("-c needs a file"));
            }
            "-i" => {
                n += 1;
                i_cli = Some(args.get(n).cloned().unwrap_or_else(|| die("-i needs a file")));
            }
            "-t" => {
                n += 1;
                t_cli = Some(Value::String(
                    args.get(n).cloned().unwrap_or_else(|| die("-t needs a file")),
                ));
            }
            "--exec" => {
                n += 1;
                exec_cli = Some(args.get(n).cloned().unwrap_or_else(|| die("--exec needs a url")));
            }
            "-l" => {
                n += 1;
                log_cli = Some(args.get(n).cloned().unwrap_or_else(|| die("-l needs a file")));
            }
            "-h" | "--help" => {
                eprintln!("min [-c min.yaml] [-i agents.md] [-t tools.json] [-l min.log] prompt");
                process::exit(0);
            }
            s if s.starts_with('-') => die(&format!("unknown flag {s}")),
            _ => prompt.push(args[n].clone()),
        }
        n += 1;
    }
    let prompt = prompt.join(" ");
    if prompt.is_empty() {
        die("usage: min [-c min.yaml] [-i agents.md] [-t tools.json] [-l min.log] prompt");
    }

    let raw: Value = serde_yaml::from_str(&read(&cfg_path)).unwrap_or_else(|e| die(&format!("{cfg_path}: {e}")));
    let mut map = match raw {
        Value::Object(m) => m,
        _ => die("config must be a yaml map"),
    };

    let url = take_str(&mut map, "url").unwrap_or_else(|| die("config needs url"));
    let key = take_str(&mut map, "key")
        .or_else(|| env::var("MIN_KEY").ok())
        .unwrap_or_default();
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
    let exec = exec_cli
        .or_else(|| take_str(&mut map, "exec"))
        .unwrap_or_else(|| die("config needs exec"));
    let log_path = log_cli.or_else(|| take_str(&mut map, "log"));
    let inst = i_cli
        .or_else(|| take_str(&mut map, "agents.md"))
        .or_else(|| take_str(&mut map, "agents"));
    let mut specs = Map::new();
    if let Some(t) = t_cli.or_else(|| take(&mut map, "tools")) {
        let (cleaned, s) = load_tools(t);
        specs = s;
        map.insert("tools".into(), cleaned);
    }
    take(&mut map, "stream");
    map.remove("messages");

    if map.get("model").and_then(|v| v.as_str()).unwrap_or("").is_empty() {
        die("config needs model");
    }

    let mut messages: Vec<Msg> = Vec::new();
    if let Some(p) = inst {
        messages.push(Msg {
            role: "system".into(),
            content: Some(Value::String(read(&p))),
            tool_calls: None,
            tool_call_id: None,
        });
    }
    log_line(log_path.as_deref(), &format!("prompt {prompt}"));
    messages.push(Msg {
        role: "user".into(),
        content: Some(Value::String(prompt)),
        tool_calls: None,
        tool_call_id: None,
    });
    let pin = messages.len();
    let mut used: Option<u64> = None;

    let endpoint = chat_url(&url);
    let agent = ureq::Agent::new_with_defaults();

    for _ in 0..steps {
        let fat = match used {
            Some(t) => t > budget,
            None => chars(&messages) as u64 > budget * 3 / 4,
        };
        let mut body = Value::Object(map.clone());
        body["messages"] = serde_json::to_value(if fat {
            send(pin, keep, &messages)
        } else {
            messages.clone()
        }).unwrap();

        let mut req = agent.post(&endpoint);
        if !key.is_empty() {
            req = req.header("Authorization", &format!("Bearer {key}"));
        }
        let mut resp = req
            .header("Content-Type", "application/json")
            .send_json(&body)
            .unwrap_or_else(|e| die(&format!("chat: {e}")));
        let v: Value = resp
            .body_mut()
            .read_json()
            .unwrap_or_else(|e| die(&format!("chat json: {e}")));
        if let Some(err) = v.get("error") {
            die(&format!("api: {err}"));
        }
        used = v["usage"]["prompt_tokens"].as_u64();
        let choice = &v["choices"][0];
        let msg = &choice["message"];
        let finish = choice["finish_reason"].as_str().unwrap_or("");
        let tool_calls = &msg["tool_calls"];
        let has_tools = tool_calls.as_array().map(|a| !a.is_empty()).unwrap_or(false);
        log_line(
            log_path.as_deref(),
            &format!("chat finish={finish} used={used:?} {msg}"),
        );

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
                let out = run_tool(&exec, specs.get(name), &args);
                log_line(
                    log_path.as_deref(),
                    &format!("tool {name} {args} -> {out}"),
                );
                messages.push(Msg {
                    role: "tool".into(),
                    content: Some(Value::String(out)),
                    tool_calls: None,
                    tool_call_id: Some(id.into()),
                });
            }
            continue;
        }

        let final_txt = msg["content"].as_str().unwrap_or("");
        log_line(log_path.as_deref(), &format!("out {final_txt}"));
        println!("{final_txt}");
        return;
    }
    die("step limit");
}
