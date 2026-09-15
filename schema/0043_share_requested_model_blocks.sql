CREATE TABLE share_requested_model_block_policies (
    share_id TEXT PRIMARY KEY REFERENCES shares(share_id) ON DELETE CASCADE,
    revision INTEGER NOT NULL DEFAULT 0 CHECK(revision >= 0),
    updated_by TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE share_requested_model_blocks (
    share_id TEXT NOT NULL REFERENCES shares(share_id) ON DELETE CASCADE,
    app_type TEXT NOT NULL CHECK(app_type IN ('claude', 'codex', 'gemini')),
    requested_model TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (share_id, app_type, requested_model),
    CHECK(requested_model = trim(requested_model)),
    CHECK(length(requested_model) BETWEEN 1 AND 200),
    CHECK(instr(requested_model, '*') = 0)
);

CREATE INDEX idx_share_requested_model_blocks_lookup
    ON share_requested_model_blocks(share_id, app_type, requested_model);
