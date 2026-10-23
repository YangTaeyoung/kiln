from http.server import ThreadingHTTPServer,SimpleHTTPRequestHandler
from pathlib import Path
import os,sys
os.chdir(sys.argv[1])
class Handler(SimpleHTTPRequestHandler):
 def do_GET(self):
  if self.path=='/releases/latest':
   self.send_response(302);self.send_header('Location','/releases/tag/v0.2.12');self.end_headers();return
  if self.path=='/releases/tag/v0.2.12':
   self.send_response(200);self.end_headers();self.wfile.write(b'Kiln 0.2.12 verification mirror');return
  prefix='/releases/download/v0.2.12/'
  if self.path.startswith(prefix):self.path='/'+self.path[len(prefix):]
  else:self.send_error(404);return
  return super().do_GET()
server=ThreadingHTTPServer(('127.0.0.1',0),Handler)
Path(sys.argv[2]).write_text(str(server.server_port))
print('mirror port',server.server_port,flush=True);server.serve_forever()
