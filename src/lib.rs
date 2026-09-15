use serde_json::{Map, Value};

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

#[no_mangle]
pub extern "C" fn min_alloc(n: u32) -> u32 {
    let mut v = Vec::<u8>::with_capacity(n as usize);
    let p = v.as_mut_ptr() as u32;
    std::mem::forget(v);
    p
}

#[no_mangle]
pub extern "C" fn min_free(p: u32, n: u32) {
    unsafe {
        let _ = Vec::from_raw_parts(p as *mut u8, 0, n as usize);
    }
}

fn take_bytes(p: u32, n: u32) -> Vec<u8> {
    unsafe { std::slice::from_raw_parts(p as *const u8, n as usize).to_vec() }
}

fn put_bytes(b: &[u8]) -> u64 {
    let n = b.len() as u32;
    let p = min_alloc(n);
    unsafe {
        std::ptr::copy_nonoverlapping(b.as_ptr(), p as *mut u8, b.len());
    }
    ((p as u64) << 32) | (n as u64)
}

#[no_mangle]
pub extern "C" fn min_qs(p: u32, n: u32) -> u64 {
    let raw = take_bytes(p, n);
    let v: Value = serde_json::from_slice(&raw).unwrap_or(Value::Null);
    let Some(o) = v.as_object() else {
        return put_bytes(b"");
    };
    let mut parts = Vec::new();
    for (k, val) in o {
        if val.is_null() {
            continue;
        }
        let s = match val {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        parts.push(format!("{}={}", enc(k), enc(&s)));
    }
    put_bytes(parts.join("&").as_bytes())
}

fn mask(pin: usize, keep: usize, msgs: &[Value]) -> Vec<Value> {
    let tail = msgs.len().saturating_sub(keep);
    if tail <= pin {
        return msgs.to_vec();
    }
    let mut out = msgs[..pin].to_vec();
    for m in &msgs[pin..tail] {
        if m.get("role").and_then(|r| r.as_str()) == Some("tool") {
            let mut o = match m {
                Value::Object(o) => o.clone(),
                _ => Map::new(),
            };
            o.insert("content".into(), Value::String(String::new()));
            out.push(Value::Object(o));
        } else {
            out.push(m.clone());
        }
    }
    out.extend_from_slice(&msgs[tail..]);
    out
}

#[no_mangle]
pub extern "C" fn min_compact(p: u32, n: u32, pin: u32, keep: u32) -> u64 {
    let raw = take_bytes(p, n);
    let v: Value = serde_json::from_slice(&raw).unwrap_or(Value::Null);
    let Some(arr) = v.as_array() else {
        return put_bytes(&raw);
    };
    let out = mask(pin as usize, keep as usize, arr);
    put_bytes(&serde_json::to_vec(&out).unwrap_or_default())
}

#[cfg(target_arch = "wasm32")]
#[link(wasm_import_module = "env")]
extern "C" {
    fn min_fetch(p: u32, n: u32) -> u64;
}

#[cfg(target_arch = "wasm32")]
fn host_fetch(body: &[u8]) -> Vec<u8> {
    let packed = put_bytes(body);
    let p = (packed >> 32) as u32;
    let n = packed as u32;
    let out = unsafe { min_fetch(p, n) };
    min_free(p, n);
    take_bytes((out >> 32) as u32, out as u32)
}

fn peel_tools(v: &Value) -> (Value, Map<String, Value>) {
    let mut specs = Map::new();
    let cleaned = match v {
        Value::Array(items) => Value::Array(
            items
                .iter()
                .cloned()
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
        other => other.clone(),
    };
    (cleaned, specs)
}

fn tool_dest(exec: &str, method: &str, args: &Value) -> String {
    let getish = matches!(method, "GET" | "HEAD" | "DELETE");
    if !getish {
        return exec.to_string();
    }
    let Some(o) = args.as_object() else {
        return exec.to_string();
    };
    let mut parts = Vec::new();
    for (k, val) in o {
        if val.is_null() {
            continue;
        }
        let s = match val {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        parts.push(format!("{}={}", enc(k), enc(&s)));
    }
    if parts.is_empty() {
        return exec.to_string();
    }
    if exec.contains('?') {
        format!("{exec}&{}", parts.join("&"))
    } else {
        format!("{exec}?{}", parts.join("&"))
    }
}

#[cfg(target_arch = "wasm32")]
fn http(method: &str, url: &str, key: &str, body: Option<&Value>) -> Result<Value, String> {
    let mut req = serde_json::Map::new();
    req.insert("method".into(), Value::String(method.into()));
    req.insert("url".into(), Value::String(url.into()));
    let mut headers = serde_json::Map::new();
    if !key.is_empty() {
        headers.insert(
            "Authorization".into(),
            Value::String(format!("Bearer {key}")),
        );
    }
    if body.is_some() {
        headers.insert("Content-Type".into(), Value::String("application/json".into()));
    }
    req.insert("headers".into(), Value::Object(headers));
    if let Some(b) = body {
        req.insert("body".into(), b.clone());
    }
    let raw = host_fetch(&serde_json::to_vec(&Value::Object(req)).unwrap_or_default());
    serde_json::from_slice(&raw).map_err(|e| e.to_string())
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn min_run(p: u32, n: u32) -> u64 {
    let raw = take_bytes(p, n);
    let job: Value = match serde_json::from_slice(&raw) {
        Ok(v) => v,
        Err(e) => return put_bytes(format!("bad job: {e}").as_bytes()),
    };
    let url = job.get("url").and_then(|v| v.as_str()).unwrap_or("");
    let key = job.get("key").and_then(|v| v.as_str()).unwrap_or("");
    let model = job.get("model").and_then(|v| v.as_str()).unwrap_or("");
    let exec = job.get("exec").and_then(|v| v.as_str()).unwrap_or("");
    let inst = job.get("agents").and_then(|v| v.as_str()).unwrap_or("");
    let prompt = job.get("prompt").and_then(|v| v.as_str()).unwrap_or("");
    let steps = job.get("steps").and_then(|v| v.as_u64()).unwrap_or(27);
    let keep = job.get("keep").and_then(|v| v.as_u64()).unwrap_or(9) as usize;
    let window = job.get("window").and_then(|v| v.as_u64()).unwrap_or(1_000_000);
    let budget = job
        .get("budget")
        .and_then(|v| v.as_u64())
        .unwrap_or(window * 54 / 100);
    let (tools, specs) = peel_tools(job.get("tools").unwrap_or(&Value::Null));
    if url.is_empty() || model.is_empty() || prompt.is_empty() {
        return put_bytes(b"job needs url, model, prompt");
    }
    let chat = if url.trim_end_matches('/').ends_with("/chat/completions") {
        url.to_string()
    } else {
        format!("{}/chat/completions", url.trim_end_matches('/'))
    };
    let mut messages = Vec::new();
    if !inst.is_empty() {
        messages.push(serde_json::json!({"role":"system","content":inst}));
    }
    messages.push(serde_json::json!({"role":"user","content":prompt}));
    let pin = messages.len();
    let mut used: Option<u64> = None;
    let mut extra = match job.get("extra") {
        Some(Value::Object(o)) => o.clone(),
        _ => Map::new(),
    };
    extra.remove("stream");
    extra.remove("messages");
    extra.insert("model".into(), Value::String(model.into()));
    if !matches!(tools, Value::Null) {
        extra.insert("tools".into(), tools);
    }
    for _ in 0..steps {
        let fat = match used {
            Some(t) => t > budget,
            None => {
                let packed = serde_json::to_vec(&messages).unwrap_or_default();
                let x = put_bytes(&packed);
                let nchars = min_chars((x >> 32) as u32, x as u32) as u64;
                min_free((x >> 32) as u32, x as u32);
                nchars > budget * 3 / 4
            }
        };
        let send_msgs = if fat {
            mask(pin, keep, &messages)
        } else {
            messages.clone()
        };
        let mut body = extra.clone();
        body.insert("messages".into(), Value::Array(send_msgs));
        let v = match http("POST", &chat, key, Some(&Value::Object(body))) {
            Ok(v) => v,
            Err(e) => return put_bytes(format!("chat: {e}").as_bytes()),
        };
        if let Some(err) = v.get("error") {
            return put_bytes(format!("api: {err}").as_bytes());
        }
        used = v["usage"]["prompt_tokens"].as_u64();
        let choice = &v["choices"][0];
        let msg = &choice["message"];
        let finish = choice["finish_reason"].as_str().unwrap_or("");
        let tool_calls = &msg["tool_calls"];
        let has_tools = tool_calls.as_array().map(|a| !a.is_empty()).unwrap_or(false);
        if finish == "tool_calls" || has_tools {
            let mut asst = serde_json::Map::new();
            asst.insert("role".into(), Value::String("assistant".into()));
            if let Some(c) = msg.get("content") {
                asst.insert("content".into(), c.clone());
            }
            asst.insert("tool_calls".into(), tool_calls.clone());
            messages.push(Value::Object(asst));
            for c in tool_calls.as_array().cloned().unwrap_or_default() {
                let id = c["id"].as_str().unwrap_or("");
                let name = c["function"]["name"].as_str().unwrap_or("");
                let args_raw = c["function"]["arguments"].clone();
                let args = match args_raw {
                    Value::String(s) => serde_json::from_str(&s).unwrap_or(serde_json::json!({})),
                    other => other,
                };
                let method = specs
                    .get(name)
                    .and_then(|s| s.get("method"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("POST")
                    .to_ascii_uppercase();
                let dest = tool_dest(exec, &method, &args);
                let out = if matches!(method.as_str(), "GET" | "HEAD" | "DELETE") {
                    http(&method, &dest, "", None)
                } else {
                    http(&method, &dest, "", Some(&args))
                };
                let text = match out {
                    Ok(v) => v.to_string(),
                    Err(e) => format!("tool error: {e}"),
                };
                messages.push(serde_json::json!({
                    "role":"tool",
                    "content":text,
                    "tool_call_id":id
                }));
            }
            continue;
        }
        let final_txt = msg["content"].as_str().unwrap_or("");
        return put_bytes(final_txt.as_bytes());
    }
    put_bytes(b"step limit")
}

#[no_mangle]
pub extern "C" fn min_chars(p: u32, n: u32) -> u32 {
    let raw = take_bytes(p, n);
    let v: Value = serde_json::from_slice(&raw).unwrap_or(Value::Null);
    let Some(arr) = v.as_array() else {
        return raw.len() as u32;
    };
    arr.iter()
        .map(|m| {
            m.get("role").and_then(|r| r.as_str()).map(|s| s.len()).unwrap_or(0)
                + m.get("content").map(|c| c.to_string().len()).unwrap_or(0)
                + m.get("tool_calls").map(|t| t.to_string().len()).unwrap_or(0)
        })
        .sum::<usize>() as u32
}
