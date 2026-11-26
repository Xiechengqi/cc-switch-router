ALTER TABLE online_client_leases ADD COLUMN management_version INTEGER NOT NULL DEFAULT 1;
ALTER TABLE installation_upgrade_tasks ADD COLUMN lease_id TEXT REFERENCES online_client_leases(id);
CREATE INDEX installation_upgrade_lease ON installation_upgrade_tasks(installation_id,lease_id,created_at DESC);
-- Client task identifiers carry their original lease across process restarts and delayed signed reports.
CREATE TRIGGER installation_upgrade_bind_lease AFTER INSERT ON installation_upgrade_tasks
WHEN substr(NEW.task_id,1,6)='lease-'
BEGIN
    UPDATE installation_upgrade_tasks SET lease_id=(
        SELECT id FROM online_client_leases WHERE installation_id=NEW.installation_id
          AND id=substr(NEW.task_id,7,36) AND substr(NEW.task_id,43,1)='-'
    ) WHERE installation_id=NEW.installation_id AND task_id=NEW.task_id;
END;
