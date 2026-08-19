-- A user receives each notification on exactly one channel.
--
-- Enabled channels stay rows rather than an enum column so a future channel
-- still needs no migration, but "enabled" now means "selected": at most one row
-- per user may carry it. The revision bump is what invalidates deliveries that
-- were already frozen for the channel being switched away from; the delivery
-- worker re-queues their events onto the newly selected channel.
UPDATE user_notification_channels
   SET enabled = 0,
       revision = revision + 1,
       updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
 WHERE enabled = 1
   AND channel <> (
       SELECT preferred.channel
         FROM user_notification_channels preferred
        WHERE preferred.user_id = user_notification_channels.user_id
          AND preferred.enabled = 1
        ORDER BY preferred.channel <> 'telegram', preferred.channel
        LIMIT 1
   );

CREATE UNIQUE INDEX idx_user_notification_channel_selected
    ON user_notification_channels(user_id)
    WHERE enabled = 1;
