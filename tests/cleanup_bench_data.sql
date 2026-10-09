-- 清理 ChatServer 压测/冒烟测试残留数据
-- 匹配 username 以 bench_/alice_/bob_ 开头的用户及其关联数据

SET @patterns := 'bench_%,alice_%,bob_%';

-- 删除消息
DELETE FROM messages
WHERE sender_id IN (SELECT id FROM users WHERE username LIKE 'bench_%' OR username LIKE 'alice_%' OR username LIKE 'bob_%')
   OR receiver_id IN (SELECT id FROM users WHERE username LIKE 'bench_%' OR username LIKE 'alice_%' OR username LIKE 'bob_%');
SELECT ROW_COUNT() AS messages_deleted;

-- 删除群成员
DELETE FROM group_members
WHERE user_id IN (SELECT id FROM users WHERE username LIKE 'bench_%' OR username LIKE 'alice_%' OR username LIKE 'bob_%');
SELECT ROW_COUNT() AS group_members_deleted;

-- 删除好友请求
DELETE FROM friend_requests
WHERE from_user_id IN (SELECT id FROM users WHERE username LIKE 'bench_%' OR username LIKE 'alice_%' OR username LIKE 'bob_%')
   OR to_user_id IN (SELECT id FROM users WHERE username LIKE 'bench_%' OR username LIKE 'alice_%' OR username LIKE 'bob_%');
SELECT ROW_COUNT() AS friend_requests_deleted;

-- 删除好友关系
DELETE FROM friends
WHERE user_id IN (SELECT id FROM users WHERE username LIKE 'bench_%' OR username LIKE 'alice_%' OR username LIKE 'bob_%')
   OR friend_id IN (SELECT id FROM users WHERE username LIKE 'bench_%' OR username LIKE 'alice_%' OR username LIKE 'bob_%');
SELECT ROW_COUNT() AS friends_deleted;

-- 删除 refresh_tokens
DELETE FROM refresh_tokens
WHERE user_id IN (SELECT id FROM users WHERE username LIKE 'bench_%' OR username LIKE 'alice_%' OR username LIKE 'bob_%');
SELECT ROW_COUNT() AS refresh_tokens_deleted;

-- 删除群（owner 为测试用户）
DELETE FROM `groups`
WHERE owner_id IN (SELECT id FROM users WHERE username LIKE 'bench_%' OR username LIKE 'alice_%' OR username LIKE 'bob_%');
SELECT ROW_COUNT() AS groups_deleted;

-- 最后删除测试用户
DELETE FROM users
WHERE username LIKE 'bench_%' OR username LIKE 'alice_%' OR username LIKE 'bob_%';
SELECT ROW_COUNT() AS users_deleted;
