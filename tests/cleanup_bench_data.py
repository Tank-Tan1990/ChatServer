#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""清理 ChatServer 压测/冒烟测试残留数据。
匹配 username 以 bench_/alice_/bob_ 开头的用户及其关联数据。
"""

import os
import sys
import pymysql

DB_HOST = os.environ.get("DB_HOST", "127.0.0.1")
DB_USER = os.environ.get("DB_USER", "chatuser")
DB_PASS = os.environ.get("DB_PASS", "CHANGE_ME")
DB_NAME = os.environ.get("DB_NAME", "chat_server")

PATTERNS = ["bench_%", "alice_%", "bob_%"]


def main():
    conn = pymysql.connect(host=DB_HOST, user=DB_USER, password=DB_PASS, database=DB_NAME)
    cur = conn.cursor()
    try:
        # 先列出将要删除的用户数量
        where = " OR ".join("username LIKE %s" for _ in PATTERNS)
        cur.execute(f"SELECT COUNT(*) FROM users WHERE {where}", PATTERNS)
        user_count = cur.fetchone()[0]
        print(f"发现测试用户: {user_count}")

        if user_count == 0:
            print("没有需要清理的测试数据。")
            return 0

        # 构造用户 ID 子查询
        user_sub = f"SELECT id FROM users WHERE {where}"

        tables = [
            ("messages", "sender_id IN ({}) OR receiver_id IN ({})"),
            ("group_members", "user_id IN ({})"),
            ("friend_requests", "from_user_id IN ({}) OR to_user_id IN ({})"),
            ("friends", "user_id IN ({}) OR friend_id IN ({})"),
            ("refresh_tokens", "user_id IN ({})"),
            ("groups", "owner_id IN ({})"),
        ]

        for table, cond_template in tables:
            cond = cond_template.format(user_sub, user_sub)
            sql = f"DELETE FROM {table} WHERE {cond}"
            cur.execute(sql, PATTERNS * cond_template.count("{}"))
            print(f"  清理 {table}: {cur.rowcount} 行")

        # 最后删除用户
        cur.execute(f"DELETE FROM users WHERE {where}", PATTERNS)
        print(f"  清理 users: {cur.rowcount} 行")

        conn.commit()
        print("清理完成。")
        return 0
    except Exception as e:
        conn.rollback()
        print(f"清理失败: {e}", file=sys.stderr)
        return 1
    finally:
        cur.close()
        conn.close()


if __name__ == "__main__":
    sys.exit(main())
