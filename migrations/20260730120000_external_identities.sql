CREATE TABLE external_identities (
    provider   TEXT NOT NULL,
    subject    TEXT NOT NULL,
    user_id    TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (provider, subject),
    UNIQUE (provider, user_id)
);

CREATE INDEX idx_external_identities_user_id ON external_identities(user_id);
