-- Binance UIDs are derived from verified API credentials. Replace any mutable
-- profile value with the live bound identity, or remove it when no credentials
-- remain. Frozen invoice snapshots and receipt history are intentionally left
-- unchanged.
WITH rewritten_profiles AS (
    SELECT profile.user_id,
           COALESCE((
               SELECT json_group_array(json(rewritten.method_json))
               FROM (
                   SELECT CASE
                       WHEN json_extract(entry.value, '$.kind') != 'binance'
                           THEN entry.value
                       WHEN account.binance_uid IS NOT NULL
                           THEN json_set(
                               json_remove(entry.value, '$.account', '$.settlementAsset'),
                               '$.account', account.binance_uid,
                               '$.settlementAsset', 'USDT'
                           )
                       ELSE json_remove(entry.value, '$.account', '$.settlementAsset')
                   END AS method_json
                   FROM json_each(profile.methods_json) AS entry
                   LEFT JOIN binance_payment_accounts AS account
                     ON account.id = (
                         SELECT candidate.id
                         FROM binance_payment_accounts AS candidate
                         WHERE candidate.supplier_user_id = profile.user_id
                           AND candidate.credentials_ciphertext != ''
                         ORDER BY candidate.updated_at DESC, candidate.id
                         LIMIT 1
                     )
                   WHERE json_extract(entry.value, '$.kind') != 'binance'
                      OR account.binance_uid IS NOT NULL
                      OR NULLIF(TRIM(json_extract(entry.value, '$.qrImageUrl')), '') IS NOT NULL
                   ORDER BY CAST(entry.key AS INTEGER)
               ) AS rewritten
           ), '[]') AS methods_json
    FROM account_payment_profiles AS profile
)
UPDATE account_payment_profiles
   SET methods_json = (
       SELECT rewritten.methods_json
       FROM rewritten_profiles AS rewritten
       WHERE rewritten.user_id = account_payment_profiles.user_id
   );

-- A verified binding is also the source of truth when an older or drifted
-- profile has no Binance row at all. Preserve its existing method order and
-- append the canonical API-derived identity.
UPDATE account_payment_profiles
   SET methods_json = json_insert(
       methods_json,
       '$[#]',
       json_object(
           'kind', 'binance',
           'account', (
               SELECT account.binance_uid
               FROM binance_payment_accounts AS account
               WHERE account.supplier_user_id = account_payment_profiles.user_id
                 AND account.credentials_ciphertext != ''
               ORDER BY account.updated_at DESC, account.id
               LIMIT 1
           ),
           'settlementAsset', 'USDT'
       )
   )
 WHERE NOT EXISTS (
           SELECT 1
           FROM json_each(account_payment_profiles.methods_json) AS entry
           WHERE json_extract(entry.value, '$.kind') = 'binance'
       )
   AND EXISTS (
           SELECT 1
           FROM binance_payment_accounts AS account
           WHERE account.supplier_user_id = account_payment_profiles.user_id
             AND account.credentials_ciphertext != ''
       );

DELETE FROM account_payment_methods;

INSERT INTO account_payment_methods (
    id, profile_user_id, position, kind, method_json, enabled, created_at, updated_at
)
SELECT lower(hex(randomblob(16))),
       profile.user_id,
       CAST(entry.key AS INTEGER),
       json_extract(entry.value, '$.kind'),
       json(entry.value),
       1,
       profile.updated_at,
       profile.updated_at
FROM account_payment_profiles AS profile,
     json_each(profile.methods_json) AS entry;
