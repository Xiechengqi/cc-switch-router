-- New listings explicitly publish one description. Never turn formerly
-- private contact/payment details into public listing text.
ALTER TABLE market_c2c_sellers ADD COLUMN description TEXT NOT NULL DEFAULT ''
    CHECK (length(description) <= 300);
UPDATE market_c2c_sellers SET enabled=0, version=version+1,
    display_name='C2C-' || lower(hex(randomblob(6)));
ALTER TABLE market_c2c_sellers DROP COLUMN contact;
ALTER TABLE market_c2c_sellers DROP COLUMN payment_method;
ALTER TABLE market_c2c_sellers DROP COLUMN payment_details;

-- Consumed quotes remain immutable, including their historical private terms.
-- Empty legacy columns in new quotes mean absent legacy data, not payment
-- instructions. All new publication and quoting uses description exclusively.
ALTER TABLE market_c2c_quotes ADD COLUMN description TEXT NOT NULL DEFAULT ''
    CHECK (length(description) <= 300);
