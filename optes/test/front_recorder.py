#!/usr/bin/env python3
"""A dumb front that forwards bytes and records what crosses it.

Usage: front_recorder.py <listen_port> <upstream_host:port> <log_file>

Log lines: "<direction> <unix_ms> <bytes>", direction "up" (relay -> gateway)
or "down" (gateway -> relay). One line per chunk read.
"""

import asyncio
import sys
import time

LISTEN, UPSTREAM, LOG = int(sys.argv[1]), sys.argv[2], sys.argv[3]
log = open(LOG, "a", buffering=1)


async def pipe(reader, writer, direction):
    try:
        while data := await reader.read(65536):
            log.write(f"{direction} {int(time.time() * 1000)} {len(data)}\n")
            writer.write(data)
            await writer.drain()
    except (ConnectionError, asyncio.CancelledError):
        pass
    finally:
        writer.close()


async def handle(client_r, client_w):
    host, port = UPSTREAM.rsplit(":", 1)
    up_r, up_w = await asyncio.open_connection(host, int(port))
    log.write(f"conn {int(time.time() * 1000)} 0\n")
    await asyncio.gather(pipe(client_r, up_w, "up"), pipe(up_r, client_w, "down"))


async def main():
    server = await asyncio.start_server(handle, "127.0.0.1", LISTEN)
    async with server:
        await server.serve_forever()


asyncio.run(main())
