#!/usr/bin/python3
"""Owned CLI boundary for the real daemon/GUI handover integration test."""
import datetime
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import uuid

tool = Path(sys.argv[0]).name
args = sys.argv[1:]
if args == ["--version"]:
    print("0.162.1" if tool == "codex" else "2.1.299")
    sys.exit(0)
root = Path(os.environ["CODEX_HOME" if tool == "codex" else "CLAUDE_CONFIG_DIR"])
config = Path(os.environ["KILN_CONFIG_DIR"])

def transcript(cid):
    return next(root.rglob("*" + cid + ".jsonl"))

if "app-server" in args:
    names = {}
    for line in sys.stdin:
        request = json.loads(line)
        if "id" not in request:
            continue
        method, params = request["method"], request.get("params", {})
        result = {}
        if method == "thread/read":
            cid = params["threadId"]
            path = transcript(cid)
            meta = json.loads(path.read_text().splitlines()[0])["payload"]
            result = {"thread": {"id": cid, "path": str(path), "cwd": meta["cwd"], "name": names.get(cid, "Shared fixture conversation")}}
        elif method == "thread/name/set":
            names[params["threadId"]] = params["name"]
        elif method == "thread/list":
            result = {"data": [], "nextCursor": None}
        elif method == "thread/archive":
            path = transcript(params["threadId"])
            destination = root / "archived_sessions" / path.name
            destination.parent.mkdir(parents=True, exist_ok=True)
            path.rename(destination)
        elif method != "initialize":
            raise RuntimeError("Unexpected metadata operation: " + method)
        print(json.dumps({"id": request["id"], "result": result}), flush=True)
    sys.exit(0)

unrelated = Path.cwd().name == "unrelated"
if unrelated:
    cid, path = "33333333-1234-1234-1234-123456789abc", None
elif "resume" in args or "--resume" in args:
    flag = "resume" if "resume" in args else "--resume"
    cid = args[args.index(flag) + 1]
    path = transcript(cid)
elif tool == "codex":
    cid = "11111111-1234-1234-1234-123456789abc"
    path = transcript(cid)
else:
    path = next((root / "projects").glob("*/*.jsonl"))
    cid = json.loads(path.read_text().splitlines()[0])["sessionId"]

def hook(event):
    payload = {"hook_event_name": event, "session_id": cid, "cwd": str(Path.cwd())}
    if path:
        payload["transcript_path"] = str(path)
    result = subprocess.run([os.environ["KILN_EXE"], "agent-hook"], input=json.dumps(payload), text=True, capture_output=True, check=True)
    return json.loads(result.stdout) if result.stdout.strip() else None

def stop(*_):
    if tool == "codex" and not unrelated:
        count_path = config / "fixture-codex-exits"
        count = int(count_path.read_text()) + 1 if count_path.exists() else 1
        count_path.write_text(str(count))
        now = datetime.datetime.now(datetime.timezone.utc).isoformat()
        with path.open("a") as output:
            for role, text in [("user", "Final question " + str(count)), ("assistant", "Last answer before transfer " + str(count))]:
                if role == "user":
                    output.write(json.dumps({"timestamp": now, "type": "event_msg", "payload": {"type": "user_message", "message": text, "images": [], "local_images": []}}) + "\n")
                output.write(json.dumps({"timestamp": now, "type": "response_item", "payload": {"type": "message", "id": str(uuid.uuid4()), "role": role, "content": [{"type": "input_text" if role == "user" else "output_text", "text": text}]}}) + "\n")
            output.write(json.dumps({"timestamp": now, "type": "event_msg", "payload": {"type": "task_complete", "turn_id": str(uuid.uuid4())}}) + "\n")
    sys.exit(0)

signal.signal(signal.SIGINT, stop)
hook("SessionStart")
history = path.read_text() if path else ""
with (config / "fixture-launches.jsonl").open("a") as output:
    output.write(json.dumps({"tool": tool, "id": cid, "generation": os.environ["KILN_AGENT_GENERATION"], "pid": os.getpid(), "resumed": "resume" in args or "--resume" in args, "final_two": "Last answer before transfer 2" in history, "unrelated": unrelated}) + "\n")
print("HANDOVER_READY " + cid, flush=True)
for line in sys.stdin:
    decision = hook("UserPromptSubmit")
    print("BLOCKED" if decision else "PROMPT_ACCEPTED", flush=True)
