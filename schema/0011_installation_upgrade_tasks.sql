CREATE TABLE installation_upgrade_tasks (
    installation_id TEXT NOT NULL,
    task_id TEXT NOT NULL,
    requested_by_email TEXT,
    status TEXT NOT NULL CHECK(status IN ('running', 'success', 'failed')),
    restart_pending INTEGER NOT NULL DEFAULT 0 CHECK(restart_pending IN (0, 1)),
    target_commit_id TEXT,
    logs_json TEXT NOT NULL DEFAULT '[]',
    restart_after INTEGER NOT NULL DEFAULT 1 CHECK(restart_after IN (0, 1)),
    client_reported_at_ms INTEGER,
    reported_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (installation_id, task_id),
    FOREIGN KEY (installation_id) REFERENCES installations(id) ON DELETE CASCADE
);

CREATE INDEX idx_installation_upgrade_tasks_latest
    ON installation_upgrade_tasks(installation_id, created_at DESC, task_id DESC);

CREATE INDEX idx_installation_upgrade_tasks_running
    ON installation_upgrade_tasks(installation_id, status, created_at DESC, task_id DESC);
