use hmac::{Hmac, Mac};
use sha2::Sha256;

use crate::backend::{AxRuntimeError, AxRuntimeResult};

type HmacSha256 = Hmac<Sha256>;
const PREFIX: &str = "axcsrf1.";

fn session_mac(session_id: &str, secret: &str) -> AxRuntimeResult<HmacSha256> {
    if secret.len() < 32 {
        return Err(AxRuntimeError::message(
            "CSRF signing secret must contain at least 32 bytes",
        ));
    }
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
        .map_err(|_| AxRuntimeError::message("CSRF signing configuration is invalid"))?;
    // Separate CSRF proofs from session-cookie signatures, even when keys coincide.
    mac.update(b"axonyx:csrf:v1\0");
    mac.update(&(session_id.len() as u64).to_be_bytes());
    mac.update(session_id.as_bytes());
    Ok(mac)
}

pub(crate) fn issue(session_id: &str, secret: &str) -> AxRuntimeResult<String> {
    use std::fmt::Write;
    let bytes = session_mac(session_id, secret)?.finalize().into_bytes();
    let mut token = String::with_capacity(PREFIX.len() + 64);
    token.push_str(PREFIX);
    for byte in bytes {
        write!(&mut token, "{byte:02x}").expect("writing into a String cannot fail");
    }
    Ok(token)
}

pub(crate) fn verify(session_id: &str, token: &str, secret: &str) -> AxRuntimeResult<bool> {
    let mac = session_mac(session_id, secret)?;
    let Some(hex) = token.strip_prefix(PREFIX).filter(|value| value.len() == 64) else {
        return Ok(false);
    };
    let mut signature = [0u8; 32];
    for (index, pair) in hex.as_bytes().chunks_exact(2).enumerate() {
        let Some(high) = nibble(pair[0]) else {
            return Ok(false);
        };
        let Some(low) = nibble(pair[1]) else {
            return Ok(false);
        };
        signature[index] = high * 16 + low;
    }
    // HMAC verification compares the fixed-length signature in constant time.
    Ok(mac.verify_slice(&signature).is_ok())
}

fn nibble(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        _ => None,
    }
}
