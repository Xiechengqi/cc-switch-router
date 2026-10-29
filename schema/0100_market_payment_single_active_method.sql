-- Buyers choose a top-up method (for example cryptocurrency), not a merchant
-- account, so each gateway kind may have at most one channel taking orders.
-- The enable API reports this with a coded error; these triggers keep direct
-- and legacy `enabled` writes honest. A deployment that already runs several
-- channels of one kind keeps them until one is disabled: only new activations
-- are refused, so the upgrade never blocks startup or stops live payments.
CREATE TRIGGER trg_market_payment_channel_single_active_method
BEFORE UPDATE OF rollout_state, gateway_kind, archived_at ON market_payment_channels
WHEN NEW.rollout_state != 'off' AND NEW.archived_at IS NULL
 AND (OLD.rollout_state = 'off' OR OLD.archived_at IS NOT NULL OR OLD.gateway_kind != NEW.gateway_kind)
 AND EXISTS (SELECT 1 FROM market_payment_channels other
             WHERE other.id != NEW.id AND other.gateway_kind = NEW.gateway_kind
               AND other.rollout_state != 'off' AND other.archived_at IS NULL)
BEGIN SELECT RAISE(ABORT, 'top-up method already has an active channel'); END;

CREATE TRIGGER trg_market_payment_channel_single_active_method_insert
BEFORE INSERT ON market_payment_channels
WHEN (NEW.rollout_state != 'off' OR NEW.enabled = 1) AND NEW.archived_at IS NULL
 AND EXISTS (SELECT 1 FROM market_payment_channels other
             WHERE other.gateway_kind = NEW.gateway_kind
               AND other.rollout_state != 'off' AND other.archived_at IS NULL)
BEGIN SELECT RAISE(ABORT, 'top-up method already has an active channel'); END;
