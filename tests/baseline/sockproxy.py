#!/usr/bin/env python3
"""Forward a unix socket to a TCP address (byte-for-byte, no HTTP parsing).

gh only speaks HTTPS to enterprise hosts, except over `http_unix_socket`, where it sends plain
HTTP. tests/gh/run.sh uses forge's own FORGE_UNIX_SOCKET; servers in containers (forge, Forgejo)
are reached through this forwarder instead, so both are tested over the same transport.
    sockproxy.py /tmp/x.sock 127.0.0.1 3320
"""
import asyncio
import os
import sys


async def pipe(reader, writer):
    try:
        while True:
            data = await reader.read(65536)
            if not data:
                break
            writer.write(data)
            await writer.drain()
    except (ConnectionError, asyncio.CancelledError):
        pass
    finally:
        try:
            writer.close()
        except Exception:
            pass


async def main(path, host, port):
    async def handle(cr, cw):
        try:
            ur, uw = await asyncio.open_connection(host, port)
        except OSError:
            cw.close()
            return
        await asyncio.gather(pipe(cr, uw), pipe(ur, cw))

    if os.path.exists(path):
        os.unlink(path)
    server = await asyncio.start_unix_server(handle, path=path)
    async with server:
        await server.serve_forever()


if __name__ == "__main__":
    asyncio.run(main(sys.argv[1], sys.argv[2], int(sys.argv[3])))
