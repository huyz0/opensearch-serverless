//! Connector credentials, encrypted at rest as ml-commons stores them: AWS
//! Encryption SDK messages under a 32-byte master key (`lucene-ml`'s `esdk`).

use lucene_ml::remote::esdk;

/// A new master key: 32 random bytes, base64.
///
/// # Errors
/// The system's randomness is unavailable.
pub fn new_master_key() -> Result<String, String> {
    esdk::generate_master_key().map_err(|e| e.to_string())
}

/// `plaintext` encrypted under the base64 `master_key`, base64.
///
/// # Errors
/// The key is not base64 of a usable key.
pub fn seal(master_key: &str, plaintext: &str) -> Result<String, String> {
    esdk::encrypt_credential(master_key, plaintext).map_err(|e| e.to_string())
}

/// A value [`seal`] made, decrypted under the same key.
///
/// # Errors
/// The key is wrong or the value was altered.
pub fn open(master_key: &str, sealed: &str) -> Result<String, String> {
    esdk::decrypt_credential(master_key, sealed).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sealed_value_opens_only_under_its_key_and_holds_no_plaintext() {
        let key = new_master_key().unwrap();
        let sealed = seal(&key, "super-secret-token").unwrap();
        assert!(!sealed.contains("super-secret-token"));
        assert_eq!(open(&key, &sealed).unwrap(), "super-secret-token");
        assert!(open(&new_master_key().unwrap(), &sealed).is_err());
        assert_ne!(sealed, seal(&key, "super-secret-token").unwrap());
    }

    #[test]
    fn a_malformed_key_or_value_is_an_error_not_a_panic() {
        assert!(seal("not base64!", "x").is_err());
        let key = new_master_key().unwrap();
        assert!(open(&key, "AAAA").is_err());
        assert!(open(&key, "").is_err());
    }
}
