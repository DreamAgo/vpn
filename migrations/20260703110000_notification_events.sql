CREATE TABLE IF NOT EXISTS notification_events (
    id TEXT PRIMARY KEY,
    event_type TEXT NOT NULL,
    channel TEXT NOT NULL,
    target TEXT NOT NULL,
    status TEXT NOT NULL,
    subject TEXT NOT NULL,
    error TEXT,
    metadata TEXT,
    dedupe_key TEXT NOT NULL,
    sent_at INTEGER,
    created_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_notification_events_created_at
    ON notification_events(created_at DESC);

CREATE INDEX IF NOT EXISTS idx_notification_events_dedupe
    ON notification_events(dedupe_key, created_at DESC);
