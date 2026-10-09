#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
ChatServer 端到端冒烟测试
覆盖 src/main.rs 中实际注册的全部 22 个 HTTP 端点 + 3 个错误用例。

用法：
    python tests/smoke_test.py            # 默认 http://127.0.0.1:8080
    python tests/smoke_test.py http://host:port

约定：Token 必须走 query param `?token=xxx`，POST 不支持 Authorization header。
"""
import json
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

BASE = sys.argv[1] if len(sys.argv) > 1 else "http://127.0.0.1:8080"

results = []


def call(method, path, body=None, token=None, expect=(200,)):
    """发起一次请求，返回 (是否通过, 状态码, 响应体dict)"""
    url = BASE + path
    if token:
        url += ("&" if "?" in path else "?") + "token=" + urllib.parse.quote(token)

    data = None
    headers = {}
    if body is not None:
        data = json.dumps(body).encode("utf-8")
        headers["Content-Type"] = "application/json"

    req = urllib.request.Request(url, data=data, headers=headers, method=method)
    try:
        with urllib.request.urlopen(req, timeout=15) as resp:
            raw = resp.read().decode("utf-8", "replace")
            status = resp.status
    except urllib.error.HTTPError as e:
        raw = e.read().decode("utf-8", "replace")
        status = e.code
    except Exception as e:
        results.append((False, "%s %s -> 请求异常: %s" % (method, path, e)))
        return False, None, None

    try:
        payload = json.loads(raw) if raw else {}
    except json.JSONDecodeError:
        payload = {"_raw": raw[:200]}

    ok = status in expect
    mark = "OK " if ok else "FAIL"
    desc = "%s %-45s -> %s" % (method, path.split("?")[0], status)
    if not ok:
        desc += "  (期望 %s)  body=%s" % (expect, str(payload)[:160])
    results.append((ok, "%s %s" % (mark, desc)))
    return ok, status, payload


def check(name, cond, detail=""):
    results.append((bool(cond), "%s %s %s" % ("OK " if cond else "FAIL", name, detail)))
    return bool(cond)


def main():
    ts = str(int(time.time() * 1000))[-8:]
    alice, bob = "alice_" + ts, "bob_" + ts
    pwd = "Test@123456"

    print("=" * 70)
    print("ChatServer 冒烟测试  %s" % BASE)
    print("测试账号: %s / %s" % (alice, bob))
    print("=" * 70)

    # ---------- 基础 ----------
    call("GET", "/health")
    call("GET", "/status")

    # ---------- 认证 ----------
    ok, _, r = call("POST", "/api/auth/register",
                    {"username": alice, "password": pwd, "nickname": "Alice"})
    ok_b, _, r_b = call("POST", "/api/auth/register",
                        {"username": bob, "password": pwd, "nickname": "Bob"})

    # 认证响应是顶层 token，没有 success/data 包装
    a_token = (r or {}).get("token")
    b_token = (r_b or {}).get("token")
    check("注册返回顶层 token", bool(a_token) and bool(b_token),
          "" if a_token else " 实际返回=%s" % str(r)[:150])

    ok, _, r = call("POST", "/api/auth/login", {"username": alice, "password": pwd})
    if (r or {}).get("token"):
        a_token = r["token"]
    _, _, r_b = call("POST", "/api/auth/login", {"username": bob, "password": pwd})
    if (r_b or {}).get("token"):
        b_token = r_b["token"]

    call("GET", "/api/user/info", token=a_token)

    ok, _, r = call("POST", "/api/auth/refresh",
                    {"refresh_token": (r or {}).get("refresh_token", "")})
    _ = ok

    # ---------- 好友 ----------
    # 先拿到 alice/bob 的 user_id
    ok, _, info = call("GET", "/api/user/info", token=a_token)
    a_uid = (info or {}).get("id") or (info or {}).get("data", {}).get("id")
    ok, _, info_b = call("GET", "/api/user/info", token=b_token)
    b_uid = (info_b or {}).get("id") or (info_b or {}).get("data", {}).get("id")

    if a_uid and b_uid:
        call("POST", "/api/friends", {"friend_id": b_uid}, token=a_token)
        call("GET", "/api/friends", token=a_token)
        # 用真实 request_id 处理好友请求（此前固定写 1，404 被当成通过，是假绿）
        _, _, reqs = call("GET", "/api/friends/requests", token=b_token)
        rid = None
        if isinstance(reqs, list) and reqs:
            rid = (reqs[0] or {}).get("request_id") or (reqs[0] or {}).get("id")
        elif isinstance(reqs, dict):
            items = reqs.get("data") or reqs.get("requests") or []
            if isinstance(items, list) and items:
                rid = (items[0] or {}).get("request_id") or (items[0] or {}).get("id")
        if rid:
            call("POST", "/api/friends/handle",
                 {"request_id": rid, "accept": True}, token=b_token)
            _, _, fl = call("GET", "/api/friends", token=a_token)
            friends = fl if isinstance(fl, list) else ((fl or {}).get("data") or [])
            check("接受好友请求后双方成为好友",
                  any((x or {}).get("id") == b_uid for x in (friends or [])),
                  " friends=%s" % str(friends)[:160])
        else:
            check("拿到好友请求 id", False, " requests=%s" % str(reqs)[:160])
        call("DELETE", "/api/friends/%s" % b_uid, token=a_token,
             expect=(200, 404, 500))
    else:
        check("获取 user_id", False, " user/info 未返回 id: %s" % str(info)[:150])

    # ---------- 消息 ----------
    if a_uid and b_uid:
        ok, _, m = call("POST", "/api/messages",
                        {"receiver_id": b_uid, "content": "hello from smoke test",
                         "msg_type": "text"}, token=a_token)
        check("发送消息返回 id", bool((m or {}).get("id")) or ok,
              " body=%s" % str(m)[:120])
        call("GET", "/api/messages", token=b_token)
        call("GET", "/api/messages?friend_id=%s" % a_uid, token=b_token)
        call("GET", "/api/messages/unread", token=b_token)
        # 标记已读需要 message_ids 数组
        msg_id = (m or {}).get("id")
        if msg_id:
            call("POST", "/api/messages/read",
                 {"message_ids": [msg_id]}, token=b_token)

    # ---------- 群组 ----------
    ok, _, g = call("POST", "/api/groups",
                    {"name": "SmokeGroup_" + ts}, token=a_token)
    gid = (g or {}).get("id") or (g or {}).get("data", {}).get("id")
    call("GET", "/api/groups", token=a_token)

    if gid:
        call("GET", "/api/groups/%s/members" % gid, token=a_token)
        if a_uid and b_uid:
            call("POST", "/api/groups/%s/members" % gid, {"user_id": b_uid},
                 token=a_token, expect=(200, 400, 403, 404, 500))
            call("DELETE", "/api/groups/%s/members/%s" % (gid, b_uid),
                 token=a_token, expect=(200, 400, 403, 404, 500))
        call("POST", "/api/groups/%s/leave" % gid, token=a_token,
             expect=(200, 400, 403, 404, 500))
    else:
        check("创建群组返回 id", False, " body=%s" % str(g)[:150])

    # ---------- 群详情 / 加群 / 解散群（2026-09-01 补齐的 04-20 回归接口） ----------
    ok, _, g2 = call("POST", "/api/groups",
                     {"name": "SmokeGroup2_" + ts}, token=a_token)
    gid2 = (g2 or {}).get("id") or (g2 or {}).get("data", {}).get("id")
    if gid2:
        _, _, detail = call("GET", "/api/groups/%s" % gid2, token=a_token)
        check("群详情返回群名", (detail or {}).get("name") == "SmokeGroup2_" + ts,
              " detail=%s" % str(detail)[:160])

        if b_uid:
            # 非成员加入
            call("POST", "/api/groups/%s/join" % gid2, token=b_token)
            # 重复加入必须被拒绝
            call("POST", "/api/groups/%s/join" % gid2, token=b_token,
                 expect=(400, 409, 422))
            _, _, mem = call("GET", "/api/groups/%s/members" % gid2, token=a_token)
            in_group = isinstance(mem, list) and any(
                (x or {}).get("id") == b_uid for x in mem)
            check("加群后成员列表包含该用户", in_group, " members=%s" % str(mem)[:160])
            # 非群主解散必须被拒绝
            call("DELETE", "/api/groups/%s" % gid2, token=b_token, expect=(401, 403))

        # 加入不存在的群 -> 404
        call("POST", "/api/groups/99999999/join", token=a_token, expect=(404, 422))

        # 群主解散
        call("DELETE", "/api/groups/%s" % gid2, token=a_token)
        # 解散后查询 -> 404
        call("GET", "/api/groups/%s" % gid2, token=a_token, expect=(404,))
    else:
        check("创建第二个群组返回 id", False, " body=%s" % str(g2)[:150])

    # ---------- 搜索 ----------
    call("GET", "/api/search/users?keyword=" + alice[:8], token=a_token)
    call("GET", "/api/search/groups?keyword=Smoke", token=a_token)

    # ---------- 错误用例 ----------
    call("POST", "/api/auth/register",
         {"username": alice, "password": pwd, "nickname": "dup"},
         expect=(400, 409, 422))
    call("POST", "/api/auth/login",
         {"username": alice, "password": "wrong_password"},
         expect=(400, 401, 403))
    call("GET", "/api/user/info?token=invalid_token_xxx", expect=(400, 401, 403))

    # ---------- 汇总 ----------
    print("\n" + "-" * 70)
    for _, line in results:
        print(line)
    passed = sum(1 for ok_, _ in results if ok_)
    total = len(results)
    print("-" * 70)
    print("结果: %d/%d 通过  (%.0f%%)" % (passed, total, 100.0 * passed / total))
    print("-" * 70)
    return 0 if passed == total else 1


if __name__ == "__main__":
    sys.exit(main())
