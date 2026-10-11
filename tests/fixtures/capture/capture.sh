#!/usr/bin/env bash
# Regenerates the real-client fixtures. Requires curl, python3 + requests, node >= 18.
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
OUT="$HERE/.."
PORT=18927
TMP=$(mktemp -d)
printf 'hello\r\nworld\r\n--\r\n--not-a-boundary\r\n\x00\x01\xff binary' > "$TMP/upload.bin"
printf 'line1\nline2\n' > "$TMP/notes.txt"
python3 "$HERE/server.py" "$OUT" "$PORT" 5 &
SRV=$!
sleep 0.5
U="http://127.0.0.1:$PORT"
# 1. curl: text field, binary file with explicit type, file with a quote and unicode in its name.
cp "$TMP/upload.bin" "$TMP/fïle \"q\".bin"
curl -sS -o /dev/null -F 'field=plain text' -F "firmware=@$TMP/upload.bin;type=application/octet-stream" -F "odd=@$TMP/fïle \"q\".bin" "$U/curl"
# 2. curl: empty field value and a field with CR/LF and a unicode name.
curl -sS -o /dev/null -F 'empty=' -F $'multi=a\r\nb' -F 'ünïcode=ü' "$U/curl_edge"
# 3. Python requests.
python3 - "$U" "$TMP" <<'PY'
import sys, requests
u, tmp = sys.argv[1], sys.argv[2]
requests.post(u + "/requests", data={"field": "plain text", "empty": ""},
              files={"firmware": ("upload.bin", open(tmp + "/upload.bin", "rb"), "application/octet-stream"),
                     "notes": ("nötes \"q\".txt", open(tmp + "/notes.txt", "rb"), "text/plain")})
PY
# 4. Node FormData (undici).
node - "$U" "$TMP" <<'JS'
const [u, tmp] = process.argv.slice(2);
const fs = require("fs");
(async () => {
  const fd = new FormData();
  fd.append("field", "plain text");
  fd.append("empty", "");
  fd.append("firmware", new Blob([fs.readFileSync(tmp + "/upload.bin")], { type: "application/octet-stream" }), "upload.bin");
  fd.append("notes", new Blob([fs.readFileSync(tmp + "/notes.txt")], { type: "text/plain" }), 'nötes "q".txt');
  fd.append("multi", "a\r\nb");
  await fetch(u + "/node", { method: "POST", body: fd });
  // Second request: no files at all.
  const fd2 = new FormData();
  fd2.append("a", "1");
  fd2.append("b", "2");
  await fetch(u + "/node_fields", { method: "POST", body: fd2 });
})();
JS
wait $SRV
rm -rf "$TMP"
ls -la "$OUT"/*.body
