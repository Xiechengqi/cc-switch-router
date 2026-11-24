-- Daily reporting totals survive after hourly reports are deleted at 7 days.
-- Counts only: no snapshots, leases, or report bodies.
CREATE TABLE client_host_health_days (
    host_id TEXT NOT NULL REFERENCES router_ssh_hosts(id) ON DELETE CASCADE,
    machine_epoch INTEGER NOT NULL,
    day_start INTEGER NOT NULL,
    received_hours INTEGER NOT NULL,
    observable_hours INTEGER NOT NULL,
    PRIMARY KEY(host_id, machine_epoch, day_start)
);
CREATE INDEX idx_client_host_health_days_retention
    ON client_host_health_days(day_start);
