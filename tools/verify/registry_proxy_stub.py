#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Long-lived stand-in for the in-process registry proxy during the R10 matrix.

The product profile names one loopback port. This process keeps that port open
for the matrix. It allows the same public registry hosts as
src-tauri/src/registry_proxy.rs and refuses everything else.
"""

from __future__ import annotations

import select
import socket
import sys
import threading

HOSTS = {
    "registry.npmjs.org",
    "registry.npmmirror.com",
    "repo.npmmirror.com",
    "index.crates.io",
    "static.crates.io",
    "crates.io",
    "pypi.org",
    "files.pythonhosted.org",
    "pypi.python.org",
    "proxy.golang.org",
    "sum.golang.org",
    "repo1.maven.org",
    "repo.maven.apache.org",
    "mirrors.aliyun.com",
    "mirrors.cloud.tencent.com",
    "mirrors.tuna.tsinghua.edu.cn",
    "mirrors.ustc.edu.cn",
    "mirrors.huaweicloud.com",
}


def allowed(host: str) -> bool:
    host = host.strip(".").lower()
    return any(host == item or host.endswith("." + item) for item in HOSTS)


def deny(client: socket.socket, reason: str) -> None:
    body = f"出口代理拒绝：{reason}".encode()
    client.sendall(
        b"HTTP/1.1 403 Forbidden\r\nContent-Type: text/plain; charset=utf-8\r\n"
        + f"Content-Length: {len(body)}\r\nConnection: close\r\n\r\n".encode()
        + body
    )


def pipe(left: socket.socket, right: socket.socket) -> None:
    sockets = [left, right]
    try:
        while sockets:
            readable, _, _ = select.select(sockets, [], [], 30)
            if not readable:
                break
            for item in readable:
                data = item.recv(65536)
                if not data:
                    return
                other = right if item is left else left
                other.sendall(data)
    except OSError:
        return
    finally:
        for item in (left, right):
            try:
                item.close()
            except OSError:
                pass


def handle(client: socket.socket) -> None:
    try:
        data = client.recv(8192)
        if not data:
            return
        line = data.split(b"\r\n", 1)[0].decode("latin1", "replace")
        parts = line.split()
        if len(parts) < 2:
            deny(client, "无法识别目标")
            return
        method, target = parts[0].upper(), parts[1]
        if method == "CONNECT":
            host, _, port = target.rpartition(":")
            if port != "443" or not allowed(host):
                deny(client, "目标主机不在注册表白名单内")
                return
            upstream = socket.create_connection((host, 443), timeout=10)
            client.sendall(b"HTTP/1.1 200 Connection Established\r\n\r\n")
            pipe(client, upstream)
            return
        if method not in {"GET", "HEAD"} or not target.lower().startswith("http://"):
            deny(client, "该端口不在自动放行范围内")
            return
        rest = target.split("://", 1)[1]
        hostport = rest.split("/", 1)[0]
        host, _, port = hostport.partition(":")
        if port not in {"", "80"} or not allowed(host):
            deny(client, "目标主机不在注册表白名单内")
            return
        upstream = socket.create_connection((host, int(port or "80")), timeout=10)
        upstream.sendall(data)
        pipe(client, upstream)
    except OSError:
        try:
            deny(client, "无法连接目标主机")
        except OSError:
            return


def main() -> None:
    server = socket.socket()
    server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    server.bind(("127.0.0.1", 0))
    server.listen()
    print(server.getsockname()[1], flush=True)
    while True:
        client, _ = server.accept()
        threading.Thread(target=handle, args=(client,), daemon=True).start()


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        sys.exit(0)
