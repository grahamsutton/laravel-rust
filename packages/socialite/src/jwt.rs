//! Just enough JWT to verify OpenID Connect ID tokens signed with RS256
//! (RSASSA-PKCS1-v1_5 with SHA-256) against a provider's JSON Web Keys —
//! what Socialite does with `firebase/php-jwt` for Facebook Limited Login.

use base64::Engine;
use base64::engine::general_purpose::{URL_SAFE, URL_SAFE_NO_PAD};
use illuminate_support::error::RuntimeException;
use illuminate_support::{Carbon, Result, Value, ValueExt};
use sha2::{Digest, Sha256};

/// A decoded (but not yet verified) JSON Web Token.
pub(crate) struct Jwt {
    /// The decoded header (`alg`, `kid`, ...).
    pub header: Value,
    payload: Value,
    signed: String,
    signature: Vec<u8>,
}

impl Jwt {
    /// Split and decode a token.
    pub fn parse(token: &str) -> Result<Self> {
        let segments: Vec<&str> = token.split('.').collect();
        if segments.len() != 3 {
            return Err(RuntimeException::new("Wrong number of segments").into());
        }
        let header = decode_json(segments[0])?;
        let payload = decode_json(segments[1])?;
        let signature = decode_base64(segments[2])
            .ok_or_else(|| RuntimeException::new("Signature encoding is invalid"))?;
        Ok(Self {
            header,
            payload,
            signed: format!("{}.{}", segments[0], segments[1]),
            signature,
        })
    }

    /// Verify the token's signature with the given JSON Web Key (`n` and
    /// `e`) and its timestamps, returning the payload.
    pub fn verify(self, key: &Value) -> Result<Value> {
        if self.header["alg"].as_str() != Some("RS256") {
            return Err(RuntimeException::new("Algorithm not supported").into());
        }
        let modulus = key["n"].as_str().and_then(decode_base64);
        let exponent = key["e"].as_str().and_then(decode_base64);
        let (Some(modulus), Some(exponent)) = (modulus, exponent) else {
            return Err(RuntimeException::new("The public key is invalid").into());
        };
        if !verify_rs256(&modulus, &exponent, self.signed.as_bytes(), &self.signature) {
            return Err(RuntimeException::new("Signature verification failed").into());
        }

        let now = Carbon::now().timestamp();
        if let Some(not_before) = self.payload.get("nbf").and_then(ValueExt::to_i64_lossy)
            && not_before > now
        {
            return Err(RuntimeException::new(format!(
                "Cannot handle token with nbf prior to {}",
                format_timestamp(not_before)
            ))
            .into());
        }
        if let Some(issued_at) = self.payload.get("iat").and_then(ValueExt::to_i64_lossy)
            && issued_at > now
        {
            return Err(RuntimeException::new(format!(
                "Cannot handle token with iat prior to {}",
                format_timestamp(issued_at)
            ))
            .into());
        }
        if let Some(expires) = self.payload.get("exp").and_then(ValueExt::to_i64_lossy)
            && now >= expires
        {
            return Err(RuntimeException::new("Expired token").into());
        }

        Ok(self.payload)
    }
}

fn format_timestamp(timestamp: i64) -> String {
    Carbon::from_timestamp(timestamp).to_iso8601_string()
}

/// Decode URL-safe base64, with or without padding.
fn decode_base64(segment: &str) -> Option<Vec<u8>> {
    URL_SAFE_NO_PAD
        .decode(segment.trim_end_matches('='))
        .or_else(|_| URL_SAFE.decode(segment))
        .ok()
}

fn decode_json(segment: &str) -> Result<Value> {
    decode_base64(segment)
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .filter(Value::is_object)
        .ok_or_else(|| RuntimeException::new("Syntax error, malformed JSON").into())
}

/// The DER prefix of a SHA-256 `DigestInfo` (RFC 8017, section 9.2).
const SHA256_DIGEST_INFO: [u8; 19] = [
    0x30, 0x31, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01, 0x05,
    0x00, 0x04, 0x20,
];

/// Verify an RSASSA-PKCS1-v1_5 SHA-256 signature with the public key
/// `(modulus, exponent)`, all big-endian.
pub(crate) fn verify_rs256(
    modulus: &[u8],
    exponent: &[u8],
    message: &[u8],
    signature: &[u8],
) -> bool {
    let modulus = strip_leading_zeros(modulus);
    let length = modulus.len();
    if signature.len() != length || length < SHA256_DIGEST_INFO.len() + 32 + 11 {
        return false;
    }

    let n = BigUint::from_be_bytes(modulus);
    let s = BigUint::from_be_bytes(signature);
    if !n.is_odd() || s.compare(&n) != std::cmp::Ordering::Less {
        return false;
    }

    let decrypted = s.mod_pow(exponent, &n).to_be_bytes(length);

    let mut expected = Vec::with_capacity(length);
    expected.extend_from_slice(&[0x00, 0x01]);
    expected.resize(length - SHA256_DIGEST_INFO.len() - 32 - 1, 0xff);
    expected.push(0x00);
    expected.extend_from_slice(&SHA256_DIGEST_INFO);
    expected.extend_from_slice(&Sha256::digest(message));

    // Compare in constant time; signatures are public, but it's cheap.
    decrypted.len() == expected.len()
        && decrypted
            .iter()
            .zip(&expected)
            .fold(0u8, |difference, (a, b)| difference | (a ^ b))
            == 0
}

fn strip_leading_zeros(bytes: &[u8]) -> &[u8] {
    let start = bytes
        .iter()
        .position(|byte| *byte != 0)
        .unwrap_or(bytes.len());
    &bytes[start..]
}

/// A minimal unsigned big integer (little-endian 32-bit limbs) with
/// Montgomery modular exponentiation — all RSA verification needs.
#[derive(Clone, Debug, PartialEq, Eq)]
struct BigUint {
    limbs: Vec<u32>,
}

impl BigUint {
    fn from_be_bytes(bytes: &[u8]) -> Self {
        let mut limbs: Vec<u32> = bytes
            .rchunks(4)
            .map(|chunk| {
                chunk
                    .iter()
                    .fold(0u32, |limb, byte| (limb << 8) | *byte as u32)
            })
            .collect();
        while limbs.len() > 1 && limbs.last() == Some(&0) {
            limbs.pop();
        }
        if limbs.is_empty() {
            limbs.push(0);
        }
        Self { limbs }
    }

    /// The value as exactly `length` big-endian bytes.
    fn to_be_bytes(&self, length: usize) -> Vec<u8> {
        let mut bytes: Vec<u8> = self
            .limbs
            .iter()
            .rev()
            .flat_map(|limb| limb.to_be_bytes())
            .collect();
        let significant = strip_leading_zeros(&bytes).len();
        bytes.drain(..bytes.len() - significant);
        let mut padded = vec![0u8; length.saturating_sub(bytes.len())];
        padded.extend_from_slice(&bytes);
        padded
    }

    fn is_odd(&self) -> bool {
        self.limbs[0] & 1 == 1
    }

    fn compare(&self, other: &Self) -> std::cmp::Ordering {
        compare_limbs(&self.limbs, &other.limbs)
    }

    /// `self ^ exponent mod modulus` (the modulus must be odd).
    fn mod_pow(&self, exponent: &[u8], modulus: &Self) -> Self {
        let n = &modulus.limbs;
        let size = n.len();
        let n_prime = montgomery_inverse(n[0]);

        // R² mod n, where R = 2^(32 · size), by repeated doubling.
        let mut r_squared = vec![0u32; size];
        r_squared[0] = 1;
        for _ in 0..(64 * size) {
            double_mod(&mut r_squared, n);
        }

        let mut base = self.limbs.clone();
        base.resize(size, 0);
        let base = montgomery_multiply(&base, &r_squared, n, n_prime);

        let mut one = vec![0u32; size];
        one[0] = 1;
        let mut accumulator = montgomery_multiply(&one, &r_squared, n, n_prime);

        for byte in exponent {
            for bit in (0..8).rev() {
                accumulator = montgomery_multiply(&accumulator, &accumulator, n, n_prime);
                if (byte >> bit) & 1 == 1 {
                    accumulator = montgomery_multiply(&accumulator, &base, n, n_prime);
                }
            }
        }

        let mut limbs = montgomery_multiply(&accumulator, &one, n, n_prime);
        while limbs.len() > 1 && limbs.last() == Some(&0) {
            limbs.pop();
        }
        Self { limbs }
    }
}

/// Compare two little-endian numbers (of any lengths).
fn compare_limbs(a: &[u32], b: &[u32]) -> std::cmp::Ordering {
    let length = a.len().max(b.len());
    for index in (0..length).rev() {
        let left = a.get(index).copied().unwrap_or(0);
        let right = b.get(index).copied().unwrap_or(0);
        if left != right {
            return left.cmp(&right);
        }
    }
    std::cmp::Ordering::Equal
}

/// `a -= b` (requires `a >= b`).
fn subtract_in_place(a: &mut [u32], b: &[u32]) {
    let mut borrow = 0i64;
    for (index, limb) in a.iter_mut().enumerate() {
        let difference = *limb as i64 - b.get(index).copied().unwrap_or(0) as i64 - borrow;
        if difference < 0 {
            *limb = (difference + (1i64 << 32)) as u32;
            borrow = 1;
        } else {
            *limb = difference as u32;
            borrow = 0;
        }
    }
}

/// `value = 2 · value mod n` (for `value < n`).
fn double_mod(value: &mut Vec<u32>, n: &[u32]) {
    let mut carry = 0u32;
    for limb in value.iter_mut() {
        let next = *limb >> 31;
        *limb = (*limb << 1) | carry;
        carry = next;
    }
    if carry == 1 {
        value.push(1);
    }
    if compare_limbs(value, n) != std::cmp::Ordering::Less {
        subtract_in_place(value, n);
    }
    value.truncate(n.len());
}

/// `-n⁻¹ mod 2³²`, by Newton's iteration (`n` odd).
fn montgomery_inverse(n0: u32) -> u32 {
    let mut inverse: u32 = 1;
    for _ in 0..5 {
        inverse = inverse.wrapping_mul(2u32.wrapping_sub(n0.wrapping_mul(inverse)));
    }
    inverse.wrapping_neg()
}

/// Montgomery multiplication (CIOS): `a · b · R⁻¹ mod n`.
fn montgomery_multiply(a: &[u32], b: &[u32], n: &[u32], n_prime: u32) -> Vec<u32> {
    let size = n.len();
    let mut t = vec![0u32; size + 2];

    for &b_i in &b[..size] {
        let mut carry = 0u64;
        for j in 0..size {
            let sum = t[j] as u64 + a[j] as u64 * b_i as u64 + carry;
            t[j] = sum as u32;
            carry = sum >> 32;
        }
        let sum = t[size] as u64 + carry;
        t[size] = sum as u32;
        t[size + 1] = (sum >> 32) as u32;

        let m = t[0].wrapping_mul(n_prime);
        let sum = t[0] as u64 + m as u64 * n[0] as u64;
        let mut carry = sum >> 32;
        for j in 1..size {
            let sum = t[j] as u64 + m as u64 * n[j] as u64 + carry;
            t[j - 1] = sum as u32;
            carry = sum >> 32;
        }
        let sum = t[size] as u64 + carry;
        t[size - 1] = sum as u32;
        t[size] = t[size + 1] + (sum >> 32) as u32;
        t[size + 1] = 0;
    }

    t.truncate(size + 1);
    if compare_limbs(&t, n) != std::cmp::Ordering::Less {
        subtract_in_place(&mut t, n);
    }
    t.truncate(size);
    t
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use illuminate_support::json;

    /// A 2048-bit test key (generated with Python's `cryptography`) and a
    /// token it signed.
    pub(crate) const MODULUS: &str = "xPFKiKL9rG4woPQunr8yXFC1-BPHLaH-p04pAwXJ1t7rx0ntr_fdIXBOQ2oEQrSagnm5CPALMtAe0bXe54pKPGy2QfWj7qbuFi5PQ7wibFcyvVdMGVGyfU0af7ZetDKsx-YkDcdbr7VjFS6r0BRkCB06VOIxCb0VwZCrrqtQdFNoxTkQpnU2V99mCbYelwm60KnKN_buHCEPQT_uVoD47cviQrxjqNxOq9nqFtvS3YoXH6MGqW-0NSjc-NJkeMnkfK4T4nv3ylNzFr-gOeXtJFGZwudeNz41TJjRdaslDrMV6X5ibOmf10t5mqK1TjbilL7w6h0mjdHzSRjx3-l5cw";
    pub(crate) const EXPONENT: &str = "AQAB";
    pub(crate) const MESSAGE: &str = "Laravel Socialite";
    pub(crate) const SIGNATURE: &str = "ALbNV4Rt13Ega6BPMaN0CtROGG-vtWPCEwBEbtmUqMpD6eQFQ5D5qNq4sdeVQgsA5YgFCSxNvbdifeMr3nbKk-AgtdGaOuITCkRbTBss-wkrj2SSRNUXw0qysnvo0Dz5FPmjkWKMkPLJU8pBTYnu3rIpZ-LdpO8tTxL7d4F8vLsx5YSdutjg4f6NJEb4jkY10NfCuIJaN5QK-OrM0xb8B1aIZd-9RUWXrucReirR3zpvfqCsBE6zX1IskqyS1oLoMluAWaYa8DliN7eAkYMJfw8XQLSWyBScRVMvZ5H8_NL4loXDOL4tQ2xyacjimj0qjGdHXPOv9OKpOR_FY3TREQ";

    /// Limited Login tokens signed by the test key (`kid` = `test-kid`, for
    /// `client-id`, nonce `nonce-123`): valid until 2100, expired, and for
    /// another application.
    pub(crate) const TOKEN: &str = "eyJhbGciOiJSUzI1NiIsImtpZCI6InRlc3Qta2lkIiwidHlwIjoiSldUIn0.eyJpc3MiOiJodHRwczovL3d3dy5mYWNlYm9vay5jb20iLCJhdWQiOiJjbGllbnQtaWQiLCJzdWIiOiIxMDIyOSIsIm5hbWUiOiJUYXlsb3IgT3R3ZWxsIiwiZ2l2ZW5fbmFtZSI6IlRheWxvciIsImZhbWlseV9uYW1lIjoiT3R3ZWxsIiwiZW1haWwiOiJ0YXlsb3JAbGFyYXZlbC5jb20iLCJwaWN0dXJlIjoiaHR0cHM6Ly9wbGF0Zm9ybS1sb29rYXNpZGUuZmJzYnguY29tL3RheWxvci5qcGciLCJub25jZSI6Im5vbmNlLTEyMyIsImlhdCI6MTcwMDAwMDAwMCwiZXhwIjo0MTAyNDQ0ODAwfQ.Cg48rQX1sdMMv5ez8dqyVHFQnEJe-Nx7JYAos-X0H3KKV7P2U9UqMEPbUOPHbD1QdBDIJW-erQp2wy5RXsU1Zdbru4I7JJA8MD9Kv31-dKXJ83t7R3GHazCmpau3rlTjPdX0GNATNE0m6VyQWC4QGrE0k9o6CcUwRgNdmtzyryJcKxOUf4i37kCQ1tmLEji2asO0Sg-OzIG4v2q2Yw9NGWco03eSvEON5HRoH9oqWpFE6sXCr32iUrqMOW6oPAlBO5quz6AOedAyh1JXswGfQVOAef3c5-liS3DiAb8axRVSNE23RQ7LrOukeW2KBcjaqX-uRXl7aLW1wejJF4vk6w";
    pub(crate) const EXPIRED_TOKEN: &str = "eyJhbGciOiJSUzI1NiIsImtpZCI6InRlc3Qta2lkIiwidHlwIjoiSldUIn0.eyJpc3MiOiJodHRwczovL3d3dy5mYWNlYm9vay5jb20iLCJhdWQiOiJjbGllbnQtaWQiLCJzdWIiOiIxMDIyOSIsIm5hbWUiOiJUYXlsb3IgT3R3ZWxsIiwiZ2l2ZW5fbmFtZSI6IlRheWxvciIsImZhbWlseV9uYW1lIjoiT3R3ZWxsIiwiZW1haWwiOiJ0YXlsb3JAbGFyYXZlbC5jb20iLCJwaWN0dXJlIjoiaHR0cHM6Ly9wbGF0Zm9ybS1sb29rYXNpZGUuZmJzYnguY29tL3RheWxvci5qcGciLCJub25jZSI6Im5vbmNlLTEyMyIsImlhdCI6MTcwMDAwMDAwMCwiZXhwIjoxNzAwMDAwNjAwfQ.F2WewLlQBq0XpIFcBK7aDnoI9kA4diUsqDzg0XK7WDvUs2CqEGN8NcMKWfQ4vzqYLASGDOH0y8SgjlR-eLQC2U6Pu786emyD8D53rvyzk7IdnUkqBAyQnJR9KVFEWJ3-gmyfkXs_RDxl-_knxHM3J6i0_2SZP-FJfBdPXcE-rEhnO47AQaw8veBfw0aLmjgS-nEPsVZHlOuN7vWAPODZL5-FbOoY8yXiZTNIs2RzBW_F68M607Zlm1PYbNamsqOmlqq3sf1PsrwkirEh0YjO4xorvbNX7SNWZ23RLRf3XM8yuuaIl5UUglc9wXB19mitX8b_CIH1Sn8Nz4cF2qykvw";
    pub(crate) const WRONG_AUDIENCE_TOKEN: &str = "eyJhbGciOiJSUzI1NiIsImtpZCI6InRlc3Qta2lkIiwidHlwIjoiSldUIn0.eyJpc3MiOiJodHRwczovL3d3dy5mYWNlYm9vay5jb20iLCJhdWQiOiJzb21lb25lLWVsc2UiLCJzdWIiOiIxMDIyOSIsIm5hbWUiOiJUYXlsb3IgT3R3ZWxsIiwiZ2l2ZW5fbmFtZSI6IlRheWxvciIsImZhbWlseV9uYW1lIjoiT3R3ZWxsIiwiZW1haWwiOiJ0YXlsb3JAbGFyYXZlbC5jb20iLCJwaWN0dXJlIjoiaHR0cHM6Ly9wbGF0Zm9ybS1sb29rYXNpZGUuZmJzYnguY29tL3RheWxvci5qcGciLCJub25jZSI6Im5vbmNlLTEyMyIsImlhdCI6MTcwMDAwMDAwMCwiZXhwIjo0MTAyNDQ0ODAwfQ.WB8ldmvU7P3QSfxhD3WX0IHrnwz9sZ83UBlbSBib28acsAPSWGUjeYRbyAA09kxoRMBGvlGLGSvzZgF86pfmg3qoYKY9_SzMB0yb4U_zv5Az7jvJSiNydqLaxEzfx-JEuhVs7zuyyRYKxbdm_7PFVnDetb9hrARNkhlT0oVOhJWokD_8MEXOfr-d3OQZhuEiLgnsroHagNET-XR7K7auIj9VxanNsL76QOpNqe9fGzECEWUv1Cv8UN0pw0BQveVxSPCKiWFur9XGWh2iQBULHLGqZffi8Esh3TpEsWRKwKlV8Np9IoZUQlQH5R-JQ-LFV8TXlGG0ferNyhGAe6Tj0A";

    fn bytes(segment: &str) -> Vec<u8> {
        decode_base64(segment).unwrap()
    }

    #[test]
    fn small_numbers_exponentiate() {
        let n = BigUint::from_be_bytes(&[0x0b, 0xb9]); // 3001
        let base = BigUint::from_be_bytes(&[0x07, 0xd0]); // 2000
        let result = base.mod_pow(&[0x01, 0x00, 0x01], &n);
        // 2000^65537 mod 3001
        let mut expected: u64 = 1;
        for _ in 0..65537 {
            expected = expected * 2000 % 3001;
        }
        assert_eq!(result.limbs, vec![expected as u32]);
        assert_eq!(
            BigUint::from_be_bytes(&[0, 0, 1]).to_be_bytes(3),
            vec![0, 0, 1]
        );
    }

    #[test]
    fn multi_limb_numbers_exponentiate() {
        // n = 2^64 + 13 (odd), base = 2^40 + 7, exponent 3.
        let n = BigUint::from_be_bytes(&[1, 0, 0, 0, 0, 0, 0, 0, 13]);
        let base = BigUint::from_be_bytes(&[1, 0, 0, 0, 0, 7]);
        let result = base.mod_pow(&[3], &n);
        let modulus: u128 = (1u128 << 64) + 13;
        let b: u128 = (1u128 << 40) + 7;
        let expected = (b * b % modulus) * b % modulus;
        assert_eq!(result.to_be_bytes(16), expected.to_be_bytes().to_vec(),);
    }

    #[test]
    fn rs256_signatures_are_verified() {
        let modulus = bytes(MODULUS);
        let exponent = bytes(EXPONENT);
        let signature = bytes(SIGNATURE);
        assert!(verify_rs256(
            &modulus,
            &exponent,
            MESSAGE.as_bytes(),
            &signature
        ));
        assert!(!verify_rs256(
            &modulus,
            &exponent,
            b"Laravel Socialitf",
            &signature
        ));

        let mut tampered = signature.clone();
        tampered[10] ^= 1;
        assert!(!verify_rs256(
            &modulus,
            &exponent,
            MESSAGE.as_bytes(),
            &tampered
        ));
        assert!(!verify_rs256(
            &modulus,
            &exponent,
            MESSAGE.as_bytes(),
            &signature[1..]
        ));
    }

    #[test]
    fn malformed_tokens_are_rejected() {
        assert_eq!(
            Jwt::parse("a.b").err().unwrap().to_string(),
            "Wrong number of segments"
        );
        assert_eq!(
            Jwt::parse("bm90IGpzb24.e30.c2ln")
                .err()
                .unwrap()
                .to_string(),
            "Syntax error, malformed JSON"
        );
        let jwt = Jwt::parse("eyJhbGciOiJIUzI1NiJ9.e30.c2ln").unwrap();
        assert_eq!(
            jwt.verify(&json!({"n": MODULUS, "e": EXPONENT}))
                .unwrap_err()
                .to_string(),
            "Algorithm not supported"
        );
    }
}
