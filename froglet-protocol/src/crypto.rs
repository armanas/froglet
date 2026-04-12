use hmac::{Hmac, Mac};
use k256::schnorr::{
    Signature as SchnorrSignature, SigningKey, VerifyingKey,
    signature::{Signer, Verifier},
};
use k256::{PublicKey, SecretKey};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

pub use k256::schnorr::SigningKey as NodeSigningKey;

pub fn verify_signature(pubkey_hex: &str, sig_hex: &str, content: &str) -> bool {
    verify_message(pubkey_hex, sig_hex, content.as_bytes())
}

pub fn verify_message(pubkey_hex: &str, sig_hex: &str, message: &[u8]) -> bool {
    let pubkey_bytes: Vec<u8> = match hex::decode(pubkey_hex) {
        Ok(b) if b.len() == 32 => b,
        _ => return false,
    };

    let sig_bytes: Vec<u8> = match hex::decode(sig_hex) {
        Ok(b) if b.len() == 64 => b,
        _ => return false,
    };

    let verifying_key = match VerifyingKey::from_bytes(pubkey_bytes.as_slice()) {
        Ok(key) => key,
        Err(_) => return false,
    };

    let signature = match SchnorrSignature::try_from(sig_bytes.as_slice()) {
        Ok(signature) => signature,
        Err(_) => return false,
    };

    verifying_key.verify(message, &signature).is_ok()
}

pub fn generate_signing_key() -> NodeSigningKey {
    SigningKey::random(&mut rand::rngs::OsRng)
}

pub fn signing_key_from_seed_bytes(seed: &[u8; 32]) -> Result<NodeSigningKey, String> {
    let secret_key =
        SecretKey::from_slice(seed).map_err(|e| format!("invalid secp256k1 seed: {e}"))?;
    Ok(SigningKey::from(secret_key))
}

pub fn signing_key_seed_bytes(signing_key: &NodeSigningKey) -> [u8; 32] {
    signing_key.to_bytes().into()
}

pub fn public_key_hex(signing_key: &NodeSigningKey) -> String {
    hex::encode(signing_key.verifying_key().to_bytes())
}

pub fn compressed_public_key_hex(signing_key: &NodeSigningKey) -> String {
    let verifying_key = signing_key.verifying_key();
    let public_key: PublicKey = (*verifying_key).into();
    hex::encode(public_key.to_sec1_bytes())
}

pub fn sign_message_hex(signing_key: &NodeSigningKey, message: &[u8]) -> String {
    let signature: SchnorrSignature = signing_key.sign(message);
    hex::encode(signature.to_bytes())
}

pub fn sha256_hex(input: impl AsRef<[u8]>) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_ref());
    hex::encode(hasher.finalize())
}

/// HMAC-SHA256 (RFC 2104). Returns the hex-encoded MAC.
pub fn hmac_sha256_hex(key: &[u8], message: &[u8]) -> String {
    let mut mac =
        HmacSha256::new_from_slice(key).expect("HMAC-SHA256 accepts any key length");
    mac.update(message);
    hex::encode(mac.finalize().into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_and_verify_roundtrip_succeeds() {
        let signing_key = generate_signing_key();
        let message = b"froglet-test-message";
        let pubkey_hex = public_key_hex(&signing_key);
        let sig_hex = sign_message_hex(&signing_key, message);

        assert!(verify_message(&pubkey_hex, &sig_hex, message));
        assert!(verify_signature(
            &pubkey_hex,
            &sig_hex,
            "froglet-test-message"
        ));
    }

    #[test]
    fn verify_fails_for_wrong_length_pubkey_or_sig() {
        let signing_key = generate_signing_key();
        let message = b"froglet-test-message";

        let pubkey_hex = public_key_hex(&signing_key);
        let sig_hex = sign_message_hex(&signing_key, message);

        let short_pubkey = &pubkey_hex[..60];
        let short_sig = &sig_hex[..120];

        assert!(!verify_message(short_pubkey, &sig_hex, message));
        assert!(!verify_message(&pubkey_hex, short_sig, message));
    }

    #[test]
    fn verify_fails_for_malformed_hex() {
        let malformed_pubkey = "zzzz";
        let malformed_sig = "yy";
        let message = b"froglet-test-message";

        assert!(!verify_message(malformed_pubkey, "00", message));
        assert!(!verify_message("00", malformed_sig, message));
    }

    #[test]
    fn verify_fails_for_tampered_message_or_sig() {
        let signing_key = generate_signing_key();
        let message = b"froglet-test-message";
        let other_message = b"froglet-other-message";

        let pubkey_hex = public_key_hex(&signing_key);
        let sig_hex = sign_message_hex(&signing_key, message);

        assert!(!verify_message(&pubkey_hex, &sig_hex, other_message));

        let mut tampered_sig = sig_hex.clone();
        if let Some(c) = tampered_sig.get_mut(0..1) {
            if c == "0" {
                tampered_sig.replace_range(0..1, "1");
            } else {
                tampered_sig.replace_range(0..1, "0");
            }
        }

        assert!(!verify_message(&pubkey_hex, &tampered_sig, message));
    }

    #[test]
    fn signing_key_seed_roundtrip_succeeds() {
        let signing_key = generate_signing_key();
        let seed = signing_key_seed_bytes(&signing_key);
        let restored = signing_key_from_seed_bytes(&seed).expect("seed should restore key");

        assert_eq!(public_key_hex(&signing_key), public_key_hex(&restored));
    }

    #[test]
    fn hmac_sha256_hex_matches_rfc4231_test_case_2() {
        // RFC 4231 Test Case 2: key = "Jefe", data = "what do ya want for nothing?"
        let mac = hmac_sha256_hex(b"Jefe", b"what do ya want for nothing?");
        assert_eq!(
            mac,
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    #[test]
    fn sha256_hex_matches_known_value() {
        let digest = sha256_hex(b"froglet");
        assert_eq!(
            digest,
            "37b1d40aa65361c1f8bc309fed70096d03923cad89937a271ea54362c2be829e"
        );
    }
}
