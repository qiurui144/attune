"""Raw TCP CORS proxy: reads raw request, forwards to attune, injects CORS."""
import os
import socket
import threading
import select
import re

PROXY = (
    os.environ.get("ATTUNE_PROXY_HOST", "0.0.0.0"),
    int(os.environ.get("ATTUNE_PROXY_PORT", "8889")),
)
TARGET = (
    os.environ.get("ATTUNE_TARGET_HOST", "127.0.0.1"),
    int(os.environ.get("ATTUNE_TARGET_PORT", "18906")),
)

CORS_HEADERS = (
    "Access-Control-Allow-Origin: *\r\n"
    "Access-Control-Allow-Methods: GET,POST,PATCH,DELETE,OPTIONS\r\n"
    "Access-Control-Allow-Headers: *\r\n"
    "Access-Control-Allow-Credentials: true\r\n"
    "Access-Control-Max-Age: 86400\r\n"
)

def handle(conn):
    try:
        # Read full raw request
        data = b""
        while True:
            chunk = conn.recv(65536)
            if not chunk: break
            data += chunk
            # Check if we have headers (headers end with \r\n\r\n)
            if b"\r\n\r\n" in data:
                headers_end = data.index(b"\r\n\r\n") + 4
                headers = data[:headers_end]
                body = data[headers_end:]
                # Get Content-Length
                cl_match = re.search(rb"Content-Length: (\d+)\r\n", headers, re.IGNORECASE)
                if cl_match:
                    cl = int(cl_match.group(1))
                    remaining = cl - len(body)
                    while remaining > 0:
                        chunk = conn.recv(min(65536, remaining))
                        if not chunk: break
                        body += chunk
                        remaining -= len(chunk)
                    data = headers + body
                break
        
        # Handle OPTIONS (CORS preflight)
        if data.startswith(b"OPTIONS"):
            resp = (
                b"HTTP/1.1 204 No Content\r\n"
                + CORS_HEADERS.encode()
                + b"\r\n"
            )
            conn.sendall(resp)
            return
        
        # Forward to attune
        target = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        target.settimeout(300)
        target.connect(TARGET)
        target.sendall(data)
        
        # Read response
        resp = b""
        while True:
            r, _, _ = select.select([target], [], [], 30)
            if not r: break
            chunk = target.recv(65536)
            if not chunk: break
            resp += chunk
            # Check if we have headers
            if b"\r\n\r\n" in resp:
                hdr_end = resp.index(b"\r\n\r\n") + 4
                resp_hdrs = resp[:hdr_end]
                # Get Content-Length from response
                rcl_match = re.search(rb"Content-Length: (\d+)\r\n", resp_hdrs, re.IGNORECASE)
                if rcl_match:
                    rcl = int(rcl_match.group(1))
                    body_sofar = resp[hdr_end:]
                    remaining = rcl - len(body_sofar)
                    while remaining > 0:
                        chunk = target.recv(min(65536, remaining))
                        if not chunk: break
                        resp += chunk
                        remaining -= len(chunk)
                break
        
        target.close()
        
        # Inject CORS into response headers
        if b"\r\n\r\n" in resp:
            hdr_end = resp.index(b"\r\n\r\n")
            # Remove existing CORS-like headers
            resp_hdrs = resp[:hdr_end].decode("utf-8", "replace")
            # Remove any existing Access-Control headers
            resp_hdrs = re.sub(r'Access-Control-[^\r\n]*\r\n', '', resp_hdrs, flags=re.IGNORECASE)
            # Add our CORS headers
            resp_hdrs = resp_hdrs.rstrip("\r\n") + "\r\n" + CORS_HEADERS + "\r\n"
            body = resp[hdr_end+4:]
            resp = resp_hdrs.encode() + body
        
        conn.sendall(resp)
    except Exception:
        try:
            body = b'{"error":"bad gateway"}'
            resp = (
                b"HTTP/1.1 502 Bad Gateway\r\n"
                + CORS_HEADERS.encode()
                + f"Content-Type: application/json\r\nContent-Length: {len(body)}\r\n\r\n".encode()
                + body
            )
            conn.sendall(resp)
        except: pass
    finally:
        try: conn.close()
        except: pass

srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
srv.bind(PROXY)
srv.listen(100)
print(f"Raw CORS proxy: {PROXY[0]}:{PROXY[1]} -> {TARGET[0]}:{TARGET[1]}")
while True:
    conn, addr = srv.accept()
    threading.Thread(target=handle, args=(conn,), daemon=True).start()
