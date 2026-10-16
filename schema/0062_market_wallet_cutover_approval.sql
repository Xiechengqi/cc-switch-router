-- A mode string alone is not an authorization to take real wallet payments.
-- Only the Owner cutover endpoint writes this marker after the live gates pass.
ALTER TABLE market_finance_control ADD COLUMN cutover_approved_at TEXT;

-- A credited status is not, by itself, a live payment receipt. Readiness and
-- channel re-enablement use this same evidence projection.
CREATE VIEW market_verified_topup_evidence AS
SELECT topup.id, topup.channel_id, topup.channel_revision
FROM market_topup_orders topup
JOIN market_topup_receipts receipt ON receipt.order_id = topup.id
JOIN market_journal_transactions journal
  ON journal.id = topup.credited_journal_id
 AND journal.source_kind = 'topup'
 AND journal.source_id = topup.id
 AND journal.action = 'credit'
 AND journal.fingerprint = topup.user_id || ':' || topup.amount_minor || ':' || receipt.receipt_fingerprint
JOIN market_journal_seals seal ON seal.journal_id = journal.id
WHERE topup.status = 'credited'
  AND topup.gateway_order_id IS NOT NULL
  AND receipt.confirmations > 0
  AND receipt.chain != '' AND receipt.token_id != ''
  AND receipt.tx_id != '' AND receipt.settled_amount != ''
  AND EXISTS (
      SELECT 1 FROM market_journal_postings posting
      WHERE posting.journal_id = journal.id
        AND posting.account_kind = 'wallet_available'
        AND posting.subject_id = topup.user_id
        AND posting.signed_units = -topup.amount_minor * 86400
  )
  AND EXISTS (
      SELECT 1 FROM market_journal_postings posting
      WHERE posting.journal_id = journal.id
        AND posting.account_kind = 'owner_clearing'
        AND posting.subject_id = 'owner'
        AND posting.signed_units = topup.amount_minor * 86400
  );
