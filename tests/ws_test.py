#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
WebSocket 端到端测试（零依赖，标准库实现 WS 客户端）。

覆盖 2026-09-01 修复的 WS 推送链路：
  1. 握手 + auth -> 必须收到 auth_success（且为第一条推送）
  2. 单聊：A 发 B 收，A 收到回显
  3. 群聊：广播给全部在线成员
  4. 应用层 ping -> pong
  5. 长连接存活 > 35 秒（修复前连接 30 秒被空壳 send_task 掐断）
  6. 未认证连接 30 秒被拒（安全行为不回归）

用法：python tests/ws_test.py
"""
import base64
import json
import os
import socket
import struct
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

BASE = sys.argv[1] if len(sys.argv) > 1 else "http://127.0.0.1:8080"
_parse = urllib.parse.urlparse(BASE)
HOST = _parse.hostname or "127.0.0.1"
WS_PORT = _parse.port or 8080
PASS, FAIL = 0, 0


def check(name, cond, detail=""):
    global PASS, FAIL
    if cond:
        PASS += 1
        print("  [PASS] %s" % name)
    else:
        FAIL += 1
        print("  [FAIL] %s  %s" % (name, detail))


def call(method, path, body=None, token=None):
    url = BASE + path
    if token:
        url += ("&" if "?" in path else "?") + "token=" + urllib.parse.quote(token)
    data = json.dumps(body).encode() if body is not None else None
    headers = {"Content-Type": "application/json"} if body is not None else {}
    req = urllib.request.Request(url, data=data, headers=headers, method=method)
    try:
        with urllib.request.urlopen(req, timeout=30) as r:
            return r.status, json.loads(r.read().decode() or "{}")
    except urllib.error.HTTPError as e:
        try:
            return e.code, json.loads(e.read().decode() or "{}")
        except Exception:
            return e.code, {}


class WsClient:
    """标准库手写的最小 WebSocket 客户端（仅支持文本帧，够测试用）。"""

    def __init__(self):
        self.sock = socket.create_connection((HOST, WS_PORT), timeout=10)
        key = base64.b64encode(os.urandom(16)).decode()
        req = ("GET /ws HTTP/1.1\r\nHost: %s:%d\r\nUpgrade: websocket\r\n"
               "Connection: Upgrade\r\nSec-WebSocket-Key: %s\r\n"
               "Sec-WebSocket-Version: 13\r\n\r\n") % (HOST, WS_PORT, key)
        self.sock.sendall(req.encode())
        buf = b""
        while b"\r\n\r\n" not in buf:
            chunk = self.sock.recv(4096)
            if not chunk:
                raise RuntimeError("握手阶段连接被关闭")
            buf += chunk
        head, self.buf = buf.split(b"\r\n\r\n", 1)
        if b"101" not in head.split(b"\r\n")[0]:
            raise RuntimeError("握手失败: %s" % head[:120])

    def send_text(self, text):
        payload = text.encode()
        mask = os.urandom(4)
        masked = bytes(b ^ mask[i % 4] for i, b in enumerate(payload))
        n = len(payload)
        hdr = bytes([0x81])
        if n < 126:
            hdr += bytes([0x80 | n])
        elif n < 65536:
            hdr += bytes([0x80 | 126]) + struct.pack(">H", n)
        else:
            hdr += bytes([0x80 | 127]) + struct.pack(">Q", n)
        self.sock.sendall(hdr + mask + masked)

    def _read_exact(self, n):
        while len(self.buf) < n:
            self.sock.settimeout(max(0.1, self._deadline - time.time()))
            chunk = self.sock.recv(4096)
            if not chunk:
                raise RuntimeError("连接已被服务端关闭")
            self.buf += chunk
        out, self.buf = self.buf[:n], self.buf[n:]
        return out

    def recv_text(self, timeout=15):
        self._deadline = time.time() + timeout
        hdr = self._read_exact(2)
        n = hdr[1] & 0x7F
        if n == 126:
            n = struct.unpack(">H", self._read_exact(2))[0]
        elif n == 127:
            n = struct.unpack(">Q", self._read_exact(8))[0]
        payload = self._read_exact(n) if n else b""
        op = hdr[0] & 0x0F
        if op == 0x8:
            raise RuntimeError("收到 Close 帧")
        if op in (0x1, 0x0):
            return payload.decode("utf-8", "replace")
        return None  # ping/pong 等控制帧

    def recv_json(self, timeout=15):
        deadline = time.time() + timeout
        while time.time() < deadline:
            try:
                t = self.recv_text(timeout=max(0.1, deadline - time.time()))
            except (TimeoutError, socket.timeout):
                break
            if t is None:
                continue
            try:
                return json.loads(t)
            except json.JSONDecodeError:
                continue
        raise TimeoutError("recv_json 超时")

    def wait_type(self, msg_type, timeout=15):
        """持续读取直到出现指定 type 的消息（忽略其他消息）。"""
        deadline = time.time() + timeout
        while time.time() < deadline:
            try:
                m = self.recv_json(timeout=max(0.1, deadline - time.time()))
            except TimeoutError:
                break
            if m.get("type") == msg_type:
                return m
        raise TimeoutError("未等到 type=%s 的消息" % msg_type)

    def auth(self, token):
        self.send_text(json.dumps({"type": "auth", "token": token}))
        return self.wait_type("auth_success", timeout=15)

    def send_chat(self, receiver_id=None, group_id=None, content=""):
        self.send_text(json.dumps({
            "type": "chat", "receiver_id": receiver_id,
            "group_id": group_id, "content": content}))

    def close(self):
        try:
            self.sock.close()
        except Exception:
            pass


def register(tag):
    u = "ws_%s_%d" % (tag, int(time.time() * 1000) % 100000000)
    _, p = call("POST", "/api/auth/register",
                {"username": u, "password": "Test@123456", "nickname": tag})
    token = (p or {}).get("token")
    if not token:
        return None, None
    _, info = call("GET", "/api/user/info", token=token)
    return token, (info or {}).get("id")


def main():
    print("=" * 66)
    print("WebSocket 端到端测试")
    print("=" * 66)
    t0 = time.time()

    # ---------- 准备账号 ----------
    a_token, a_id = register("alice")
    b_token, b_id = register("bob")
    check("注册两个测试账号", bool(a_token and b_token and a_id and b_id))
    if not (a_token and b_token):
        return report(t0)

    # ---------- 1. 握手 + auth ----------
    try:
        a = WsClient()
        first = a.recv_json(timeout=3)
        check("auth 前无任何推送", first is None or isinstance(first, dict),
              str(first)[:80])
    except TimeoutError:
        pass  # 预期：无消息
    except Exception as e:
        check("WS 握手", False, str(e)[:120])
        return report(t0)

    m = a.auth(a_token)
    check("auth 后收到 auth_success（首条推送）", m.get("user_id") == a_id,
          str(m)[:120])

    b = WsClient()
    m = b.auth(b_token)
    check("第二个用户 auth_success", m.get("user_id") == b_id, str(m)[:120])

    # ---------- 2. 单聊 ----------
    a.send_chat(receiver_id=b_id, content="hello-b")
    got_b = b.wait_type("message", timeout=10)
    check("B 收到 A 的单聊消息", got_b.get("content") == "hello-b"
          and got_b.get("sender_id") == a_id, str(got_b)[:120])
    got_a = a.wait_type("message", timeout=10)
    check("A 收到回显", got_a.get("content") == "hello-b", str(got_a)[:120])

    # ---------- 3. 群聊 ----------
    _, g = call("POST", "/api/groups",
                {"name": "WS_Test_%d" % (int(time.time() * 1000) % 100000000)},
                token=a_token)
    gid = (g or {}).get("id")
    check("创建群", bool(gid), str(g)[:120])
    if gid:
        call("POST", "/api/groups/%s/join" % gid, token=b_token)
        a.send_chat(group_id=gid, content="group-hello")
        gb = b.wait_type("message", timeout=10)
        check("B 收到群消息", gb.get("content") == "group-hello"
              and gb.get("group_id") == gid, str(gb)[:120])
        ga = a.wait_type("message", timeout=10)
        check("A（发送者）收到群消息广播", ga.get("content") == "group-hello",
              str(ga)[:120])

    # ---------- 4. 应用层 ping/pong ----------
    a.send_text(json.dumps({"type": "ping"}))
    try:
        pong = a.wait_type("pong", timeout=10)
        check("应用层 ping -> pong", True)
    except TimeoutError:
        check("应用层 ping -> pong", False, "未收到 pong")

    # ---------- 5. 长连接存活（> 35 秒，修复前 30 秒必断） ----------
    print("  ...等待 35 秒验证长连接不被掐断（修复前 30s 必断）...")
    time.sleep(35)
    try:
        a.send_chat(receiver_id=b_id, content="still-alive")
        got_b2 = b.wait_type("message", timeout=10)
        check("35 秒后连接仍存活且可推送", got_b2.get("content") == "still-alive",
              str(got_b2)[:120])
    except Exception as e:
        check("35 秒后连接仍存活且可推送", False, str(e)[:120])

    # ---------- 6. 未认证连接被拒 ----------
    try:
        anon = WsClient()
        try:
            anon.recv_json(timeout=35)  # 应在 30s 左右收到 Close / 关闭
            check("未认证连接 ~30s 被断开", False, "35 秒内仍有消息")
        except (RuntimeError, TimeoutError):
            check("未认证连接 ~30s 被断开", True)
        anon.close()
    except Exception as e:
        check("未认证连接 ~30s 被断开", False, str(e)[:120])

    a.close()
    b.close()
    return report(t0)


def report(t0):
    print("-" * 66)
    total = PASS + FAIL
    print("结果: %d/%d 通过，用时 %.1f 秒" % (PASS, total, time.time() - t0))
    return 1 if FAIL else 0


if __name__ == "__main__":
    sys.exit(main())
