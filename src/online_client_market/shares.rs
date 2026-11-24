use super::*;

fn current_scope(
    c: &Connection,
    id: &str,
) -> Result<Option<(String, String, String, String)>, AppError> {
    c.query_row("SELECT id,alias,tenant_email,state FROM online_client_leases WHERE installation_id=?1 AND released_at IS NULL",params![id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional().market()
}
pub(crate) fn check_nonce(
    c: &Connection,
    id: &str,
    action: &str,
    nonce: &str,
) -> Result<(), AppError> {
    let lease = nonce
        .strip_prefix("lease_")
        .map(|v| v.split_once('_').map(|v| v.0).unwrap_or(""));
    // Physical heartbeat and the Client tunnel must survive a workspace switch.
    // Content writes and remote acknowledgements always identify the boot workspace.
    if lease.is_none()
        && !action.contains("share")
        && !action.contains("quota")
        && !action.contains("edit")
    {
        return Ok(());
    }
    let actual = current_scope(c, id)?.map(|r| r.0);
    if actual.as_deref() != lease {
        return Err(conflict("Client workspace has changed"));
    }
    Ok(())
}
pub(crate) fn namespace(c: &Connection, id: &str, physical: &str) -> Result<String, AppError> {
    Ok(current_scope(c, id)?
        .map(|r| r.1)
        .unwrap_or_else(|| physical.to_owned()))
}
pub(crate) fn share_owner(c: &Connection, id: &str, physical: &str) -> Result<String, AppError> {
    Ok(current_scope(c, id)?
        .map(|r| r.2)
        .unwrap_or_else(|| physical.to_owned()))
}
pub(crate) fn check_share_id(
    c: &Connection,
    installation: &str,
    share: &str,
) -> Result<(), AppError> {
    let binding: Option<String> = c
        .query_row(
            "SELECT lease_id FROM online_client_share_bindings WHERE share_id=?1",
            params![share],
            |r| r.get(0),
        )
        .optional()
        .market()?;
    let scope = current_scope(c, installation)?;
    if let Some(bound) = binding {
        if scope.as_ref().map(|r| r.0.as_str()) != Some(&bound) {
            return Err(conflict("Share belongs to another workspace"));
        }
    } else if let Some((_, _, _, state)) = scope {
        let exists: bool = c
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM shares WHERE share_id=?1)",
                params![share],
                |r| r.get(0),
            )
            .market()?;
        if exists || matches!(state.as_str(), "ending" | "cleaning") {
            return Err(conflict("Share belongs to a frozen workspace"));
        }
    }
    Ok(())
}
pub(crate) fn bind_share(
    c: &Connection,
    installation: &str,
    share: &crate::models::ShareDescriptor,
) -> Result<(), AppError> {
    check_share_id(c, installation, &share.share_id)?;
    if let Some((lease, alias, _, _)) = current_scope(c, installation)? {
        if share.subdomain.split_once("--").map(|v| v.1) != Some(alias.as_str()) {
            return Err(conflict("Share alias belongs to another workspace"));
        }
        c.execute(
            "INSERT OR IGNORE INTO online_client_share_bindings(share_id,lease_id) VALUES(?1,?2)",
            params![share.share_id, lease],
        )
        .market()?;
    }
    Ok(())
}
pub(crate) fn allow_traffic(
    c: &Connection,
    installation: &str,
    share: &str,
) -> Result<bool, AppError> {
    let matched:bool=c.query_row("SELECT CASE
     WHEN EXISTS(SELECT 1 FROM online_client_share_bindings WHERE share_id=?2)
      THEN EXISTS(SELECT 1 FROM online_client_share_bindings b JOIN online_client_leases r ON r.id=b.lease_id
         WHERE b.share_id=?2 AND r.installation_id=?1 AND r.state='active' AND r.grant_until>unixepoch())
      ELSE NOT EXISTS(SELECT 1 FROM online_client_leases WHERE installation_id=?1 AND released_at IS NULL) END",params![installation,share],|r|r.get(0)).market()?;
    Ok(matched)
}
