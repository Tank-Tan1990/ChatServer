#!/usr/bin/env python3
"""
ChatServer 10K 并发登录+消息压测（不复注册，直接复用预存用户）
用法: python tests/bench_10k_login_msg.py [BASE_URL] [MAX_CONCURRENT]
"""
import asyncio
import aiohttp
import os
import sys
import time
from collections import defaultdict

BASE = sys.argv[1] if len(sys.argv) > 1 else "http://127.0.0.1:18080"
MAX_CONCURRENT = int(sys.argv[2]) if len(sys.argv) > 2 else 10000
os.environ.pop("HTTP_PROXY", None)
os.environ.pop("HTTPS_PROXY", None)
os.environ.pop("http_proxy", None)
os.environ.pop("https_proxy", None)

STAGES = [500, 1000, 2000, 5000, 8000, 10000]
if MAX_CONCURRENT < 10000:
    STAGES = [x for x in STAGES if x <= MAX_CONCURRENT]
    if MAX_CONCURRENT not in STAGES:
        STAGES.append(MAX_CONCURRENT)
TIMEOUT = aiohttp.ClientTimeout(total=60, connect=10)
USER_COUNT = max(STAGES) + 100
PASSWORD = "Test@123456"


async def status(session):
    try:
        async with session.get(f"{BASE}/status", timeout=aiohttp.ClientTimeout(total=5)) as r:
            return await r.json()
    except Exception as e:
        return {"error": str(e)}


def fmt_ms(v):
    return f"{v*1000:.1f}ms" if v is not None else "N/A"


async def one_login(session, username):
    t0 = time.time()
    try:
        async with session.post(
            f"{BASE}/api/auth/login",
            json={"username": username, "password": PASSWORD},
            timeout=TIMEOUT,
        ) as r:
            body = await r.json()
            token = body.get("token")
            user_id = (body.get("user") or {}).get("id")
            return (r.status == 200, time.time() - t0, r.status, token, user_id)
    except asyncio.TimeoutError:
        return False, time.time() - t0, "timeout", None, None
    except Exception as e:
        return False, time.time() - t0, type(e).__name__, None, None


async def one_send_msg(session, token, receiver_id):
    t0 = time.time()
    try:
        url = f"{BASE}/api/messages?token={token}"
        async with session.post(
            url,
            json={"receiver_id": receiver_id, "content": "bench", "msg_type": "text"},
            timeout=TIMEOUT,
        ) as r:
            await r.text()
            return r.status == 200 or r.status == 201, time.time() - t0, r.status
    except asyncio.TimeoutError:
        return False, time.time() - t0, "timeout"
    except Exception as e:
        return False, time.time() - t0, type(e).__name__


async def run_stage(name, coros, concurrency):
    sem = asyncio.Semaphore(concurrency)

    async def bounded(coro):
        async with sem:
            return await coro

    t0 = time.time()
    results = await asyncio.gather(*(bounded(c) for c in coros))
    elapsed = time.time() - t0

    ok = sum(1 for r in results if r[0])
    total = len(results)
    latencies = [r[1] for r in results if r[0] and isinstance(r[1], (int, float))]
    errs = defaultdict(int)
    for r in results:
        if not r[0]:
            errs[str(r[2])] += 1
    p50 = sorted(latencies)[len(latencies) // 2] if latencies else None
    p99 = sorted(latencies)[int(len(latencies) * 0.99)] if latencies else None
    max_lat = max(latencies) if latencies else None
    print(f"  {name:12s} total={total:5d} ok={ok:5d} ({100*ok/total:.1f}%) "
          f"qps={ok/elapsed:.1f} time={elapsed:.1f}s "
          f"p50={fmt_ms(p50)} p99={fmt_ms(p99)} max={fmt_ms(max_lat)}")
    if errs:
        print(f"    errors: {dict(errs)}")
    return ok == total, elapsed, results


async def main():
    print(f"ChatServer 10K 登录+消息并发压测  BASE={BASE} STAGES={STAGES}")
    print("=" * 80)

    usernames = [f"bench_{i:07d}" for i in range(1, USER_COUNT + 1)]

    connector = aiohttp.TCPConnector(limit=max(STAGES) * 2, limit_per_host=max(STAGES) * 2)
    async with aiohttp.ClientSession(connector=connector, timeout=TIMEOUT) as session:
        # 确认服务健康
        for _ in range(5):
            try:
                async with session.get(f"{BASE}/health", timeout=aiohttp.ClientTimeout(total=3)) as r:
                    if r.status == 200:
                        break
            except Exception:
                pass
            await asyncio.sleep(0.5)
        else:
            print("ERROR: /health 不可用")
            return 1

        for con in STAGES:
            print(f"\n[并发 {con:5d}]")
            before = await status(session)
            db_before = before.get('database', {})
            print(f"  status before: read_in_use={db_before.get('read_pool_in_use')} "
                  f"write_in_use={db_before.get('write_pool_in_use')} "
                  f"write_sem={db_before.get('write_sem_available')}")

            # 登录压测
            login_names = usernames[:con]
            ok, elapsed, login_results = await run_stage("login", [one_login(session, u) for u in login_names], con)
            tokens_uid = [(r[3], r[4]) for r in login_results if r[0] and r[3] and r[4]]

            # 消息压测
            if len(tokens_uid) >= 2:
                msg_coros = []
                for i in range(con):
                    sender_idx = i % len(tokens_uid)
                    receiver_idx = (i + 1) % len(tokens_uid)
                    token, receiver_id = tokens_uid[sender_idx][0], tokens_uid[receiver_idx][1]
                    msg_coros.append(one_send_msg(session, token, receiver_id))
                await run_stage("send_msg", msg_coros, con)
            else:
                print("  send_msg     skipped (no tokens)")

            after = await status(session)
            db_after = after.get('database', {})
            print(f"  status after : read_in_use={db_after.get('read_pool_in_use')} "
                  f"write_in_use={db_after.get('write_pool_in_use')} "
                  f"write_sem={db_after.get('write_sem_available')}")
            await asyncio.sleep(1)

    print("\n" + "=" * 80)
    print("压测完成")
    return 0


if __name__ == "__main__":
    asyncio.run(main())
