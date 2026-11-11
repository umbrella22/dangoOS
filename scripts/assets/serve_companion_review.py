#!/usr/bin/env python3
"""Serve the offline art review locally without stale WebView image responses."""
import argparse
import functools
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path


class ReviewHandler(SimpleHTTPRequestHandler):
    def end_headers(self):
        self.send_header('Cache-Control', 'no-store')
        super().end_headers()

    def do_GET(self):
        # Some embedded WebViews lose cached lazy-loaded images after a source
        # switch. A complete local response avoids reusing an empty 304 entry.
        if 'If-Modified-Since' in self.headers:
            del self.headers['If-Modified-Since']
        try:
            super().do_GET()
        except (BrokenPipeError, ConnectionResetError):
            pass  # Switching the selected pose cancels the previous video.


class ReviewServer(ThreadingHTTPServer):
    request_queue_size = 64


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--directory', type=Path, required=True)
    parser.add_argument('--port', type=int, default=8768)
    args = parser.parse_args()
    handler = functools.partial(ReviewHandler, directory=str(args.directory.resolve()))
    ReviewServer(('127.0.0.1', args.port), handler).serve_forever()
