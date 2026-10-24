#!/usr/bin/env python3
"""Minimal SOCKS4a + SOCKS5 CONNECT proxy, standing in for Tor's SocksPort.

Usage: socks_proxy.py <port> <log_file>
Logs one line per connection: "<socks version> <host>:<port>".
"""

import asyncio
import socket
import struct
import sys

PORT, LOG = int(sys.argv[1]), sys.argv[2]
log = open(LOG, "a", buffering=1)


async def pipe(r, w):
    try:
        while data := await r.read(65536):
            w.write(data)
            await w.drain()
    except ConnectionError:
        pass
    finally:
        w.close()


async def handle(r, w):
    ver = (await r.readexactly(1))[0]
    if ver == 4:
        cmd, port = struct.unpack(">BH", await r.readexactly(3))
        ip = await r.readexactly(4)
        await r.readuntil(b"\0")  # user id
        host = (await r.readuntil(b"\0"))[:-1].decode() if ip[:3] == b"\0\0\0" else socket.inet_ntoa(ip)
        ok, fail = b"\0\x5a" + b"\0" * 6, b"\0\x5b" + b"\0" * 6
    elif ver == 5:
        n = (await r.readexactly(1))[0]
        await r.readexactly(n)
        w.write(b"\x05\x00")
        _, cmd, _, atyp = await r.readexactly(4)
        if atyp == 3:
            host = (await r.readexactly((await r.readexactly(1))[0])).decode()
        elif atyp == 1:
            host = socket.inet_ntoa(await r.readexactly(4))
        else:
            host = socket.inet_ntop(socket.AF_INET6, await r.readexactly(16))
        (port,) = struct.unpack(">H", await r.readexactly(2))
        ok, fail = b"\x05\x00\x00\x01" + b"\0" * 6, b"\x05\x01\x00\x01" + b"\0" * 6
    else:
        w.close()
        return
    log.write(f"socks{ver} {host}:{port}\n")
    try:
        ur, uw = await asyncio.open_connection(host, port)
    except OSError:
        w.write(fail)
        w.close()
        return
    w.write(ok)
    await w.drain()
    await asyncio.gather(pipe(r, uw), pipe(ur, w))


async def main():
    async with await asyncio.start_server(handle, "127.0.0.1", PORT) as srv:
        await srv.serve_forever()


asyncio.run(main())
