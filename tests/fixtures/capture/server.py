#!/usr/bin/env python3
"""Captures raw multipart bodies: POST /<name> stores <name>.body and <name>.ct."""
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path

OUT = Path(sys.argv[1])
PORT = int(sys.argv[2])
EXPECTED = int(sys.argv[3])
count = 0


class H(BaseHTTPRequestHandler):
    def do_POST(self):
        global count
        n = int(self.headers.get("Content-Length", "0"))
        body = self.rfile.read(n)
        name = self.path.strip("/")
        (OUT / f"{name}.body").write_bytes(body)
        (OUT / f"{name}.ct").write_bytes(self.headers["Content-Type"].encode())
        self.send_response(200)
        self.send_header("Content-Length", "2")
        self.end_headers()
        self.wfile.write(b"ok")
        count += 1

    def log_message(self, *a):
        pass


srv = HTTPServer(("127.0.0.1", PORT), H)
while count < EXPECTED:
    srv.handle_request()
