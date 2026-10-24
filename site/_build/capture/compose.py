import json, os, subprocess, sys, time
D = os.environ.get("KILN_DEMO_DIR", "/tmp/kd"); A = f"{D}/home/dev/acme"; API = f"{A}/services/api"
env = dict(os.environ)
def kiln(*a): return subprocess.run(["kiln", *a], capture_output=True, text=True, env=env).stdout.strip()

def sessions():
    path = f"{D}/sessions.json"
    if os.path.exists(path): return json.load(open(path))
    s = {
        "claude": kiln("new", "--cwd", A, "--name", "Connect the frontend and API", "claude", f"{D}/scenes/claude.txt"),
        "codex": kiln("new", "--cwd", A, "--name", "Review the integration tests", "codex", f"{D}/scenes/codex.txt"),
        "done": kiln("new", "--cwd", API, "--name", "Fix flaky checkout retry", "claude", f"{D}/scenes/done.txt"),
        "agy": kiln("new", "--cwd", A, "--name", "Draft the release notes", "agy", f"{D}/scenes/agy.txt"),
        "shell": kiln("new", "--cwd", A),
        "fail": kiln("new", "--cwd", API, "--name", "Bump the Rust toolchain", "codex", f"{D}/scenes/fail.txt"),
    }
    s = {k: int(v.split()[-1]) for k, v in s.items()}
    json.dump(s, open(path, "w")); return s

def pane(i, session=None, cwd=A, tool=None): return {"id": i, "session": session, "cwd": cwd, "tool": tool}
def page(root, panes, focused, title=None):
    return {"manual_split": False, "zoomed": None, "root": root, "focused": focused, "panes": panes, "title": title, "agent_request": None, "agent_request_offset": 0}
def split(d, r, a, b): return {"Split": {"dir": d, "ratio": r, "a": a, "b": b}}
def leaf(i): return {"Leaf": i}

CONN = 7001
SQL = "SELECT o.id, o.status, p.amount, p.provider_ref\nFROM orders o\nJOIN payments p ON p.order_id = o.id\nWHERE o.status = 'pending'\nORDER BY o.id;"

def build(active_page, sheet=None, active_ws=0):
    s = sessions()
    acme_pages = [
        page(split("Horizontal", 0.5, leaf(1), split("Vertical", 0.56, leaf(2), leaf(10))), [pane(1, s["claude"]), pane(2, s["codex"]), pane(10, s["agy"])], 2, "Connect the frontend and API"),
        page(leaf(3), [pane(3, None, API, {"RepositoryHistory": {"root": API}})], 3),
        page(leaf(4), [pane(4, None, A, {"DbTable": {"conn": CONN, "schema": None, "table": "orders"}})], 4),
        page(split("Horizontal", 0.56, leaf(6), leaf(7)), [pane(6, None, API, {"Editor": {"path": f"{API}/payment/validate.go"}}), pane(7, None, API, {"Diff": {"root": API, "path": "payment/handler.go", "staged": False}})], 6),
    ]
    acme_pages.append(page(split("Horizontal", 0.5, leaf(11), leaf(12)), [pane(11, s["shell"], A), pane(12, None, API, {"Editor": {"path": f"{API}/payment/handler.go"}})], 11, "Terminal"))
    review_pages = [
        page(leaf(8), [pane(8, s["done"], API)], 8, "Fix flaky checkout retry"),
        page(leaf(9), [pane(9, s["fail"], API)], 9, "Bump the Rust toolchain"),
    ]
    old = json.load(open(f"{D}/config/state.json"))
    drafts = {"agent_prompt": "", "repositories": {}, "commit_message": "", "github": {"repositories": {}, "selected": None}, "remote": []}
    ws = [
        {"conversation_sync": False, "name": "Acme", "root": A, "pages": acme_pages, "active_page": active_page, "sheet": sheet, "last_inspector": sheet or "git", "drafts": drafts},
        {"conversation_sync": False, "name": "API review", "root": API, "pages": review_pages, "active_page": 0, "sheet": None, "last_inspector": "git", "drafts": drafts},
    ]
    old["workspaces"] = ws; old["active"] = active_ws
    json.dump(old, open(f"{D}/config/state.json", "w"), indent=1)
    json.dump([
        {"id": "r1", "name": "acme-assets", "endpoint": {"S3": {"bucket": "acme-assets", "region": "us-east-1", "endpoint": None, "path_style": False, "prefix": "", "aws_profile": "acme", "aws_auth": "Default"}}},
        {"id": "r2", "name": "staging-api", "endpoint": {"Sftp": {"alias": "staging-api", "config_path": None, "root": "/srv/acme"}}},
        {"id": "r3", "name": "release-cdn", "endpoint": {"Ftp": {"host": "ftp.acme.test", "port": 21, "username": "deploy", "tls": True, "root": "/public"}}},
    ], open(f"{D}/config/remote-connections.json", "w"), indent=1)
    json.dump({"connections": [{"id": CONN, "name": "acme-local", "driver": "Sqlite", "host": "", "port": 0, "user": "", "database": "", "file": f"{A}/acme.db", "ssl_mode": "Disable", "color": None, "connect_timeout_secs": 5, "save_password_in_file": False}]}, open(f"{D}/config/db_connections.json", "w"), indent=1)

if __name__ == "__main__":
    build(int(sys.argv[1]), None if sys.argv[2] == "-" else sys.argv[2], int(sys.argv[3]) if len(sys.argv) > 3 else 0)
    print(json.load(open(f"{D}/sessions.json")))
