-- Passwords are optional and never establish email ownership by themselves.
ALTER TABLE users ADD COLUMN auth_version INTEGER NOT NULL DEFAULT 0 CHECK(auth_version >= 0);
ALTER TABLE user_sessions ADD COLUMN auth_version INTEGER NOT NULL DEFAULT 0;
ALTER TABLE user_sessions ADD COLUMN auth_method TEXT NOT NULL DEFAULT 'legacy';
ALTER TABLE user_sessions ADD COLUMN authenticated_at TEXT;
ALTER TABLE user_sessions ADD COLUMN email_authenticated_at TEXT;
ALTER TABLE user_sessions ADD COLUMN login_event_id TEXT;

CREATE TABLE user_password_credentials (
    user_id TEXT PRIMARY KEY REFERENCES users(id),
    password_hash TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE TABLE auth_password_attempts (
    id TEXT PRIMARY KEY,
    email_hash TEXT NOT NULL,
    source_hash TEXT NOT NULL,
    device_id TEXT NOT NULL,
    created_at TEXT NOT NULL
);
CREATE INDEX idx_password_attempt_email ON auth_password_attempts(email_hash, created_at);
CREATE INDEX idx_password_attempt_source ON auth_password_attempts(source_hash, created_at);
CREATE INDEX idx_password_attempt_device ON auth_password_attempts(device_id, created_at);
CREATE INDEX idx_password_attempt_time ON auth_password_attempts(created_at);
CREATE TABLE auth_password_events (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(id),
    auth_version INTEGER NOT NULL,
    source TEXT NOT NULL CHECK(source IN ('web', 'local_cli')),
    actor TEXT NOT NULL,
    created_at TEXT NOT NULL,
    UNIQUE(user_id, auth_version)
);
CREATE TABLE auth_password_notifications (
    event_id TEXT PRIMARY KEY REFERENCES auth_password_events(id),
    status TEXT NOT NULL DEFAULT 'pending' CHECK(status IN ('pending','sending','sent','failed')),
    attempts INTEGER NOT NULL DEFAULT 0,
    next_attempt_at TEXT NOT NULL,
    envelope_json TEXT,
    last_error TEXT
);
