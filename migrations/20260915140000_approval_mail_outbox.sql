CREATE TABLE approval_mail_outbox (
    instance_code TEXT PRIMARY KEY,
    recipient TEXT NOT NULL,
    body TEXT NOT NULL,
    expires_at INTEGER NOT NULL,
    done INTEGER NOT NULL DEFAULT 0,
    attempts INTEGER NOT NULL DEFAULT 0,
    next_attempt_at INTEGER NOT NULL DEFAULT 0,
    last_error TEXT
);
CREATE INDEX approval_mail_due ON approval_mail_outbox(done, next_attempt_at);
