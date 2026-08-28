ALTER TABLE share_request_logs
    ADD COLUMN cache_usage_observed INTEGER NOT NULL DEFAULT 1;

ALTER TABLE share_request_logs
    ADD COLUMN usage_estimated INTEGER NOT NULL DEFAULT 0;
