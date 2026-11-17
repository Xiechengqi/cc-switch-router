//! Optional independent chain verification. Trust anchors are deployment-owned,
//! never copied from a gateway response. Missing/unsupported anchors fail closed.
use super::*;
use serde::Deserialize;
use serde_json::{Value, json};
#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Anchor {
    channel_id: String,
    token_id: String,
    rpc_url: String,
    recipient: String,
    asset_id: String,
    decimals: u32,
    network_id: String,
    #[serde(default)]
    deposit_store: Option<String>,
    #[serde(default)]
    usd_micros_per_token: Option<u64>,
    #[serde(default)]
    price_valid_until: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Anchors {
    rails: Vec<Anchor>,
}
pub(super) struct Evidence {
    chain: String,
    token: String,
    tx: String,
    recipient: String,
    amount: String,
    anchor: Value,
    observation: Value,
    fingerprint: String,
}
fn fail(message: &str) -> AppError {
    AppError::coded_conflict("TOPUP_CHAIN_VERIFICATION_REQUIRED", message, json!({}))
}
fn atomic(value: &str, decimals: u32) -> Result<u128, AppError> {
    if decimals > 24 || value.is_empty() || value.len() > 64 {
        return Err(fail("invalid chain amount"));
    }
    let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
    if whole.is_empty()
        || !whole.bytes().all(|b| b.is_ascii_digit())
        || !fraction.bytes().all(|b| b.is_ascii_digit())
        || fraction.len() > decimals as usize
    {
        return Err(fail("invalid chain decimal"));
    }
    let scale = 10_u128
        .checked_pow(decimals)
        .ok_or_else(|| fail("chain amount overflow"))?;
    let major = whole
        .parse::<u128>()
        .ok()
        .and_then(|n| n.checked_mul(scale))
        .ok_or_else(|| fail("chain amount overflow"))?;
    let minor = if fraction.is_empty() {
        0
    } else {
        fraction
            .parse::<u128>()
            .ok()
            .and_then(|n| n.checked_mul(10_u128.pow(decimals - fraction.len() as u32)))
            .ok_or_else(|| fail("chain amount overflow"))?
    };
    major
        .checked_add(minor)
        .ok_or_else(|| fail("chain amount overflow"))
}
fn hex_int(value: &str) -> Result<u128, AppError> {
    let raw = value.strip_prefix("0x").unwrap_or(value);
    if raw.is_empty() || !raw.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(fail("invalid RPC integer"));
    }
    let n = raw.trim_start_matches('0');
    if n.is_empty() {
        return Ok(0);
    }
    u128::from_str_radix(n, 16).map_err(|_| fail("invalid RPC integer"))
}
fn str_at<'a>(v: &'a Value, p: &str) -> Result<&'a str, AppError> {
    v.pointer(p)
        .and_then(Value::as_str)
        .ok_or_else(|| fail("incomplete independent chain response"))
}
async fn bounded_json(response: reqwest::Response) -> Result<Value, AppError> {
    use futures_util::StreamExt;
    let response = response
        .error_for_status()
        .map_err(|_| fail("independent RPC unavailable"))?;
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| fail("independent RPC interrupted"))?;
        if bytes.len() + chunk.len() > 2 * 1024 * 1024 {
            return Err(fail("independent RPC response too large"));
        }
        bytes.extend_from_slice(&chunk)
    }
    serde_json::from_slice(&bytes).map_err(|_| fail("invalid independent RPC response"))
}
async fn rpc(
    client: &reqwest::Client,
    url: &str,
    method: &str,
    params: Value,
) -> Result<Value, AppError> {
    let v = bounded_json(
        client
            .post(url)
            .json(&json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}))
            .send()
            .await
            .map_err(|_| fail("independent RPC unavailable"))?,
    )
    .await?;
    if !v["error"].is_null() {
        return Err(fail("independent RPC rejected lookup"));
    }
    v.get("result")
        .filter(|v| !v.is_null())
        .cloned()
        .ok_or_else(|| fail("transaction not found by independent RPC"))
}
fn address(value: &str) -> String {
    value
        .trim_start_matches("0x")
        .trim_start_matches('0')
        .to_ascii_lowercase()
}
fn evm_address(value: &str) -> Option<String> {
    let s = value.strip_prefix("0x").unwrap_or(value);
    (s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit())).then(|| s.to_ascii_lowercase())
}
fn evm_transfers(receipt: &Value, asset: &str, recipient: &str) -> Result<u128, AppError> {
    let asset = evm_address(asset).ok_or_else(|| fail("invalid anchored token contract"))?;
    let recipient = evm_address(recipient).ok_or_else(|| fail("invalid anchored recipient"))?;
    let mut incoming = 0_u128;
    let mut outgoing = 0_u128;
    for log in receipt["logs"]
        .as_array()
        .ok_or_else(|| fail("RPC receipt logs missing"))?
    {
        if evm_address(log["address"].as_str().unwrap_or("")) != Some(asset.clone())
            || log["removed"].as_bool() == Some(true)
        {
            continue;
        }
        let Some(topics) = log["topics"].as_array() else {
            continue;
        };
        if topics.len() != 3
            || topics[0]
                .as_str()
                .unwrap_or("")
                .trim_start_matches("0x")
                .to_ascii_lowercase()
                != "ddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef"
        {
            continue;
        }
        let account = |v: &Value| -> Option<String> {
            let s = v.as_str()?.strip_prefix("0x").unwrap_or(v.as_str()?);
            if s.len() != 64
                || !s.bytes().all(|b| b.is_ascii_hexdigit())
                || !s[..24].bytes().all(|b| b == b'0')
            {
                return None;
            }
            Some(s[24..].to_ascii_lowercase())
        };
        let amount = hex_int(log["data"].as_str().unwrap_or(""))?;
        if account(&topics[2]).as_deref() == Some(recipient.as_str()) {
            incoming = incoming
                .checked_add(amount)
                .ok_or_else(|| fail("chain credit overflow"))?;
        }
        if account(&topics[1]).as_deref() == Some(recipient.as_str()) {
            outgoing = outgoing
                .checked_add(amount)
                .ok_or_else(|| fail("chain debit overflow"))?;
        }
    }
    incoming
        .checked_sub(outgoing)
        .ok_or_else(|| fail("recipient has a net token withdrawal"))
}

async fn verify_evm(
    client: &reqwest::Client,
    a: &Anchor,
    tx: &str,
    min: i64,
) -> Result<(u128, Value), AppError> {
    let network = rpc(client, &a.rpc_url, "eth_chainId", json!([])).await?;
    if hex_int(network.as_str().unwrap_or(""))? != hex_int(&a.network_id)? {
        return Err(fail("independent RPC network mismatch"));
    }
    let receipt = rpc(client, &a.rpc_url, "eth_getTransactionReceipt", json!([tx])).await?;
    if str_at(&receipt, "/transactionHash")?.to_ascii_lowercase() != tx.to_ascii_lowercase()
        || hex_int(str_at(&receipt, "/status")?)? != 1
    {
        return Err(fail("independent chain transaction failed"));
    }
    let block = str_at(&receipt, "/blockNumber")?;
    let canonical = rpc(
        client,
        &a.rpc_url,
        "eth_getBlockByNumber",
        json!([block, false]),
    )
    .await?;
    if str_at(&canonical, "/hash")? != str_at(&receipt, "/blockHash")? {
        return Err(fail("independent receipt block is not canonical"));
    }
    let tip = rpc(client, &a.rpc_url, "eth_blockNumber", json!([])).await?;
    let height = hex_int(block)?;
    let head = hex_int(tip.as_str().unwrap_or(""))?;
    if head < height || head - height + 1 < min as u128 {
        return Err(fail("independent chain confirmations too low"));
    }
    let amount = if a.asset_id == "native" {
        let recipient =
            evm_address(&a.recipient).ok_or_else(|| fail("invalid native recipient anchor"))?;
        let code = rpc(
            client,
            &a.rpc_url,
            "eth_getCode",
            json!([a.recipient, block]),
        )
        .await?;
        if code.as_str() != Some("0x") {
            return Err(fail(
                "native deposits require an externally owned recipient",
            ));
        }
        let transaction = rpc(client, &a.rpc_url, "eth_getTransactionByHash", json!([tx])).await?;
        if transaction["hash"] != receipt["transactionHash"]
            || transaction["blockHash"] != receipt["blockHash"]
            || evm_address(str_at(&transaction, "/to")?).as_deref() != Some(recipient.as_str())
            || evm_address(str_at(&transaction, "/from")?).as_deref() == Some(recipient.as_str())
        {
            return Err(fail("independent native recipient mismatch"));
        }
        hex_int(str_at(&transaction, "/value")?)?
    } else {
        evm_transfers(&receipt, &a.asset_id, &a.recipient)?
    };
    Ok((
        amount,
        json!({"receipt":receipt,"canonicalBlockHash":canonical["hash"],"head":tip,"network":network}),
    ))
}
async fn tron_call(
    client: &reqwest::Client,
    a: &Anchor,
    path: &str,
    body: Value,
) -> Result<Value, AppError> {
    bounded_json(
        client
            .post(format!("{}/{}", a.rpc_url.trim_end_matches('/'), path))
            .json(&body)
            .send()
            .await
            .map_err(|_| fail("independent Tron RPC unavailable"))?,
    )
    .await
}
fn tron_hex(value: &str) -> Result<String, AppError> {
    if value.starts_with('T') {
        let raw = bs58::decode(value)
            .with_check(None)
            .into_vec()
            .map_err(|_| fail("invalid Tron recipient"))?;
        if raw.len() != 21 || raw[0] != 0x41 {
            return Err(fail("invalid Tron address network"));
        }
        Ok(hex::encode(raw))
    } else {
        let s = value.trim_start_matches("0x");
        if s.len() != 42 || !s.starts_with("41") || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(fail("invalid Tron address"));
        }
        Ok(s.to_ascii_lowercase())
    }
}
async fn verify_tron(
    client: &reqwest::Client,
    a: &Anchor,
    tx: &str,
    min: i64,
) -> Result<(u128, Value), AppError> {
    let genesis = tron_call(client, a, "wallet/getblockbynum", json!({"num":0})).await?;
    if str_at(&genesis, "/blockID")? != a.network_id {
        return Err(fail("independent Tron network mismatch"));
    }
    let receipt = tron_call(
        client,
        a,
        "walletsolidity/gettransactioninfobyid",
        json!({"value":tx}),
    )
    .await?;
    let transaction = tron_call(
        client,
        a,
        "walletsolidity/gettransactionbyid",
        json!({"value":tx}),
    )
    .await?;
    if receipt["id"].as_str() != Some(tx)
        || transaction["txID"].as_str() != Some(tx)
        || transaction
            .pointer("/ret/0/contractRet")
            .and_then(Value::as_str)
            != Some("SUCCESS")
        || receipt
            .pointer("/receipt/result")
            .and_then(Value::as_str)
            .is_some_and(|v| v != "SUCCESS")
    {
        return Err(fail("independent Tron transaction failed"));
    }
    let head = tron_call(client, a, "walletsolidity/getnowblock", json!({})).await?;
    let tip = head
        .pointer("/block_header/raw_data/number")
        .and_then(Value::as_u64)
        .ok_or_else(|| fail("Tron head missing"))?;
    let height = receipt["blockNumber"]
        .as_u64()
        .ok_or_else(|| fail("Tron receipt block missing"))?;
    if tip < height || tip - height + 1 < min as u64 {
        return Err(fail("independent Tron confirmations too low"));
    }
    let recipient = tron_hex(&a.recipient)?;
    let amount = if a.asset_id == "native" {
        let c = transaction
            .pointer("/raw_data/contract/0")
            .ok_or_else(|| fail("Tron transfer missing"))?;
        if c["type"] != "TransferContract"
            || transaction
                .pointer("/raw_data/contract")
                .and_then(Value::as_array)
                .is_none_or(|v| v.len() != 1)
            || tron_hex(str_at(c, "/parameter/value/owner_address")?)? == recipient
            || tron_hex(str_at(c, "/parameter/value/to_address")?)? != recipient
        {
            return Err(fail("Tron native recipient mismatch"));
        }
        c.pointer("/parameter/value/amount")
            .and_then(Value::as_u64)
            .map(u128::from)
            .ok_or_else(|| fail("Tron transfer amount missing"))?
    } else {
        let asset = tron_hex(&a.asset_id)?;
        let mut evm = receipt.clone();
        let logs = receipt["log"]
            .as_array()
            .ok_or_else(|| fail("Tron token logs missing"))?;
        evm["logs"]=Value::Array(logs.iter().map(|log|json!({"address":format!("0x{}",log["address"].as_str().unwrap_or("")),"topics":log["topics"],"data":log["data"]})).collect());
        evm_transfers(&evm, &asset[2..], &recipient[2..])?
    };
    Ok((
        amount,
        json!({"receipt":receipt,"transaction":transaction,"solidHead":tip,"genesis":a.network_id}),
    ))
}
async fn verify_solana(
    client: &reqwest::Client,
    a: &Anchor,
    tx: &str,
    min: i64,
) -> Result<(u128, Value), AppError> {
    if rpc(client, &a.rpc_url, "getGenesisHash", json!([]))
        .await?
        .as_str()
        != Some(a.network_id.as_str())
    {
        return Err(fail("independent Solana network mismatch"));
    }
    let v=rpc(client,&a.rpc_url,"getTransaction",json!([tx,{"encoding":"jsonParsed","commitment":"finalized","maxSupportedTransactionVersion":0}])).await?;
    if !v.pointer("/meta/err").is_some_and(Value::is_null)
        || v.pointer("/transaction/signatures/0")
            .and_then(Value::as_str)
            != Some(tx)
    {
        return Err(fail("independent Solana transaction failed"));
    }
    let slot = v["slot"]
        .as_u64()
        .ok_or_else(|| fail("Solana transaction slot missing"))?;
    let head = rpc(
        client,
        &a.rpc_url,
        "getSlot",
        json!([{"commitment":"finalized"}]),
    )
    .await?
    .as_u64()
    .ok_or_else(|| fail("Solana finalized slot missing"))?;
    if head < slot || head - slot + 1 < min as u64 {
        return Err(fail("Solana finality insufficient"));
    }
    let total_for = |pointer: &str| -> Result<u128, AppError> {
        let mut total = 0_u128;
        let mut indices = std::collections::HashSet::new();
        for b in v
            .pointer(pointer)
            .and_then(Value::as_array)
            .ok_or_else(|| fail("Solana token balances missing"))?
        {
            if b["mint"].as_str() != Some(a.asset_id.as_str())
                || b["owner"].as_str() != Some(a.recipient.as_str())
            {
                continue;
            }
            let index = b["accountIndex"]
                .as_u64()
                .ok_or_else(|| fail("Solana balance index missing"))?;
            if !indices.insert(index)
                || b.pointer("/uiTokenAmount/decimals").and_then(Value::as_u64)
                    != Some(a.decimals as u64)
            {
                return Err(fail("ambiguous Solana balance"));
            }
            total = total
                .checked_add(
                    str_at(b, "/uiTokenAmount/amount")?
                        .parse::<u128>()
                        .map_err(|_| fail("invalid Solana balance"))?,
                )
                .ok_or_else(|| fail("Solana balance overflow"))?;
        }
        Ok(total)
    };
    let before = total_for("/meta/preTokenBalances")?;
    let after = total_for("/meta/postTokenBalances")?;
    let total = after
        .checked_sub(before)
        .ok_or_else(|| fail("Solana recipient has a net token withdrawal"))?;
    Ok((
        total,
        json!({"transaction":v,"finalizedSlot":head,"genesis":a.network_id}),
    ))
}
async fn aptos_get(client: &reqwest::Client, a: &Anchor, path: &str) -> Result<Value, AppError> {
    bounded_json(
        client
            .get(format!("{}/{}", a.rpc_url.trim_end_matches('/'), path))
            .send()
            .await
            .map_err(|_| fail("independent Aptos RPC unavailable"))?,
    )
    .await
}
async fn verify_aptos(
    client: &reqwest::Client,
    a: &Anchor,
    tx: &str,
) -> Result<(u128, Value), AppError> {
    let ledger = aptos_get(client, a, "").await?;
    if ledger["chain_id"]
        .as_u64()
        .map(|n| n.to_string())
        .as_deref()
        != Some(a.network_id.as_str())
    {
        return Err(fail("independent Aptos network mismatch"));
    }
    let v = aptos_get(client, a, &format!("transactions/by_hash/{tx}")).await?;
    if v["hash"].as_str() != Some(tx) || v["success"] != true || v["type"] != "user_transaction" {
        return Err(fail("independent Aptos transaction failed"));
    }
    let mut total = 0_u128;
    let mut outgoing = 0_u128;
    let events = v["events"]
        .as_array()
        .ok_or_else(|| fail("Aptos events missing"))?;
    if let Some(store) = a.deposit_store.as_deref() {
        // Fungible stores bind asset metadata and owner independently of the
        // payment gateway, queried at this exact finalized ledger version.
        if !store.starts_with("0x") || !store[2..].bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(fail("invalid anchored Aptos store"));
        }
        let version = str_at(&v, "/version")?;
        if !version.bytes().all(|b| b.is_ascii_digit()) {
            return Err(fail("invalid Aptos ledger version"));
        }
        let resources = aptos_get(
            client,
            a,
            &format!("accounts/{store}/resources?ledger_version={version}"),
        )
        .await?;
        let resources = resources
            .as_array()
            .ok_or_else(|| fail("Aptos store resources missing"))?;
        let owner = resources
            .iter()
            .find(|r| r["type"] == "0x1::object::ObjectCore")
            .and_then(|r| r.pointer("/data/owner"))
            .and_then(Value::as_str)
            .ok_or_else(|| fail("Aptos store owner missing"))?;
        let metadata = resources
            .iter()
            .find(|r| r["type"] == "0x1::fungible_asset::FungibleStore")
            .and_then(|r| r.pointer("/data/metadata/inner"))
            .and_then(Value::as_str)
            .ok_or_else(|| fail("Aptos store metadata missing"))?;
        if address(owner) != address(&a.recipient) || address(metadata) != address(&a.asset_id) {
            return Err(fail("Aptos deposit store anchor mismatch"));
        }
        for e in events {
            if matches!(
                e["type"].as_str(),
                Some("0x1::fungible_asset::Deposit" | "0x1::fungible_asset::Withdraw")
            ) && address(
                e.pointer("/data/store")
                    .and_then(Value::as_str)
                    .unwrap_or(""),
            ) == address(store)
            {
                let target = if e["type"] == "0x1::fungible_asset::Withdraw" {
                    &mut outgoing
                } else {
                    &mut total
                };
                *target = target
                    .checked_add(
                        str_at(e, "/data/amount")?
                            .parse::<u128>()
                            .map_err(|_| fail("invalid Aptos amount"))?,
                    )
                    .ok_or_else(|| fail("Aptos amount overflow"))?
            }
        }
    } else {
        let expected_type = format!("0x1::coin::CoinStore<{}>", a.asset_id);
        let store = v["changes"]
            .as_array()
            .ok_or_else(|| fail("Aptos changes missing"))?
            .iter()
            .find(|r| {
                r["type"] == "write_resource"
                    && r.pointer("/data/type").and_then(Value::as_str)
                        == Some(expected_type.as_str())
                    && address(r["address"].as_str().unwrap_or("")) == address(&a.recipient)
            })
            .ok_or_else(|| fail("anchored Aptos coin store missing"))?;
        let guid = store
            .pointer("/data/data/deposit_events/guid/id")
            .ok_or_else(|| fail("Aptos deposit event identity missing"))?;
        let withdraw_guid = store
            .pointer("/data/data/withdraw_events/guid/id")
            .ok_or_else(|| fail("Aptos withdraw event identity missing"))?;
        for e in events {
            let withdrawing = e["type"] == "0x1::coin::WithdrawEvent";
            if (e["type"] == "0x1::coin::DepositEvent" || withdrawing)
                && e.pointer("/guid/account_address")
                    .and_then(Value::as_str)
                    .is_some_and(|s| address(s) == address(&a.recipient))
                && e.pointer("/guid/creation_number")
                    == if withdrawing {
                        withdraw_guid.get("creation_num")
                    } else {
                        guid.get("creation_num")
                    }
            {
                let target = if withdrawing {
                    &mut outgoing
                } else {
                    &mut total
                };
                *target = target
                    .checked_add(
                        str_at(e, "/data/amount")?
                            .parse::<u128>()
                            .map_err(|_| fail("invalid Aptos coin amount"))?,
                    )
                    .ok_or_else(|| fail("Aptos amount overflow"))?
            }
        }
    }
    let total = total
        .checked_sub(outgoing)
        .ok_or_else(|| fail("Aptos recipient has a net withdrawal"))?;
    Ok((total, json!({"transaction":v,"chainId":ledger["chain_id"]})))
}
pub(super) async fn verify(
    channel: &str,
    gateway: &GatewayOrder,
) -> Result<Option<Evidence>, AppError> {
    let Some(path) = std::env::var_os("MARKET_CHAIN_ANCHORS_FILE") else {
        return Ok(None);
    };
    let raw = tokio::fs::read(path)
        .await
        .map_err(|_| fail("independent chain anchors are unavailable"))?;
    if raw.len() > 128 * 1024 {
        return Err(fail("chain anchor configuration too large"));
    }
    let config: Anchors =
        serde_json::from_slice(&raw).map_err(|_| fail("invalid independent chain anchors"))?;
    let token = gateway
        .token_id
        .as_deref()
        .ok_or_else(|| fail("chain token missing"))?;
    let Some(a) = config
        .rails
        .into_iter()
        .find(|a| a.channel_id == channel && a.token_id == token)
    else {
        return Ok(None);
    };
    let rail = token_rail(token).ok_or_else(|| fail("unsupported settlement rail"))?;
    let tx = gateway
        .tx_id
        .as_deref()
        .filter(|s| {
            !s.is_empty()
                && s.len() <= 200
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        })
        .ok_or_else(|| fail("invalid chain transaction identifier"))?;
    let expected = atomic(
        gateway
            .payable_amount
            .as_deref()
            .ok_or_else(|| fail("chain payable amount missing"))?,
        a.decimals,
    )?;
    let reported = atomic(
        gateway
            .settled_amount
            .as_deref()
            .ok_or_else(|| fail("chain settled amount missing"))?,
        a.decimals,
    )?;
    if expected == 0 || reported < expected {
        return Err(fail("chain settlement amount insufficient"));
    }
    let url = reqwest::Url::parse(&a.rpc_url).map_err(|_| fail("invalid independent RPC URL"))?;
    if url.scheme() != "https"
        && !(url.scheme() == "http"
            && matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]")))
    {
        return Err(fail("independent RPC requires HTTPS or a local node"));
    }
    if a.recipient.is_empty() || a.asset_id.is_empty() || a.network_id.is_empty() {
        return Err(fail(
            "independent recipient, asset and network anchors required",
        ));
    }
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(8))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| fail("independent RPC client unavailable"))?;
    let check = async {
        match rail.chain {
            "tron" => verify_tron(&client, &a, tx, rail.min_confirmations).await,
            "solana" => verify_solana(&client, &a, tx, rail.min_confirmations).await,
            "aptos" => verify_aptos(&client, &a, tx).await,
            _ => verify_evm(&client, &a, tx, rail.min_confirmations).await,
        }
    };
    let (amount, observation) = tokio::time::timeout(std::time::Duration::from_secs(25), check)
        .await
        .map_err(|_| fail("independent chain verification timed out"))??;
    // The full claimed settlement must be independently visible, not merely
    // enough to cover an attacker-controlled smaller payable figure.
    if amount < reported {
        return Err(fail(
            "independent recipient or amount does not match the settlement",
        ));
    }
    if !rail.usd_stable()
        && a.price_valid_until
            .as_deref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .is_none_or(|t| t <= Utc::now())
    {
        return Err(fail(
            "native-token verification requires a fresh independent USD price expiry",
        ));
    }
    let rate = if rail.usd_stable() {
        1_000_000_u128
    } else {
        u128::from(a.usd_micros_per_token.filter(|n|*n>0).ok_or_else(||fail("native-token verification requires an independently configured conservative USD price"))?)
    };
    let value = amount
        .checked_mul(rate)
        .map(|n| n / 10_u128.pow(a.decimals))
        .ok_or_else(|| fail("independent USD valuation overflow"))?;
    let expected_usd = u128::try_from(gateway.fiat_amount_minor)
        .ok()
        .and_then(|v| v.checked_mul(10_000))
        .ok_or_else(|| fail("invalid fiat amount"))?;
    if value < expected_usd {
        return Err(fail(
            "independently valued deposit is below the local USD order amount",
        ));
    }
    let canonical_tx = if matches!(rail.chain, "solana") {
        tx.to_owned()
    } else {
        tx.to_ascii_lowercase()
    };
    let anchor = json!({"usdMicrosPerToken":rate.to_string(),"fiatMinor":gateway.fiat_amount_minor,"recipient":a.recipient,"assetId":a.asset_id,"networkId":a.network_id,"decimals":a.decimals,"rpcHost":url.host_str(),"depositStore":a.deposit_store,"priceValidUntil":a.price_valid_until});
    Ok(Some(Evidence {
        chain: rail.chain.into(),
        token: token.into(),
        tx: canonical_tx.clone(),
        recipient: a.recipient.clone(),
        amount: amount.to_string(),
        anchor,
        observation,
        fingerprint: hex::encode(Sha256::digest(format!(
            "{}:{}:{}",
            rail.chain, token, canonical_tx
        ))),
    }))
}
pub(super) fn save_tx(
    conn: &Connection,
    order: &str,
    e: Evidence,
    time: &str,
) -> Result<(), AppError> {
    let prior: Option<String> = conn
        .query_row(
            "SELECT order_id FROM market_chain_verification_evidence WHERE chain=?1 AND token_id=?2 AND tx_id=?3 LIMIT 1",
            params![e.chain,e.token,e.tx],
            |r| r.get(0),
        )
        .optional()
        .map_err(db_error("check independent receipt reuse"))?;
    if prior.as_deref().is_some_and(|prior| prior != order) {
        return Err(fail("independent chain transaction was already used"));
    }
    // Retain immutable observations when a native valuation anchor is refreshed.
    // An unchanged anchor reuses its evidence without storing every head poll.
    let fingerprint = hex::encode(Sha256::digest(format!("{}:{}", e.fingerprint, e.anchor)));
    conn.execute("INSERT OR IGNORE INTO market_chain_verification_evidence(id,order_id,chain,token_id,tx_id,recipient,amount_atomic,anchor_json,observation_json,fingerprint,verified_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",params![Uuid::new_v4().to_string(),order,e.chain,e.token,e.tx,e.recipient,e.amount,e.anchor.to_string(),e.observation.to_string(),fingerprint,time]).map_err(db_error("save independent chain evidence"))?;
    Ok(())
}
pub(super) fn evidence_matches_tx(
    conn: &Connection,
    order: &str,
    g: &GatewayOrder,
) -> Result<bool, AppError> {
    let tx = g.tx_id.as_deref().map(|s| {
        if g.chain.as_deref() == Some("solana") {
            s.to_owned()
        } else {
            s.to_ascii_lowercase()
        }
    });
    let evidence:Option<(String,String,String)>=conn.query_row("SELECT amount_atomic,anchor_json,recipient FROM market_chain_verification_evidence WHERE order_id=?1 AND chain=?2 AND token_id=?3 AND tx_id=?4 ORDER BY verified_at DESC LIMIT 1",params![order,g.chain,g.token_id,tx],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(db_error("read independent chain evidence"))?;
    let Some((amount, anchor, recipient)) = evidence else {
        return Ok(false);
    };
    let anchor: Value = serde_json::from_str(&anchor)
        .map_err(|_| AppError::Internal("invalid stored chain anchor".into()))?;
    let decimals = anchor["decimals"].as_u64().unwrap_or(255) as u32;
    let Some(settled) = g.settled_amount.as_deref() else {
        return Ok(false);
    };
    let expected = atomic(settled, decimals)?;
    let native = g
        .token_id
        .as_deref()
        .and_then(token_rail)
        .is_some_and(|r| !r.usd_stable());
    if native
        && anchor["priceValidUntil"]
            .as_str()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .is_none_or(|t| t <= Utc::now())
    {
        return Ok(false);
    }
    Ok(anchor["fiatMinor"].as_i64() == Some(g.fiat_amount_minor)
        && amount.parse::<u128>().is_ok_and(|n| n >= expected)
        && anchor["recipient"].as_str() == Some(recipient.as_str()))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn chain_decimal_is_exact_and_rejects_rounding() {
        assert_eq!(atomic("1.234567", 6).unwrap(), 1_234_567);
        assert!(atomic("1.2345671", 6).is_err());
        assert!(atomic("1e6", 6).is_err());
        assert!(atomic("-1", 6).is_err());
        assert_eq!(hex_int("0x0").unwrap(), 0);
    }
    #[test]
    fn independent_evm_receipt_requires_anchored_contract_and_recipient() {
        let r = json!({"logs":[{"address":"0x1111111111111111111111111111111111111111","topics":["0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef","0x0000000000000000000000000000000000000000000000000000000000000000","0x0000000000000000000000002222222222222222222222222222222222222222"],"data":"0x64"}]});
        assert_eq!(
            evm_transfers(
                &r,
                "1111111111111111111111111111111111111111",
                "2222222222222222222222222222222222222222"
            )
            .unwrap(),
            100
        );
        assert_eq!(
            evm_transfers(
                &r,
                "3333333333333333333333333333333333333333",
                "2222222222222222222222222222222222222222"
            )
            .unwrap(),
            0
        );
    }
    async fn mock_node(replies: Value) -> (String, tokio::task::JoinHandle<()>) {
        use axum::{
            Json,
            extract::{Request, State},
            routing::any,
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = axum::Router::new()
            .fallback(any(
                |State(replies): State<Value>, request: Request| async move {
                    let path = request.uri().path().to_owned();
                    let body = axum::body::to_bytes(request.into_body(), 1024 * 1024)
                        .await
                        .unwrap();
                    let input: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
                    if let Some(method) = input["method"].as_str() {
                        Json(json!({"jsonrpc":"2.0","id":1,"result":replies[method]}))
                    } else {
                        Json(replies[&path].clone())
                    }
                },
            ))
            .with_state(replies);
        let handle = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{address}"), handle)
    }
    fn anchor(url: String) -> Anchor {
        Anchor {
            channel_id: "channel".into(),
            token_id: "ethereum-usdc".into(),
            rpc_url: url,
            recipient: "0x2222222222222222222222222222222222222222".into(),
            asset_id: "0x1111111111111111111111111111111111111111".into(),
            decimals: 6,
            network_id: "0x1".into(),
            deposit_store: None,
            usd_micros_per_token: None,
            price_valid_until: None,
        }
    }
    fn transfer_log(from: &str, to: &str, amount: &str) -> Value {
        json!({"address":"0x1111111111111111111111111111111111111111","topics":["0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef",format!("0x{:0>64}",from),format!("0x{:0>64}",to)],"data":amount})
    }
    #[test]
    fn token_receipt_uses_net_credit_and_rejects_offsetting_withdrawals() {
        let receiver = "2222222222222222222222222222222222222222";
        let deposit = transfer_log("0", receiver, "0x64");
        let withdrawal = transfer_log(receiver, "3333333333333333333333333333333333333333", "0x32");
        assert_eq!(
            evm_transfers(
                &json!({"logs":[deposit.clone(),withdrawal]}),
                "1111111111111111111111111111111111111111",
                receiver
            )
            .unwrap(),
            50
        );
        let self_transfer = transfer_log(receiver, receiver, "0x64");
        assert_eq!(
            evm_transfers(
                &json!({"logs":[self_transfer]}),
                "1111111111111111111111111111111111111111",
                receiver
            )
            .unwrap(),
            0
        );
        let malformed = transfer_log("0", "中文", "0x64");
        assert_eq!(
            evm_transfers(
                &json!({"logs":[malformed]}),
                "1111111111111111111111111111111111111111",
                receiver
            )
            .unwrap(),
            0
        );
    }
    #[tokio::test]
    async fn evm_rpc_checks_network_receipt_canonical_block_confirmations_and_anchor() {
        let log = transfer_log("0", "2222222222222222222222222222222222222222", "0x64");
        let replies = json!({"eth_chainId":"0x1","eth_getTransactionReceipt":{"transactionHash":"0xabc","status":"0x1","blockNumber":"0x10","blockHash":"canonical","logs":[log]},"eth_getBlockByNumber":{"hash":"canonical"},"eth_blockNumber":"0x30"});
        let (url, handle) = mock_node(replies).await;
        let mut a = anchor(url);
        let client = reqwest::Client::new();
        assert_eq!(verify_evm(&client, &a, "0xabc", 20).await.unwrap().0, 100);
        assert!(verify_evm(&client, &a, "0xabc", 100).await.is_err());
        assert!(verify_evm(&client, &a, "0xother", 20).await.is_err());
        a.network_id = "0x2".into();
        assert!(verify_evm(&client, &a, "0xabc", 20).await.is_err());
        a.network_id = "0x1".into();
        a.recipient = "0x3333333333333333333333333333333333333333".into();
        assert_eq!(verify_evm(&client, &a, "0xabc", 20).await.unwrap().0, 0);
        handle.abort();
    }
    #[tokio::test]
    async fn tron_rpc_binds_solidified_transfer_recipient_network_and_confirmation_count() {
        let replies = json!({"/wallet/getblockbynum":{"blockID":"tron-genesis"},"/walletsolidity/gettransactioninfobyid":{"id":"tx","blockNumber":10,"receipt":{"result":"SUCCESS"}},"/walletsolidity/gettransactionbyid":{"txID":"tx","ret":[{"contractRet":"SUCCESS"}],"raw_data":{"contract":[{"type":"TransferContract","parameter":{"value":{"to_address":"412222222222222222222222222222222222222222","owner_address":"411111111111111111111111111111111111111111","amount":100}}}]}},"/walletsolidity/getnowblock":{"block_header":{"raw_data":{"number":30}}}});
        let (url, handle) = mock_node(replies).await;
        let mut a = anchor(url);
        a.network_id = "tron-genesis".into();
        a.asset_id = "native".into();
        a.recipient = "412222222222222222222222222222222222222222".into();
        let client = reqwest::Client::new();
        assert_eq!(verify_tron(&client, &a, "tx", 19).await.unwrap().0, 100);
        assert!(verify_tron(&client, &a, "tx", 50).await.is_err());
        a.recipient = "413333333333333333333333333333333333333333".into();
        assert!(verify_tron(&client, &a, "tx", 19).await.is_err());
        handle.abort();
    }
    #[tokio::test]
    async fn solana_finalized_balances_include_closed_accounts_and_net_withdrawals() {
        let balance = |amount: &str| json!({"accountIndex":0,"mint":"mint","owner":"treasury","uiTokenAmount":{"amount":amount,"decimals":6}});
        for (pre, post, expected) in [
            (json!([balance("10")]), json!([balance("110")]), Some(100)),
            (json!([balance("110")]), json!([balance("10")]), None),
            (json!([balance("110")]), json!([]), None),
        ] {
            let replies = json!({"getGenesisHash":"solana-genesis","getSlot":100,"getTransaction":{"slot":10,"transaction":{"signatures":["signature"]},"meta":{"err":null,"preTokenBalances":pre,"postTokenBalances":post}}});
            let (url, handle) = mock_node(replies).await;
            let mut a = anchor(url);
            a.network_id = "solana-genesis".into();
            a.asset_id = "mint".into();
            a.recipient = "treasury".into();
            let result = verify_solana(&reqwest::Client::new(), &a, "signature", 32).await;
            if let Some(amount) = expected {
                assert_eq!(result.unwrap().0, amount)
            } else {
                assert!(result.is_err())
            }
            handle.abort();
        }
    }
    #[tokio::test]
    async fn aptos_fungible_store_checks_metadata_owner_and_offsets_withdrawals() {
        let replies = json!({"/":{"chain_id":1},"/transactions/by_hash/0xabc":{"hash":"0xabc","success":true,"type":"user_transaction","version":"10","events":[{"type":"0x1::fungible_asset::Deposit","data":{"store":"0x333","amount":"100"}},{"type":"0x1::fungible_asset::Withdraw","data":{"store":"0x333","amount":"20"}}]},"/accounts/0x333/resources":[{"type":"0x1::object::ObjectCore","data":{"owner":"0x222"}},{"type":"0x1::fungible_asset::FungibleStore","data":{"metadata":{"inner":"0x111"}}}]});
        let (url, handle) = mock_node(replies).await;
        let mut a = anchor(url);
        a.network_id = "1".into();
        a.asset_id = "0x111".into();
        a.recipient = "0x222".into();
        a.deposit_store = Some("0x333".into());
        assert_eq!(
            verify_aptos(&reqwest::Client::new(), &a, "0xabc")
                .await
                .unwrap()
                .0,
            80
        );
        a.asset_id = "0x444".into();
        assert!(
            verify_aptos(&reqwest::Client::new(), &a, "0xabc")
                .await
                .is_err()
        );
        handle.abort();
    }
}
