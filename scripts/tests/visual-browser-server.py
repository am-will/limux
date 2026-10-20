#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.12"
# dependencies = []
# ///
"""Serve a loopback website and validate its WebKit load callback."""

import argparse
import http.server
import json
from pathlib import Path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--url-file", type=Path, required=True)
    parser.add_argument("--proof-file", type=Path, required=True)
    args = parser.parse_args()
    page = Path(__file__).with_name("visual-browser.html").read_bytes()

    class Handler(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            if self.path != "/":
                self.send_error(404)
                return
            self.send_response(200)
            self.send_header("Content-Type", "text/html; charset=utf-8")
            self.send_header("Content-Length", str(len(page)))
            self.end_headers()
            self.wfile.write(page)

        def do_POST(self):
            try:
                length = int(self.headers.get("Content-Length", "0"))
                if self.path != "/ready" or not 0 < length < 4096:
                    raise ValueError("invalid callback request")
                payload = json.loads(self.rfile.read(length))
                if payload != {
                    "url": url,
                    "ready": "complete",
                    "text": "JavaScript updated this page: 42",
                }:
                    raise ValueError("unexpected browser state")
            except (ValueError, UnicodeDecodeError):
                self.send_error(400)
                return
            proof = args.proof_file.with_suffix(".tmp")
            proof.write_text(json.dumps(payload) + "\n")
            proof.replace(args.proof_file)
            self.send_response(204)
            self.end_headers()

    with http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler) as server:
        server.daemon_threads = True
        url = f"http://127.0.0.1:{server.server_port}/"
        args.url_file.write_text(url + "\n")
        server.serve_forever()


if __name__ == "__main__":
    main()
