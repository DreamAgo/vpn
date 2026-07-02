-- 清理 user_group_members 的悬挂行。
--
-- 早先删除用户/组时未联动删除其在关联表中的成员行（表无 FK / ON DELETE CASCADE），
-- 残留的 (user_id, group_id) 会让组的 member_count 永久虚高。应用层现已在删除用户时
-- 联动清理（delete_user → remove_user_from_groups），删除组时本就同事务清成员；此处
-- 一次性清掉历史遗留的悬挂行。

DELETE FROM user_group_members
    WHERE user_id NOT IN (SELECT id FROM users);

DELETE FROM user_group_members
    WHERE group_id NOT IN (SELECT id FROM user_groups);
