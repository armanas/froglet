use crate::crypto;
use std::str::FromStr;

#[derive(Debug, Clone)]
pub struct CashuTokenInfo {
    pub amount_satoshis: u64,
    pub token_hash: String,
}

pub fn inspect_cashu_token(
    token_str: &str,
) -> Result<CashuTokenInfo, Box<dyn std::error::Error + Send + Sync>> {
    let token = cashu::Token::from_str(token_str)?;
    let amount = token.value()?;

    Ok(CashuTokenInfo {
        amount_satoshis: amount.into(),
        token_hash: crypto::sha256_hex(token_str),
    })
}

pub fn verify_cashu_token(
    token_str: &str,
) -> Result<u64, Box<dyn std::error::Error + Send + Sync>> {
    let info = inspect_cashu_token(token_str)?;
    Ok(info.amount_satoshis)
}
