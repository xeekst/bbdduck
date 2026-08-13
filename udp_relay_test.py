
import sys, socket, struct, select, time, threading
try:
    stdin = sys.stdin.buffer
    stdout = sys.stdout.buffer
except AttributeError:
    stdin = sys.stdin
    stdout = sys.stdout
HOST = sys.argv[1]
PORT = int(sys.argv[2])
MODE = sys.argv[3]
lock = threading.Lock()
flows = {}
by_addr = {}
next_fid = [0]
IDLE = 120.0
def alloc_fid():
    with lock:
        fid = next_fid[0]
        next_fid[0] = (fid + 1) & 0xFFFFFFFF
        return fid
def send_frame(fid, data, op=1):
    head = struct.pack("<BBII", op, 0, fid, len(data))
    stdout.write(head + data)
    stdout.flush()
def recvn(n):
    buf = b""
    while len(buf) < n:
        chunk = stdin.read(n - len(buf))
        if not chunk:
            raise EOFError()
        buf += chunk
    return buf
def reader(sock):
    try:
        while True:
            h = recvn(10)
            op, _, fid, ln = struct.unpack("<BBII", h)
            if ln > 65536:
                continue
            data = recvn(ln) if ln else b""
            if op == 2:
                with lock:
                    item = flows.pop(fid, None)
                if item is None:
                    continue
                if MODE == "connect":
                    try:
                        item[0].close()
                    except Exception:
                        pass
                else:
                    with lock:
                        by_addr.pop(item[0], None)
                continue
            with lock:
                item = flows.get(fid)
                if item is None:
                    if MODE == "connect":
                        s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
                        s.settimeout(0.0)
                        s.connect((HOST, PORT))
                        item = [s, time.time()]
                    else:
                        item = [None, time.time()]
                    flows[fid] = item
            if MODE == "connect":
                try:
                    item[0].send(data)
                except Exception:
                    pass
                with lock:
                    item[1] = time.time()
            else:
                with lock:
                    item = flows.get(fid)
                if item is not None and item[0] is not None:
                    try:
                        sock.sendto(data, item[0])
                    except Exception:
                        pass
                    with lock:
                        item[1] = time.time()
    except Exception:
        return
def prune():
    now = time.time()
    with lock:
        stale = [fid for fid, item in flows.items() if now - item[1] > IDLE]
    for fid in stale:
        with lock:
            item = flows.pop(fid, None)
        if item is None:
            continue
        if MODE == "listen":
            with lock:
                by_addr.pop(item[0], None)
        else:
            try:
                item[0].close()
            except Exception:
                pass
        try:
            send_frame(fid, b"", 2)
        except Exception:
            pass
def main_loop():
    if MODE == "listen":
        s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        s.settimeout(0.5)
        s.bind((HOST, PORT))
        t = threading.Thread(target=reader, args=(s,))
        t.daemon = True
        t.start()
        while True:
            try:
                data, addr = s.recvfrom(65535)
            except Exception:
                data = None
            if data is not None:
                with lock:
                    fid = by_addr.get(addr)
                    if fid is None:
                        fid = alloc_fid()
                        by_addr[addr] = fid
                        flows[fid] = [addr, time.time()]
                send_frame(fid, data)
            prune()
    else:
        t = threading.Thread(target=reader, args=(None,))
        t.daemon = True
        t.start()
        while True:
            with lock:
                items = [(fid, item[0]) for fid, item in flows.items() if item[0] is not None]
            if items:
                socks = [it[1] for it in items]
                try:
                    r, _, _ = select.select(socks, [], [], 0.5)
                except Exception:
                    r = []
                for sock in r:
                    try:
                        data = sock.recv(65535)
                    except Exception:
                        data = None
                    if data:
                        fid = None
                        for f0, f1 in items:
                            if f1 is sock:
                                fid = f0
                                break
                        if fid is not None:
                            send_frame(fid, data)
                            with lock:
                                item = flows.get(fid)
                                if item is not None:
                                    item[1] = time.time()
            else:
                time.sleep(0.5)
            prune()
main_loop()

