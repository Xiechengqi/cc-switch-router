ALTER TABLE installation_upgrade_tasks ADD COLUMN failure_json TEXT;

CREATE INDEX idx_installation_upgrade_tasks_failure_target
    ON installation_upgrade_tasks(target_commit_id, status, installation_id);
