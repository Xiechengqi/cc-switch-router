CREATE TABLE installation_online_days (
    installation_id TEXT NOT NULL,
    day_utc TEXT NOT NULL,
    online_minutes INTEGER NOT NULL DEFAULT 0 CHECK (online_minutes >= 0),
    observed_minutes INTEGER NOT NULL DEFAULT 0 CHECK (observed_minutes >= 0),
    PRIMARY KEY (installation_id, day_utc)
);

CREATE INDEX idx_installation_online_days_day
    ON installation_online_days(day_utc);
