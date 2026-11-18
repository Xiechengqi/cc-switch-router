ALTER TABLE shares ADD COLUMN capability_policy_json TEXT CHECK(capability_policy_json IS NULL OR json_valid(capability_policy_json));
