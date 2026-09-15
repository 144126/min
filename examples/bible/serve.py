import json
import os
import time
import uuid

import needle
from fastapi import FastAPI
from fastapi.responses import JSONResponse

TOOLS = [
    {
        "name": "search_verses",
        "description": "semantic bible verse search",
        "parameters": {
            "type": "object",
            "properties": {
                "q": {"type": "string", "description": "what to find"},
                "b": {"type": "string", "description": "book name, optional"},
                "x": {"type": "string", "description": "chapter number, optional"},
            },
            "required": ["q"],
        },
    }
]

os.environ.setdefault("NEEDLE_TELEMETRY", "0")
os.environ.setdefault("DO_NOT_TRACK", "1")
agent = needle.Needle(tools=TOOLS)
app = FastAPI()


def last_user(messages):
    for m in reversed(messages or []):
        if m.get("role") == "user":
            c = m.get("content")
            if isinstance(c, str):
                return c
            if isinstance(c, list):
                return "".join(
                    p.get("text", "") for p in c if isinstance(p, dict)
                )
    return ""


def last_tool(messages):
    for m in reversed(messages or []):
        if m.get("role") == "tool":
            c = m.get("content")
            return c if isinstance(c, str) else json.dumps(c)
    return ""


@app.get("/v1/models")
def models():
    return {
        "object": "list",
        "data": [{"id": "needle", "object": "model", "owned_by": "cactus"}],
    }


@app.post("/v1/chat/completions")
def chat(body: dict):
    msgs = body.get("messages") or []
    if any(m.get("role") == "tool" for m in msgs):
        text = last_tool(msgs)
    else:
        text = last_user(msgs)
    out = agent.complete(text)
    calls = out.get("function_calls") or []
    if out.get("type") == "call" and calls:
        tcs = []
        for c in calls:
            tcs.append(
                {
                    "id": f"call_{uuid.uuid4().hex[:12]}",
                    "type": "function",
                    "function": {
                        "name": c.get("name") or "",
                        "arguments": json.dumps(c.get("arguments") or {}),
                    },
                }
            )
        msg = {"role": "assistant", "content": None, "tool_calls": tcs}
        finish = "tool_calls"
    else:
        body_txt = ""
        if any(m.get("role") == "tool" for m in msgs):
            try:
                data = json.loads(last_tool(msgs))
                rows = data.get("r") or []
                lines = []
                for r in rows[:8]:
                    lines.append(f"{r.get('b')} {r.get('c')}:{r.get('v')} {r.get('t')}")
                body_txt = "\n".join(lines)
            except Exception:
                body_txt = last_tool(msgs)
        if not body_txt:
            body_txt = out.get("reasoning") or json.dumps(out)
        msg = {"role": "assistant", "content": body_txt}
        finish = "stop"
    return JSONResponse(
        {
            "id": f"chatcmpl-{uuid.uuid4().hex[:12]}",
            "object": "chat.completion",
            "created": int(time.time()),
            "model": "needle",
            "choices": [
                {"index": 0, "message": msg, "finish_reason": finish}
            ],
            "usage": {"prompt_tokens": 0, "completion_tokens": 0, "total_tokens": 0},
        }
    )
