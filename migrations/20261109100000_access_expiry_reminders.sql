CREATE TABLE access_expiry_reminders (
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    expires_at INTEGER NOT NULL,
    sent INTEGER NOT NULL DEFAULT 0 CHECK (sent IN (0, 1)),
    attempts INTEGER NOT NULL CHECK (attempts > 0),
    next_attempt_at INTEGER NOT NULL,
    PRIMARY KEY (user_id, expires_at)
);

CREATE INDEX idx_access_grants_expiry ON access_grants(expires_at);
CREATE INDEX idx_access_grants_user_group_expiry ON access_grants(user_id, group_id, expires_at);

-- Only the latest grant for a group can end its access. Manual memberships have no deadline.
CREATE VIEW access_expiry_reminder_groups AS
    SELECT a.user_id, a.expires_at, u.username, trim(u.email) AS email, g.name AS group_name
    FROM access_grants a
    JOIN users u ON u.id = a.user_id
    JOIN user_groups g ON g.id = a.group_id
    WHERE u.status = 'active' AND trim(u.email) <> ''
      AND NOT EXISTS (
          SELECT 1 FROM external_identities e
          JOIN feishu_user_states f ON f.subject = e.subject
          WHERE e.provider = 'feishu' AND e.user_id = u.id AND f.blocked = 1
      )
      AND NOT EXISTS (
          SELECT 1 FROM user_group_members m
          WHERE m.user_id = a.user_id AND m.group_id = a.group_id
      )
      AND NOT EXISTS (
          SELECT 1 FROM access_grants later
          WHERE later.user_id = a.user_id AND later.group_id = a.group_id
            AND later.expires_at > a.expires_at
      );
