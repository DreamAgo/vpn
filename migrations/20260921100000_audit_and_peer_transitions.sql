-- Stable audit pagination and chronological lookup by action.
CREATE INDEX idx_audit_logs_time_id ON audit_logs(created_at DESC, id DESC);
CREATE INDEX idx_audit_logs_action_time ON audit_logs(action, created_at DESC);

-- Emit only real lifecycle transitions, atomically with the status change.
-- Repeated heartbeats have identical status and produce no event.
CREATE TRIGGER peer_status_transition AFTER UPDATE OF status ON peers
WHEN OLD.status <> NEW.status
BEGIN
    INSERT INTO peer_events(id, peer_id, user_id, field, old_value, new_value, created_at)
    VALUES(lower(hex(randomblob(16))), NEW.id, NEW.user_id, 'status', OLD.status, NEW.status, NEW.updated_at);
END;
