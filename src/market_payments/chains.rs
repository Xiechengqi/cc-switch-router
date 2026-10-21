//! Static DujiaoPay settlement-rail catalogue.
//!
//! The values mirror the official merchant documentation (chain, token ID,
//! symbol and the minimum confirmations the gateway waits for before it marks
//! an order paid). A `token_id` outside this table is rejected at channel
//! configuration time instead of guessing its chain from the string shape.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TokenRail {
    pub token_id: &'static str,
    pub chain: &'static str,
    pub chain_name: &'static str,
    pub symbol: &'static str,
    pub min_confirmations: i64,
}

const fn rail(
    token_id: &'static str,
    chain: &'static str,
    chain_name: &'static str,
    symbol: &'static str,
    min_confirmations: i64,
) -> TokenRail {
    TokenRail {
        token_id,
        chain,
        chain_name,
        symbol,
        min_confirmations,
    }
}

pub(crate) const TOKEN_RAILS: &[TokenRail] = &[
    rail("tron-trx", "tron", "Tron", "TRX", 19),
    rail("tron-usdt", "tron", "Tron", "USDT", 19),
    rail("ethereum-eth", "ethereum", "Ethereum", "ETH", 20),
    rail("ethereum-usdt", "ethereum", "Ethereum", "USDT", 20),
    rail("ethereum-usdc", "ethereum", "Ethereum", "USDC", 20),
    rail("bsc-bnb", "bsc", "BSC", "BNB", 15),
    rail("bsc-usdt", "bsc", "BSC", "USDT", 15),
    rail("bsc-usdc", "bsc", "BSC", "USDC", 15),
    rail("polygon-usdc", "polygon", "Polygon", "USDC", 128),
    rail("polygon-usdt0", "polygon", "Polygon", "USDT0", 128),
    rail("base-usdc", "base", "Base", "USDC", 20),
    rail("arbitrum-usdc", "arbitrum", "Arbitrum", "USDC", 20),
    rail("arbitrum-usdt0", "arbitrum", "Arbitrum", "USDT0", 20),
    rail("plasma-usdt0", "plasma", "Plasma", "USDT0", 20),
    rail("x-layer-usdt0", "x-layer", "X Layer", "USDT0", 20),
    rail("solana-usdc", "solana", "Solana", "USDC", 32),
    rail("solana-usdt", "solana", "Solana", "USDT", 32),
    rail("aptos-usdc", "aptos", "Aptos", "USDC", 1),
    rail("aptos-usdt", "aptos", "Aptos", "USDT", 1),
];

impl TokenRail {
    /// A USD-pegged token, whose on-chain amount equals the fiat amount.
    pub(crate) fn usd_stable(&self) -> bool {
        matches!(self.symbol, "USDT" | "USDC" | "USDT0")
    }
}

pub(crate) fn token_rail(token_id: &str) -> Option<&'static TokenRail> {
    TOKEN_RAILS.iter().find(|rail| rail.token_id == token_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_token_id_is_unique_and_prefixed_by_its_chain() {
        for (index, rail) in TOKEN_RAILS.iter().enumerate() {
            assert!(rail.token_id.starts_with(&format!("{}-", rail.chain)));
            assert!(rail.min_confirmations > 0);
            assert!(
                TOKEN_RAILS[index + 1..]
                    .iter()
                    .all(|other| other.token_id != rail.token_id)
            );
        }
    }

    #[test]
    fn multi_segment_chain_resolves_from_the_table_not_the_string_shape() {
        assert_eq!(token_rail("x-layer-usdt0").unwrap().chain, "x-layer");
        assert_eq!(token_rail("tron-usdt").unwrap().min_confirmations, 19);
        assert!(token_rail("tron-usdt-trc20").is_none());
    }

    #[test]
    fn only_usd_pegged_tokens_are_stable() {
        assert!(token_rail("tron-usdt").unwrap().usd_stable());
        assert!(token_rail("polygon-usdt0").unwrap().usd_stable());
        assert!(!token_rail("tron-trx").unwrap().usd_stable());
        assert!(!token_rail("ethereum-eth").unwrap().usd_stable());
    }
}
