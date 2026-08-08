ALTER TABLE shares ADD COLUMN descriptor_generation INTEGER NOT NULL DEFAULT 0;
ALTER TABLE shares ADD COLUMN descriptor_fingerprint TEXT NOT NULL DEFAULT '';

CREATE INDEX idx_shares_installation_descriptor_generation
    ON shares (installation_id, descriptor_generation);
