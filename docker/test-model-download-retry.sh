#!/usr/bin/env bash

set -euo pipefail

repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly repository_root
test_root="$(mktemp -d "${TMPDIR:-/tmp}/sbol-db-download-retry.XXXXXX")"
server_pid=""

cleanup() {
  if [ -n "$server_pid" ]; then
    kill "$server_pid" >/dev/null 2>&1 || true
    wait "$server_pid" >/dev/null 2>&1 || true
  fi
  rm -rf "$test_root"
}
trap cleanup EXIT

payload="$test_root/payload"
port_file="$test_root/port"
request_count_file="$test_root/request-count"
target="$test_root/downloaded"
printf 'checksum-pinned retry fixture\n' > "$payload"

python3 - "$payload" "$port_file" "$request_count_file" <<'PY' &
import socket
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

payload = Path(sys.argv[1]).read_bytes()
port_file = Path(sys.argv[2])
request_count_file = Path(sys.argv[3])


class Handler(BaseHTTPRequestHandler):
    requests = 0

    def do_GET(self):
        Handler.requests += 1
        request_count_file.write_text(str(Handler.requests))
        self.send_response(200)
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        if Handler.requests == 1:
            self.wfile.write(payload[: len(payload) // 2])
            self.wfile.flush()
            self.connection.shutdown(socket.SHUT_RDWR)
            self.connection.close()
            return
        self.wfile.write(payload)

    def log_message(self, _format, *_args):
        pass


server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
port_file.write_text(str(server.server_port))
server.serve_forever()
PY
server_pid="$!"

for _attempt in $(seq 1 100); do
  if [ -s "$port_file" ]; then
    break
  fi
  sleep 0.05
done
test -s "$port_file"

# Source the production helper so this exercises the exact curl flags and
# checksum/atomic-rename path used by the FAISS container lifecycle test.
source "$repository_root/docker/fetch-test-embedding-model.sh"
expected="$(sha256 "$payload")"
SBOL_DB_DOWNLOAD_RETRY_DELAY_SECONDS=0 download_verified \
  "http://127.0.0.1:$(<"$port_file")/model" \
  "$target" \
  "$expected"

cmp "$payload" "$target"
test "$(<"$request_count_file")" -ge 2
test ! -e "$target.partial"

requests_after_download="$(<"$request_count_file")"
download_verified \
  "http://127.0.0.1:$(<"$port_file")/model" \
  "$target" \
  "$expected"
test "$(<"$request_count_file")" -eq "$requests_after_download"
echo "verified model download retry passed"
