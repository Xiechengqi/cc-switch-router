ALTER TABLE shares ADD COLUMN auto_start INTEGER NOT NULL DEFAULT 0;
ALTER TABLE shares ADD COLUMN allow_personal_credits INTEGER NOT NULL DEFAULT 0;
ALTER TABLE shares ADD COLUMN auto_consume_banked_reset INTEGER NOT NULL DEFAULT 0;
ALTER TABLE shares ADD COLUMN banked_reset_expiry_lead_minutes INTEGER NOT NULL DEFAULT 60;
ALTER TABLE shares ADD COLUMN previous_response_cache_enabled INTEGER NOT NULL DEFAULT 0;
