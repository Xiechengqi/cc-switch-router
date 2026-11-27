//! The marketplace decision is local to Router; host delivery is an outbox.
use super::*;

pub(super) fn request_unlist(c: &Connection, id: &str, now: i64) -> Result<(), AppError> {
    c.execute("UPDATE online_client_listings SET state='unlisted',unlist_pending=1,
        unlisted_at=?2,revision=revision+1,challenge=NULL,challenge_until=NULL,paused_at=NULL,updated_at=?2
        WHERE installation_id=?1 AND state!='unlisted'",params![id,now]).market()?;
    schedule_unlist(c, id)
}

pub(super) fn schedule_unlist(c: &Connection, id: &str) -> Result<(), AppError> {
    let ready:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM online_client_listings l
        WHERE l.installation_id=?1 AND l.state='unlisted' AND l.unlist_pending=1
        AND NOT EXISTS(SELECT 1 FROM online_client_leases r WHERE r.installation_id=?1 AND r.released_at IS NULL)
        AND NOT EXISTS(SELECT 1 FROM client_market_terminations t WHERE t.host_id=?1 AND (t.cleanup_confirmed_at IS NULL OR t.status='finance_pending'))
        AND NOT EXISTS(SELECT 1 FROM online_client_operations o WHERE o.installation_id=?1 AND o.sequence=l.sequence AND json_extract(o.request_json,'$.action.operation')='unlist'))",params![id],|r|r.get(0)).market()?;
    if ready {
        let seq = advance(c, id)?;
        queue(c, id, seq, json!({"operation":"unlist"}))?;
    }
    Ok(())
}
