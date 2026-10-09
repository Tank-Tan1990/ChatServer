#!/usr/bin/env python3
"""
ChatServer 10K 并发综合压测
分阶段抬升并发度，覆盖注册 / 登录 / 单聊消息，并采集服务器 /status 指标。
用法: python tests/bench_10k.py [BASE_URL]
"""
import asyncio
import aiohttp
import json
import os
import sys
import time
from collections import defaultdict

BASE = sys.argv[1] if len(sys.argv) > 1 else "http://127.0.0.1:18080"
# 强制不走系统代理
os.environ.pop("HTTP_PROXY", None)
os.environ.pop("HTTPS_PROXY", None)
os.environ.pop("http_proxy", None)
os.environ.pop("https_proxy", None)

STAGES = [500, 1000, 2000, 5000, 8000, 10000]
TIMEOUT = aiohttp.ClientTimeout(total=30, connect=10)
PRE_REGISTERED = 12000  # 预注册用户总数，必须 >= 最大并发数
PASSWORD = "Test@123456"


async def status(session):
    try:
        async with session.get(f"{BASE}/status", timeout=aiohttp.ClientTimeout(total=5)) as r:
            return await r.json()
    except Exception as e:
        return {"error": str(e)}


def fmt_ms(v):
    return f"{v*1000:.1f}ms" if v is not None else "N/A"


async def one_register(session, username):
    t0 = time.time()
    try:
        async with session.post(
            f"{BASE}/api/auth/register",
            json={"username": username, "password": PASSWORD, "nickname": username},
            timeout=TIMEOUT,
        ) as r:
            await r.text()
            return r.status == 200 or r.status == 201 or r.status == 409 or r.status == 422, time.time() - t0, r.status
    except asyncio.TimeoutError:
        return False, time.time() - t0, "timeout"
    except Exception as e:
        return False, time.time() - t0, type(e).__name__


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
    return ok == total, elapsed


async def main():
    print(f"ChatServer 10K 并发压测  BASE={BASE}")
    print("=" * 80)

    async with aiohttp.ClientSession(timeout=TIMEOUT) as session:
        # 先确认服务健康
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

        connector = aiohttp.TCPConnector(limit=max(STAGES) * 2, limit_per_host=max(STAGES) * 2)
        async with aiohttp.ClientSession(connector=connector, timeout=TIMEOUT) as session:
            print(f"\n[预注册 {PRE_REGISTERED} 个用户]")
            # 预注册分批，避免 Argon2id 同时压垮 CPU
            batch_size = 200
            reg_usernames = [f"bench_{i:07d}" for i in range(PRE_REGISTERED)]
            ok_total = 0
            t0 = time.time()
            for i in range(0, PRE_REGISTERED, batch_size):
                batch = reg_usernames[i:i + batch_size]
                results = await asyncio.gather(*(one_register(session, u) for u in batch))
                ok_total += sum(1 for r in results if r[0])
                if i % 2000 == 0:
                    print(f"  registered {min(i + batch_size, PRE_REGISTERED):5d}/{PRE_REGISTERED} ok={ok_total}")
            print(f"  预注册完成: ok={ok_total}/{PRE_REGISTERED} time={time.time()-t0:.1f}s")

            # 阶段压测
            for con in STAGES:
                print(f"\n[并发 {con:5d}]")
                before = await status(session)
                db_before = before.get('database', {})
                print(f"  status before: read_in_use={db_before.get('read_pool_in_use')} "
                      f"write_in_use={db_before.get('write_pool_in_use')} "
                      f"write_sem={db_before.get('write_sem_available')}")

                # 注册压测：使用新用户名
                register_names = [f"bench_reg_{con}_{i:05d}_{int(time.time()*1000)%1000000}" for i in range(con)]
                reg_ok, reg_t = await run_stage("register", [one_register(session, u) for u in register_names], con)

                # 登录压测：使用预注册用户
                login_names = reg_usernames[:con]
                login_results = await asyncio.gather(*(one_login(session, u) for u in login_names))
                ok = sum(1 for r in login_results if r[0])
                latencies = [r[1] for r in login_results if r[0] and isinstance(r[1], (int, float))]
                p99 = sorted(latencies)[int(len(latencies)*0.99)] if latencies else None
                print(f"  {'login':12s} total={len(login_results):5d} ok={ok:5d} ({100*ok/len(login_results):.1f}%) "
                      f"p99={fmt_ms(p99)}")
                tokens_uid = [(r[3], r[4]) for r in login_results if r[0] and r[3] and r[4]]

                # 消息压测：登录用户循环互发消息
                if len(tokens_uid) >= 2:
                    msg_coros = []
                    for i in range(con):
                        sender_idx = i % len(tokens_uid)
                        receiver_idx = (i + 1) % len(tokens_uid)
                        token, receiver_id = tokens_uid[sender_idx][0], tokens_uid[receiver_idx][1]
                        msg_coros.append(one_send_msg(session, token, receiver_id))
                    msg_ok, msg_t = await run_stage("send_msg", msg_coros, con)
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
