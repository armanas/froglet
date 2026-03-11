use crate::crypto;
use cashu::{Token, dhke, nuts::PublicKey};
use std::str::FromStr;

#[derive(Debug, Clone)]
pub struct CashuTokenInfo {
    pub amount_satoshis: u64,
    pub token_hash: String,
    pub mint_url: String,
    pub proof_ys: Vec<PublicKey>,
    pub has_spend_conditions: bool,
    pub p2pk_pubkeys: Vec<String>,
    pub p2pk_refund_pubkeys: Vec<String>,
}

pub fn inspect_cashu_token(
    token_str: &str,
) -> Result<CashuTokenInfo, Box<dyn std::error::Error + Send + Sync>> {
    let token = Token::from_str(token_str)?;
    let amount = token.value()?;
    let mint_url = token.mint_url()?.to_string();
    let proof_ys = token
        .token_secrets()
        .into_iter()
        .map(|secret| dhke::hash_to_curve(secret.as_bytes()))
        .collect::<Result<Vec<_>, _>>()?;
    let has_spend_conditions = !token.spending_conditions()?.is_empty();
    let mut p2pk_pubkeys = token
        .p2pk_pubkeys()?
        .into_iter()
        .map(|pubkey| pubkey.to_string())
        .collect::<Vec<_>>();
    let mut p2pk_refund_pubkeys = token
        .p2pk_refund_pubkeys()?
        .into_iter()
        .map(|pubkey| pubkey.to_string())
        .collect::<Vec<_>>();
    p2pk_pubkeys.sort();
    p2pk_refund_pubkeys.sort();

    Ok(CashuTokenInfo {
        amount_satoshis: amount.into(),
        token_hash: crypto::sha256_hex(token_str),
        mint_url,
        proof_ys,
        has_spend_conditions,
        p2pk_pubkeys,
        p2pk_refund_pubkeys,
    })
}

pub fn verify_cashu_token(
    token_str: &str,
) -> Result<u64, Box<dyn std::error::Error + Send + Sync>> {
    let info = inspect_cashu_token(token_str)?;
    Ok(info.amount_satoshis)
}
