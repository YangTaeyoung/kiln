#!/usr/bin/env python3
"""Loopback GitHub-release-layout mirror, adapted from the 0.2.15 verifier."""
import argparse
from functools import partial
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from io import BytesIO
from pathlib import Path
import re
from urllib.parse import urlsplit


def make_handler(directory: Path, version: str):
    allowed = {f"Kiln-{version}-windows-{arch}.zip" for arch in ("x64", "arm64")}
    allowed.add("SHA256SUMS.txt")
    prefix = f"/releases/download/v{version}/"

    class Handler(SimpleHTTPRequestHandler):
        def send_head(self):
            path = urlsplit(self.path).path
            if path == "/releases/latest":
                self.send_response(302)
                self.send_header("Location", f"/releases/tag/v{version}")
                self.end_headers()
                return None
            if path == f"/releases/tag/v{version}":
                body = f"Kiln {version} verification mirror".encode("ascii")
                self.send_response(200)
                self.send_header("Content-Type", "text/plain; charset=ascii")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                return BytesIO(body)
            if not path.startswith(prefix) or path[len(prefix):] not in allowed:
                self.send_error(404)
                return None
            self.path = "/" + path[len(prefix):]
            return super().send_head()

    return partial(Handler, directory=str(directory))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    parser.add_argument("port_file", type=Path)
    parser.add_argument("--version", default="0.2.17")
    args = parser.parse_args()
    if not re.fullmatch(r"\d+\.\d+\.\d+", args.version):
        parser.error("version must be a stable numeric version")
    if not args.directory.is_dir():
        parser.error("archive directory does not exist")
    with ThreadingHTTPServer(("127.0.0.1", 0), make_handler(args.directory, args.version)) as server:
        args.port_file.write_text(str(server.server_port), encoding="ascii")
        print("mirror port", server.server_port, flush=True)
        server.serve_forever()


if __name__ == "__main__":
    main()
