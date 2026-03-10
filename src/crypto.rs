use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use sha2::{Digest, Sha256};

pub fn verify_signature(pubkey_hex: &str, sig_hex: &str, content: &str) -> bool {
    verify_message(pubkey_hex, sig_hex, content.as_bytes())
}

pub fn verify_message(pubkey_hex: &str, sig_hex: &str, message: &[u8]) -> bool {
    let pubkey_bytes: Vec<u8> = match hex::decode(pubkey_hex) {
        Ok(b) if b.len() == 32 => b,
        _ => return false,
    };

    let pubkey_array: [u8; 32] = match pubkey_bytes.as_slice().try_into() {
        Ok(arr) => arr,
        Err(_) => return false,
    };

    let sig_bytes: Vec<u8> = match hex::decode(sig_hex) {
        Ok(b) if b.len() == 64 => b,
        _ => return false,
    };

    let vk = match VerifyingKey::from_bytes(&pubkey_array) {
        Ok(key) => key,
        Err(_) => return false,
    };

    let sig = match Signature::from_slice(sig_bytes.as_slice()) {
        Ok(s) => s,
        Err(_) => return false,
    };

    vk.verify(message, &sig).is_ok()
}

pub fn public_key_hex(signing_key: &SigningKey) -> String {
    hex::encode(signing_key.verifying_key().to_bytes())
}

pub fn sign_message_hex(signing_key: &SigningKey, message: &[u8]) -> String {
    let sig = signing_key.sign(message);
    hex::encode(sig.to_bytes())
}

pub fn sha256_hex(input: impl AsRef<[u8]>) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_ref());
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;

    #[test]
    fn sign_and_verify_roundtrip_succeeds() {
        let mut rng = OsRng;
        let signing_key: SigningKey = SigningKey::generate(&mut rng);

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
        let mut rng = OsRng;
        let signing_key: SigningKey = SigningKey::generate(&mut rng);
        let message = b"froglet-test-message";

        let pubkey_hex = public_key_hex(&signing_key);
        let sig_hex = sign_message_hex(&signing_key, message);

        // Truncate hex so that decoded length is wrong.
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
        let mut rng = OsRng;
        let signing_key: SigningKey = SigningKey::generate(&mut rng);
        let message = b"froglet-test-message";
        let other_message = b"froglet-other-message";

        let pubkey_hex = public_key_hex(&signing_key);
        let sig_hex = sign_message_hex(&signing_key, message);

        // Different message should not verify with same signature.
        assert!(!verify_message(&pubkey_hex, &sig_hex, other_message));

        // Tamper with signature hex.
        let mut tampered_sig = sig_hex.clone();
        // Flip a nibble if long enough.
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
    fn sha256_hex_matches_known_value() {
        let digest = sha256_hex(b"froglet");
        assert_eq!(
            digest,
            "37b1d40aa65361c1f8bc309fed70096d03923cad89937a271ea54362c2be829e"
        );
    }
}
