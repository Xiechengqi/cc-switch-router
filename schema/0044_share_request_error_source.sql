ALTER TABLE share_request_error_snapshots
    ADD COLUMN request_source TEXT NOT NULL DEFAULT 'user'
    CHECK (request_source IN ('user', 'dashboard_test', 'health_probe', 'gateway', 'internal'));

UPDATE share_request_error_snapshots
   SET request_source = 'health_probe'
 WHERE method = 'PROBE'
    OR path LIKE '/_share-router/model-health/%'
    OR path = '/_share-router/health';

UPDATE share_request_error_snapshots
   SET request_source = 'internal'
 WHERE path LIKE '/_share-router/%'
   AND request_source = 'user';

-- Log recovery is an idempotent maintenance retry. Its tunnel failures belong
-- in Router diagnostics, not in the Share owner's last-three business errors.
DELETE FROM share_request_error_snapshots
 WHERE path LIKE '/_share-router/request-logs%';
