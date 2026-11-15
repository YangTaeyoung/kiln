"""Isolated structured CLI with audited decisions and workspace file effects."""
import json
import os
from pathlib import Path
import subprocess
import sys

provider = os.environ["FIXTURE_PROVIDER"]
mode = os.environ.get("FIXTURE_MODE", "conversation")
session = "fixture-" + provider
turn = 0
pending = None
prompt_id = None


def emit(value):
    print(json.dumps(value), flush=True)


def result(identity, value):
    emit({"jsonrpc": "2.0", "id": identity, "result": value})


def notice(method, params):
    emit({"jsonrpc": "2.0", "method": method, "params": params})


def audit(value):
    with Path("wire.jsonl").open("a", encoding="utf-8") as stream:
        stream.write(json.dumps(value) + "\n")


def finish(status="completed"):
    if provider == "claude":
        emit({"type": "result", "session_id": session, "is_error": False})
    elif provider == "codex":
        notice("turn/completed", {"turn": {"id": str(turn), "status": status}})
    else:
        result(prompt_id, {"stopReason": "cancelled" if status == "interrupted" else "end_turn"})


def ready(identity=None):
    if mode.startswith("exit"):
        for _ in range(1024):
            emit({"type": "fixture_padding", "method": "fixture/padding"})
    if provider == "claude":
        emit({"type": "system", "subtype": "init", "session_id": session, "model": "fixture"})
    elif provider == "codex":
        result(identity, {"thread": {"id": session}, "model": "fixture"})
    else:
        result(identity, {"sessionId": session})
    if mode == "exit-held":
        child = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(30)"])
        Path("descendant.pid").write_text(str(child.pid))
    if mode.startswith("exit"):
        os._exit(0)


def permission(identity):
    if provider == "claude":
        emit({"type": "assistant", "message": {"id": "tool-" + identity, "content": [{"type": "tool_use", "id": identity, "name": "Write", "input": {"file_path": identity + ".txt", "content": "fixture result"}}]}})
        emit({"type": "control_request", "request_id": identity, "request": {"subtype": "can_use_tool", "tool_name": "Write", "tool_use_id": identity, "input": {"file_path": identity + ".txt", "content": "fixture result"}}})
    elif provider == "codex":
        notice("item/started", {"item": {"id": identity, "type": "commandExecution", "command": "fixture write " + identity}})
        emit({"id": identity, "method": "item/commandExecution/requestApproval", "params": {"itemId": identity, "reason": "Write " + identity + ".txt"}})
    else:
        notice("session/update", {"sessionId": session, "update": {"sessionUpdate": "tool_call", "toolCallId": identity, "title": "Write " + identity, "status": "pending", "rawInput": {"file_path": identity + ".txt"}}})
        emit({"id": identity, "method": "session/request_permission", "params": {"sessionId": session, "toolCall": {"toolCallId": identity, "title": "Write " + identity}, "options": [{"optionId": "allow", "name": "Allow", "kind": "allow_once"}, {"optionId": "deny", "name": "Deny", "kind": "reject_once"}]}})


for line in sys.stdin:
    value = json.loads(line)
    audit(value)
    method = value.get("method", "")
    kind = value.get("type", "")
    if provider == "claude" and kind == "control_request":
        subtype = value["request"]["subtype"]
        if subtype == "initialize":
            emit({"type": "control_response", "response": {"subtype": "success", "request_id": value["request_id"], "response": {"models": []}}})
            ready()
        elif subtype == "interrupt":
            pending = None
            finish("interrupted")
    elif method == "initialize":
        result(value["id"], {"protocolVersion": 1, "agentCapabilities": {"loadSession": True}} if provider == "antigravity" else {})
    elif method == "model/list":
        result(value["id"], {"data": []})
    elif method in ("thread/start", "thread/resume", "session/new", "session/load"):
        ready(value["id"])
    elif method in ("turn/interrupt", "session/cancel"):
        pending = None
        if method == "turn/interrupt":
            result(value["id"], {})
        finish("interrupted")
    elif kind == "user" or method in ("turn/start", "session/prompt"):
        turn += 1
        prompt_id = value.get("id")
        if provider == "codex":
            result(value["id"], {"turn": {"id": str(turn)}})
        pending = "operation-" + str(turn)
        permission(pending)
    elif kind == "control_response" or "result" in value:
        identity = value.get("request_id") or value.get("id") or value.get("response", {}).get("request_id")
        if identity != pending:
            audit({"unexpected_decision": identity})
            continue
        if provider == "claude":
            allow = value["response"]["response"]["behavior"] == "allow"
        elif provider == "codex":
            allow = value["result"]["decision"] == "accept"
        else:
            allow = value["result"]["outcome"].get("optionId") == "allow"
        if allow:
            Path(pending + ".txt").write_text("fixture result\n", encoding="utf-8")
        output = "written" if allow else "denied"
        if provider == "claude":
            emit({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": pending, "content": output, "is_error": not allow}]}})
        elif provider == "codex":
            notice("item/completed", {"item": {"id": pending, "type": "commandExecution", "command": "fixture write", "status": "completed" if allow else "declined", "aggregatedOutput": output}})
        else:
            notice("session/update", {"sessionId": session, "update": {"sessionUpdate": "tool_call_update", "toolCallId": pending, "status": "completed" if allow else "failed", "rawOutput": output}})
        pending = None
        finish()
