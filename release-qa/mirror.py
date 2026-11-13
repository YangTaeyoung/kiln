import http.server
import pathlib
import sys

root = pathlib.Path(sys.argv[1]).resolve()
class Mirror(http.server.SimpleHTTPRequestHandler):
    def __init__(self, *args, **kwargs):
        super().__init__(*args, directory=root, **kwargs)
    def do_GET(self):
        if self.path == '/releases/latest':
            self.send_response(302)
            self.send_header('Location', '/releases/tag/v0.2.24')
            self.end_headers()
        elif self.path == '/releases/tag/v0.2.24':
            self.send_response(200)
            self.end_headers()
        else:
            self.path = self.path.removeprefix('/releases/download/v0.2.24')
            super().do_GET()

with http.server.ThreadingHTTPServer(('127.0.0.1', 0), Mirror) as server:
    (root / 'port').write_text(str(server.server_port))
    server.serve_forever()
