-- Provider contacts are product support information, not payment instructions.
-- Historical financial tables remain intact for audit; no legacy payment writer
-- or worker consumes them. Do not rewrite immutable wallet journals or orders.
CREATE TABLE market_provider_contacts (
    user_id TEXT PRIMARY KEY REFERENCES users(id),
    owner_email TEXT NOT NULL,
    contacts_json TEXT NOT NULL DEFAULT '[]' CHECK(json_valid(contacts_json)),
    updated_at TEXT NOT NULL
);

INSERT INTO market_provider_contacts(user_id, owner_email, contacts_json, updated_at)
SELECT user_id, owner_email,
       CASE WHEN json_valid(contacts_json) THEN contacts_json ELSE '[]' END,
       updated_at
FROM account_payment_profiles
WHERE EXISTS (SELECT 1 FROM users WHERE users.id = account_payment_profiles.user_id);
