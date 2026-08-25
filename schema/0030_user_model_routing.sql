CREATE TABLE user_model_routing_profiles (
    user_id TEXT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    revision INTEGER NOT NULL DEFAULT 0 CHECK (revision >= 0),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE user_model_routes (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    app_type TEXT NOT NULL CHECK (app_type IN ('claude', 'codex', 'gemini')),
    requested_model TEXT NOT NULL CHECK (
        requested_model = trim(requested_model)
        AND length(requested_model) BETWEEN 1 AND 200
    ),
    -- Intentionally not a foreign key. A removed Share must leave a visible,
    -- unavailable route instead of silently changing into "not configured".
    target_share_id TEXT NOT NULL CHECK (target_share_id != ''),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE (user_id, app_type, requested_model)
);

CREATE INDEX idx_user_model_routes_target
    ON user_model_routes(target_share_id, app_type);

CREATE TABLE user_model_route_events (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    revision INTEGER NOT NULL CHECK (revision > 0),
    action TEXT NOT NULL CHECK (action = 'replace'),
    routes_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    UNIQUE (user_id, revision)
);

CREATE INDEX idx_user_model_route_events_user
    ON user_model_route_events(user_id, revision DESC);
