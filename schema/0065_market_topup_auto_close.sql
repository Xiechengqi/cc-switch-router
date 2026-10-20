-- New orders get a durable 30-minute automatic verification deadline.
-- Historical orders stay opt-out so a deploy cannot cancel an existing checkout.
ALTER TABLE market_topup_orders ADD COLUMN auto_close_at TEXT;
