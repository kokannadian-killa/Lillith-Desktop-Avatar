"""Logging JSON-lines proxy: LISTEN socket -> UPSTREAM socket. Logs every line both ways."""
import asyncio, os, sys, time, json
LISTEN, UP, LOG = sys.argv[1], sys.argv[2], sys.argv[3]
log = open(LOG, "a", buffering=1); n = 0
def w(cid, d, line):
    log.write(f"{time.time():.3f} c{cid} {d} {line.decode(errors='replace').rstrip()[:400]}\n")
async def pipe(r, wr, cid, d):
    try:
        while (line := await r.readline()):
            w(cid, d, line); wr.write(line); await wr.drain()
    except Exception as e:
        log.write(f"{time.time():.3f} c{cid} {d} ERR {e}\n")
    finally:
        try: wr.close()
        except Exception: pass
async def handle(r, wr):
    global n; n += 1; cid = n
    log.write(f"{time.time():.3f} c{cid} OPEN\n")
    try:
        ur, uw = await asyncio.open_unix_connection(UP)
    except Exception as e:
        log.write(f"{time.time():.3f} c{cid} UPSTREAM-FAIL {e}\n"); wr.close(); return
    await asyncio.gather(pipe(r, uw, cid, ">"), pipe(ur, wr, cid, "<"))
    log.write(f"{time.time():.3f} c{cid} CLOSE\n")
async def main():
    if os.path.exists(LISTEN): os.unlink(LISTEN)
    old = os.umask(0o177)
    srv = await asyncio.start_unix_server(handle, LISTEN, limit=1 << 22)
    os.umask(old); log.write("proxy ready\n")
    async with srv: await srv.serve_forever()
asyncio.run(main())
