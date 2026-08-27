ALTER TABLE shares ADD COLUMN grok_image_generation_enabled INTEGER NOT NULL DEFAULT 0;
ALTER TABLE shares ADD COLUMN grok_image_edit_enabled INTEGER NOT NULL DEFAULT 0;
ALTER TABLE shares ADD COLUMN grok_video_generation_enabled INTEGER NOT NULL DEFAULT 0;

ALTER TABLE share_request_logs ADD COLUMN request_kind TEXT NOT NULL DEFAULT 'text';
ALTER TABLE share_request_logs ADD COLUMN operation TEXT NOT NULL DEFAULT 'unknown';
ALTER TABLE share_request_logs ADD COLUMN parent_request_id TEXT;
ALTER TABLE share_request_logs ADD COLUMN error_message TEXT;
ALTER TABLE share_request_logs ADD COLUMN media_task_id TEXT;
ALTER TABLE share_request_logs ADD COLUMN media_status TEXT;
ALTER TABLE share_request_logs ADD COLUMN video_duration_seconds INTEGER;
ALTER TABLE share_request_logs ADD COLUMN video_resolution TEXT;
ALTER TABLE share_request_logs ADD COLUMN video_aspect_ratio TEXT;

CREATE INDEX idx_share_request_logs_share_kind_created
    ON share_request_logs(share_id, request_kind, created_at DESC, request_id DESC);
CREATE INDEX idx_share_request_logs_parent_request
    ON share_request_logs(parent_request_id)
    WHERE parent_request_id IS NOT NULL;
CREATE INDEX idx_share_request_logs_media_task
    ON share_request_logs(media_task_id)
    WHERE media_task_id IS NOT NULL;
