#!/usr/bin/env python3
"""Resumable OCI blob upload client for the registry chaos test (stdlib only).

  oci_chunked_upload.py BASE_URL TOKEN NAME SIZE_MB CHUNK_MB RATE_MBPS EVENTS_FILE

Uploads SIZE_MB of random data to {BASE}/v2/{NAME}/blobs/uploads/ as chunked PATCHes
(Content-Range), pacing the body at RATE_MBPS so that pod kills land mid-chunk. On any error
(reset, timeout, 5xx) it does what a registry client is expected to do: GET the upload
session for its current Range and resume from there (a chunk the server did not commit is
re-sent; one it did commit is not). The final PUT ?digest= is retried the same way; if the
session is gone after a PUT whose response was lost, a HEAD of the blob decides. Then the blob
is downloaded 3 times and its sha256 compared.

Prints one JSON line: {"ok", "digest", "size", "chunks", "retries", "resumed_from", "restarts",
"elapsed_s", "verify": [...]}. Appends "chunk N" progress lines to EVENTS_FILE so the driver
can time its kills.
"""
import hashlib, http.client, json, os, socket, sys, time, urllib.parse

base, token, name = sys.argv[1].rstrip("/"), sys.argv[2], sys.argv[3]
size_mb, chunk_mb, rate = float(sys.argv[4]), float(sys.argv[5]), float(sys.argv[6])
events = sys.argv[7]
u = urllib.parse.urlparse(base)
HOST, PORT = u.hostname, u.port or 80
AUTH = {"Authorization": "token " + token}
DEADLINE = time.time() + 600

def ev(msg):
    with open(events, "a") as f:
        f.write(f"{time.time():.3f} {msg}\n")

def conn():
    return http.client.HTTPConnection(HOST, PORT, timeout=20)

def req(method, path, body=None, headers=None, paced=False):
    """Returns (status, headers dict lowercased, body bytes). Raises on transport errors."""
    c = conn()
    h = dict(AUTH); h.update(headers or {})
    try:
        if body is not None and paced:
            c.putrequest(method, path)
            for k, v in h.items():
                c.putheader(k, v)
            c.putheader("Content-Length", str(len(body)))
            c.endheaders()
            step = 64 * 1024
            per = step / (rate * 1024 * 1024)
            for i in range(0, len(body), step):
                c.send(body[i:i + step])
                time.sleep(per)
        else:
            c.request(method, path, body=body, headers=h)
        r = c.getresponse()
        data = r.read()
        return r.status, {k.lower(): v for k, v in r.getheaders()}, data
    finally:
        c.close()

def rel(loc):
    p = urllib.parse.urlparse(loc)
    return p.path + (("?" + p.query) if p.query else "")

def range_end(hdrs, known):
    r = hdrs.get("range", "")
    if "-" not in r:
        return known
    end = int(r.split("-")[1])
    # "0-0" is what the registry answers for an empty session; we never upload 1-byte chunks.
    return 0 if end == 0 else end + 1

data = os.urandom(int(size_mb * 1024 * 1024))
digest = "sha256:" + hashlib.sha256(data).hexdigest()
chunk = int(chunk_mb * 1024 * 1024)
t0 = time.time()
stats = {"retries": 0, "resumed_from": [], "restarts": 0, "chunks": 0, "errors": []}

def note_err(where, e):
    stats["retries"] += 1
    if len(stats["errors"]) < 20:
        stats["errors"].append(f"{where}: {e}")
    ev(f"error {where} {e}")
    time.sleep(1)

def retry(fn, where):
    while time.time() < DEADLINE:
        try:
            return fn()
        except (OSError, http.client.HTTPException, socket.timeout) as e:
            note_err(where, repr(e))
    raise SystemExit(json.dumps({"ok": False, "error": f"deadline in {where}", **stats}))

def start():
    s, h, b = req("POST", f"/v2/{name}/blobs/uploads/")
    if s != 202:
        raise OSError(f"POST uploads -> {s} {b[:200]!r}")
    return rel(h["location"])

def status(loc, known):
    s, h, b = req("GET", loc)
    if s == 404:
        return None
    if s != 204:
        raise OSError(f"GET upload -> {s}")
    return range_end(h, known)

def blob_ok():
    s, h, _ = req("HEAD", f"/v2/{name}/blobs/{digest}")
    return s == 200

done = False
while not done and time.time() < DEADLINE:
    loc = retry(start, "start")
    off = 0
    ev("start")
    lost = False
    while off < len(data):
        piece = data[off:off + chunk]
        try:
            s, h, b = req("PATCH", loc, body=piece, paced=True, headers={
                "Content-Type": "application/octet-stream",
                "Content-Range": f"{off}-{off + len(piece) - 1}"})
            if s == 202:
                loc = rel(h.get("location", loc)); off = range_end(h, off + len(piece))
                stats["chunks"] += 1; ev(f"chunk {stats['chunks']} off={off}")
                continue
            if s == 416:
                off = range_end(h, off); continue
            raise OSError(f"PATCH -> {s} {b[:200]!r}")
        except (OSError, http.client.HTTPException, socket.timeout) as e:
            note_err("patch", repr(e))
            cur = retry(lambda: status(loc, off), "status")
            if cur is None:  # session lost: start over
                stats["restarts"] += 1; lost = True; break
            stats["resumed_from"].append(cur); off = cur
    if lost:
        continue
    # finish
    while time.time() < DEADLINE:
        try:
            sep = "&" if "?" in loc else "?"
            s, h, b = req("PUT", f"{loc}{sep}digest={urllib.parse.quote(digest)}", body=b"",
                          headers={"Content-Type": "application/octet-stream"})
            if s == 201:
                if h.get("docker-content-digest") != digest:
                    raise SystemExit(json.dumps({"ok": False, "error": "digest header mismatch", **stats}))
                done = True; break
            if s == 404 and retry(blob_ok, "head"):
                done = True; break
            if s == 404:
                stats["restarts"] += 1; break
            if s == 400:  # digest mismatch: the server's view of the content is wrong
                raise SystemExit(json.dumps({"ok": False, "error": f"PUT -> 400 {b[:300]!r}", **stats}))
            raise OSError(f"PUT -> {s} {b[:200]!r}")
        except (OSError, http.client.HTTPException, socket.timeout) as e:
            note_err("put", repr(e))
            if retry(blob_ok, "head"):
                done = True; break
            cur = retry(lambda: status(loc, off), "status")
            if cur is None:
                stats["restarts"] += 1; break
            if cur != len(data):  # should not happen: PUT carries no body
                stats["restarts"] += 1; break
elapsed = time.time() - t0
ev("finished")
verify = []
for _ in range(3):
    def get():
        s, h, b = req("GET", f"/v2/{name}/blobs/{digest}")
        return s, hashlib.sha256(b).hexdigest() if s == 200 else None, len(b)
    s, hx, n = retry(get, "verify")
    verify.append(s == 200 and "sha256:" + hx == digest and n == len(data))
print(json.dumps({"ok": done and all(verify), "digest": digest, "size": len(data), "elapsed_s": round(elapsed, 1),
                  "verify": verify, **stats}))
