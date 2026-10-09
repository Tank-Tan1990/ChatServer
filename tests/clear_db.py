import os
import pymysql
conn = pymysql.connect(host='127.0.0.1', user='chatuser', password=os.environ.get("DB_PASS", "CHANGE_ME"), database='chat_server')
c = conn.cursor()
c.execute('SET FOREIGN_KEY_CHECKS=0')
for table in ['group_members', '`groups`', 'friends', 'friend_requests', 'messages', 'refresh_tokens', 'users']:
    try:
        c.execute(f'TRUNCATE TABLE {table}')
    except: pass
c.execute('SET FOREIGN_KEY_CHECKS=1')
conn.commit()
c.execute('SELECT COUNT(*) FROM users')
print(f'Users: {c.fetchone()[0]}')
conn.close()
