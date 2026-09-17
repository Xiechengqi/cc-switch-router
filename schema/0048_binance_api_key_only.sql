-- Binance payment details are owned exclusively by a verified API credential
-- binding. Remove every editable/manual Binance method, then append one clean
-- canonical UID method for profiles that still have stored credentials.
-- Frozen invoice snapshots, receipt history, and cached assets are deliberately
-- left untouched.
WITH stripped_profiles AS (
    SELECT profile.user_id,
           COALESCE((
               SELECT json_group_array(json(stripped.method_json))
               FROM (
                   SELECT entry.value AS method_json
                   FROM json_each(profile.methods_json) AS entry
                   WHERE lower(trim(COALESCE(json_extract(entry.value, '$.kind'), ''))) != 'binance'
                   ORDER BY CAST(entry.key AS INTEGER)
               ) AS stripped
           ), '[]') AS methods_json
    FROM account_payment_profiles AS profile
)
UPDATE account_payment_profiles
   SET methods_json = (
       SELECT stripped.methods_json
       FROM stripped_profiles AS stripped
       WHERE stripped.user_id = account_payment_profiles.user_id
   );

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
 WHERE EXISTS (
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
