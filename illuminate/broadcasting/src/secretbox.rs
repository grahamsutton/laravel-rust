//! NaCl's `crypto_secretbox` (XSalsa20 + Poly1305): the authenticated
//! encryption Pusher uses for end-to-end encrypted channels.
//!
//! The output is libsodium's "easy" format: the 16-byte Poly1305 tag
//! followed by the ciphertext.
//!
//! ```
//! use illuminate_broadcasting::secretbox;
//!
//! let key = [7u8; 32];
//! let nonce = [9u8; 24];
//!
//! let sealed = secretbox::seal(b"Hello Pusher", &nonce, &key);
//! assert_eq!(sealed.len(), 16 + 12);
//! assert_eq!(secretbox::open(&sealed, &nonce, &key).unwrap(), b"Hello Pusher");
//! ```

/// The size of a secretbox key, in bytes.
pub const KEY_BYTES: usize = 32;

/// The size of a secretbox nonce, in bytes.
pub const NONCE_BYTES: usize = 24;

/// The size of the authentication tag, in bytes.
pub const MAC_BYTES: usize = 16;

const SIGMA: [u32; 4] = [0x6170_7865, 0x3320_646e, 0x7962_2d32, 0x6b20_6574];

/// Encrypt and authenticate a message.
pub fn seal(message: &[u8], nonce: &[u8; NONCE_BYTES], key: &[u8; KEY_BYTES]) -> Vec<u8> {
    let subkey = hsalsa20(key, nonce[..16].try_into().expect("16 bytes"));
    let stream_nonce: [u8; 8] = nonce[16..].try_into().expect("8 bytes");

    let mut first_block = salsa20_block(&subkey, &stream_nonce, 0);
    let poly_key: [u8; 32] = first_block[..32].try_into().expect("32 bytes");

    let mut ciphertext = message.to_vec();
    xor_stream(&subkey, &stream_nonce, &mut first_block, &mut ciphertext);

    let tag = poly1305(&poly_key, &ciphertext);
    let mut sealed = Vec::with_capacity(MAC_BYTES + ciphertext.len());
    sealed.extend_from_slice(&tag);
    sealed.extend_from_slice(&ciphertext);
    sealed
}

/// Verify and decrypt a sealed message, or `None` when it was forged or
/// encrypted with another key.
pub fn open(sealed: &[u8], nonce: &[u8; NONCE_BYTES], key: &[u8; KEY_BYTES]) -> Option<Vec<u8>> {
    if sealed.len() < MAC_BYTES {
        return None;
    }
    let (tag, ciphertext) = sealed.split_at(MAC_BYTES);

    let subkey = hsalsa20(key, nonce[..16].try_into().expect("16 bytes"));
    let stream_nonce: [u8; 8] = nonce[16..].try_into().expect("8 bytes");
    let mut first_block = salsa20_block(&subkey, &stream_nonce, 0);
    let poly_key: [u8; 32] = first_block[..32].try_into().expect("32 bytes");

    let expected = poly1305(&poly_key, ciphertext);
    let difference = expected
        .iter()
        .zip(tag)
        .fold(0u8, |difference, (a, b)| difference | (a ^ b));
    if difference != 0 {
        return None;
    }

    let mut message = ciphertext.to_vec();
    xor_stream(&subkey, &stream_nonce, &mut first_block, &mut message);
    Some(message)
}

/// XOR the Salsa20 keystream into `data`, skipping the first 32 bytes of the
/// first block (they keyed Poly1305).
fn xor_stream(key: &[u8; 32], nonce: &[u8; 8], first_block: &mut [u8; 64], data: &mut [u8]) {
    let (head, rest) = data.split_at_mut(data.len().min(32));
    for (byte, stream) in head.iter_mut().zip(&first_block[32..]) {
        *byte ^= stream;
    }
    for (index, chunk) in rest.chunks_mut(64).enumerate() {
        let block = salsa20_block(key, nonce, index as u64 + 1);
        for (byte, stream) in chunk.iter_mut().zip(block.iter()) {
            *byte ^= stream;
        }
    }
    first_block.fill(0);
}

fn load32(bytes: &[u8]) -> u32 {
    u32::from_le_bytes(bytes[..4].try_into().expect("4 bytes"))
}

fn double_rounds(x: &mut [u32; 16]) {
    macro_rules! quarter {
        ($a:expr, $b:expr, $c:expr, $d:expr) => {
            x[$b] ^= x[$a].wrapping_add(x[$d]).rotate_left(7);
            x[$c] ^= x[$b].wrapping_add(x[$a]).rotate_left(9);
            x[$d] ^= x[$c].wrapping_add(x[$b]).rotate_left(13);
            x[$a] ^= x[$d].wrapping_add(x[$c]).rotate_left(18);
        };
    }
    for _ in 0..10 {
        // Column round.
        quarter!(0, 4, 8, 12);
        quarter!(5, 9, 13, 1);
        quarter!(10, 14, 2, 6);
        quarter!(15, 3, 7, 11);
        // Row round.
        quarter!(0, 1, 2, 3);
        quarter!(5, 6, 7, 4);
        quarter!(10, 11, 8, 9);
        quarter!(15, 12, 13, 14);
    }
}

fn initial_state(key: &[u8; 32], input: &[u8; 16]) -> [u32; 16] {
    [
        SIGMA[0],
        load32(&key[0..]),
        load32(&key[4..]),
        load32(&key[8..]),
        load32(&key[12..]),
        SIGMA[1],
        load32(&input[0..]),
        load32(&input[4..]),
        load32(&input[8..]),
        load32(&input[12..]),
        SIGMA[2],
        load32(&key[16..]),
        load32(&key[20..]),
        load32(&key[24..]),
        load32(&key[28..]),
        SIGMA[3],
    ]
}

/// One 64-byte block of the Salsa20 keystream.
fn salsa20_block(key: &[u8; 32], nonce: &[u8; 8], counter: u64) -> [u8; 64] {
    let mut input = [0u8; 16];
    input[..8].copy_from_slice(nonce);
    input[8..].copy_from_slice(&counter.to_le_bytes());

    let state = initial_state(key, &input);
    let mut working = state;
    double_rounds(&mut working);

    let mut block = [0u8; 64];
    for (index, (word, original)) in working.iter().zip(state.iter()).enumerate() {
        block[index * 4..index * 4 + 4]
            .copy_from_slice(&word.wrapping_add(*original).to_le_bytes());
    }
    block
}

/// HSalsa20: derives XSalsa20's subkey from the key and the first 16 bytes
/// of the nonce.
fn hsalsa20(key: &[u8; 32], input: &[u8; 16]) -> [u8; 32] {
    let mut working = initial_state(key, input);
    double_rounds(&mut working);

    let mut subkey = [0u8; 32];
    for (index, word) in [0, 5, 10, 15, 6, 7, 8, 9].iter().enumerate() {
        subkey[index * 4..index * 4 + 4].copy_from_slice(&working[*word].to_le_bytes());
    }
    subkey
}

/// The Poly1305 one-time authenticator (26-bit limbs).
pub(crate) fn poly1305(key: &[u8; 32], message: &[u8]) -> [u8; 16] {
    const MASK: u32 = 0x03ff_ffff;

    let r0 = load32(&key[0..]) & 0x03ff_ffff;
    let r1 = (load32(&key[3..]) >> 2) & 0x03ff_ff03;
    let r2 = (load32(&key[6..]) >> 4) & 0x03ff_c0ff;
    let r3 = (load32(&key[9..]) >> 6) & 0x03f0_3fff;
    let r4 = (load32(&key[12..]) >> 8) & 0x000f_ffff;

    let (s1, s2, s3, s4) = (r1 * 5, r2 * 5, r3 * 5, r4 * 5);
    let (mut h0, mut h1, mut h2, mut h3, mut h4) = (0u32, 0u32, 0u32, 0u32, 0u32);

    for chunk in message.chunks(16) {
        let mut block = [0u8; 16];
        block[..chunk.len()].copy_from_slice(chunk);
        let hibit = if chunk.len() == 16 {
            1 << 24
        } else {
            block[chunk.len()] = 1;
            0
        };

        h0 = h0.wrapping_add(load32(&block[0..]) & MASK);
        h1 = h1.wrapping_add((load32(&block[3..]) >> 2) & MASK);
        h2 = h2.wrapping_add((load32(&block[6..]) >> 4) & MASK);
        h3 = h3.wrapping_add((load32(&block[9..]) >> 6) & MASK);
        h4 = h4.wrapping_add((load32(&block[12..]) >> 8) | hibit);

        let mul = |a: u32, b: u32| u64::from(a) * u64::from(b);
        let d0 = mul(h0, r0) + mul(h1, s4) + mul(h2, s3) + mul(h3, s2) + mul(h4, s1);
        let mut d1 = mul(h0, r1) + mul(h1, r0) + mul(h2, s4) + mul(h3, s3) + mul(h4, s2);
        let mut d2 = mul(h0, r2) + mul(h1, r1) + mul(h2, r0) + mul(h3, s4) + mul(h4, s3);
        let mut d3 = mul(h0, r3) + mul(h1, r2) + mul(h2, r1) + mul(h3, r0) + mul(h4, s4);
        let mut d4 = mul(h0, r4) + mul(h1, r3) + mul(h2, r2) + mul(h3, r1) + mul(h4, r0);

        let mut carry = d0 >> 26;
        h0 = (d0 as u32) & MASK;
        d1 += carry;
        carry = d1 >> 26;
        h1 = (d1 as u32) & MASK;
        d2 += carry;
        carry = d2 >> 26;
        h2 = (d2 as u32) & MASK;
        d3 += carry;
        carry = d3 >> 26;
        h3 = (d3 as u32) & MASK;
        d4 += carry;
        carry = d4 >> 26;
        h4 = (d4 as u32) & MASK;
        h0 = h0.wrapping_add((carry as u32).wrapping_mul(5));
        let carry = h0 >> 26;
        h0 &= MASK;
        h1 = h1.wrapping_add(carry);
    }

    // Fully carry h.
    let mut carry = h1 >> 26;
    h1 &= MASK;
    h2 = h2.wrapping_add(carry);
    carry = h2 >> 26;
    h2 &= MASK;
    h3 = h3.wrapping_add(carry);
    carry = h3 >> 26;
    h3 &= MASK;
    h4 = h4.wrapping_add(carry);
    carry = h4 >> 26;
    h4 &= MASK;
    h0 = h0.wrapping_add(carry.wrapping_mul(5));
    carry = h0 >> 26;
    h0 &= MASK;
    h1 = h1.wrapping_add(carry);

    // Compute h + -p.
    let mut g0 = h0.wrapping_add(5);
    carry = g0 >> 26;
    g0 &= MASK;
    let mut g1 = h1.wrapping_add(carry);
    carry = g1 >> 26;
    g1 &= MASK;
    let mut g2 = h2.wrapping_add(carry);
    carry = g2 >> 26;
    g2 &= MASK;
    let mut g3 = h3.wrapping_add(carry);
    carry = g3 >> 26;
    g3 &= MASK;
    let mut g4 = h4.wrapping_add(carry).wrapping_sub(1 << 26);

    // Select h if h < p, or h + -p if h >= p.
    let mut select = (g4 >> 31).wrapping_sub(1);
    g0 &= select;
    g1 &= select;
    g2 &= select;
    g3 &= select;
    g4 &= select;
    select = !select;
    h0 = (h0 & select) | g0;
    h1 = (h1 & select) | g1;
    h2 = (h2 & select) | g2;
    h3 = (h3 & select) | g3;
    h4 = (h4 & select) | g4;

    // h = h % 2^128.
    let h0 = h0 | (h1 << 26);
    let h1 = (h1 >> 6) | (h2 << 20);
    let h2 = (h2 >> 12) | (h3 << 14);
    let h3 = (h3 >> 18) | (h4 << 8);

    // mac = (h + s) % 2^128.
    let mut f = u64::from(h0) + u64::from(load32(&key[16..]));
    let t0 = f as u32;
    f = u64::from(h1) + u64::from(load32(&key[20..])) + (f >> 32);
    let t1 = f as u32;
    f = u64::from(h2) + u64::from(load32(&key[24..])) + (f >> 32);
    let t2 = f as u32;
    f = u64::from(h3) + u64::from(load32(&key[28..])) + (f >> 32);
    let t3 = f as u32;

    let mut tag = [0u8; 16];
    tag[0..4].copy_from_slice(&t0.to_le_bytes());
    tag[4..8].copy_from_slice(&t1.to_le_bytes());
    tag[8..12].copy_from_slice(&t2.to_le_bytes());
    tag[12..16].copy_from_slice(&t3.to_le_bytes());
    tag
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> [u8; 32] {
        std::array::from_fn(|index| index as u8)
    }

    fn nonce() -> [u8; 24] {
        std::array::from_fn(|index| 100 + index as u8)
    }

    #[test]
    fn poly1305_matches_rfc_8439() {
        let key: [u8; 32] =
            hex::decode("85d6be7857556d337f4452fe42d506a80103808afb0db2fd4abff6af4149f51b")
                .unwrap()
                .try_into()
                .unwrap();
        let tag = poly1305(&key, b"Cryptographic Forum Research Group");
        assert_eq!(hex::encode(tag), "a8061dc1305136c6c22b8baf0c0127a9");
    }

    // The expected values were produced by libsodium (PHP's
    // `sodium_crypto_secretbox`) for the same key, nonce and message.
    #[test]
    fn seal_matches_libsodium() {
        assert_eq!(
            hex::encode(seal(br#"{"message":"hello"}"#, &nonce(), &key())),
            "ce59ae2f72499c728cc2d6d8bdd0b170799bf4ac49c5af8ed5df19b65fe1cf74518845"
        );
        assert_eq!(
            hex::encode(seal(b"", &nonce(), &key())),
            "f49572d6194281e3c87fbb4e2106932c"
        );
    }

    #[test]
    fn seal_spans_many_blocks() {
        let message = "Laravel broadcasting! ".repeat(20);
        let sealed = seal(message.as_bytes(), &nonce(), &key());
        assert_eq!(
            hex::encode(&sealed),
            "6b8513fbda03b810dc2ae9aca84b1d3d4ed8eba84cd3a2c9d28f4cf553e7c26b4ac356c0c586077ad46d34f166ababfac70e907b2e44ef8d9b7136424c72af04b7f5c23870f4563e67823277afdbf00362ab000cba5d3eefd5af7d802fa289f035e4f96ec9985a22defafb460e8389e698bb861a8bba5bb4771211ec9d7cd9590a4ee02fbb0b227b2fed6ac08562a255d0672c387baf6f683b464718bfbb8f340748976c594e1c7b8a29eb131459bf95ed5f495ab2b0b704f393974f37dfc3c7b0c3847c8f844b3a270943e2203a7f33c404ec2aee8e60b5161b8240221c389f16501015b12e8179b73fb4092dea86f164d5f09fd5bf827cff7f9f60229402e72b779861749262ff3199b2e39831dbc11cd3267c139a29675b9e94bfa21250671a183d7400b1c3d13ddd9276d81933ab2b5062cb905aa5cb9982fbdd04c656d2146c4dfc98e8a296a37c5395e147527afad9034cd5b4932211da7ed5eb5c9146f3b3f666ed00475d44845a9239819c7ca05727418e80753d6947e9a804531b6f182c49f00b934200bf32a57f76bdb0c653d70398b190c3cef6aec616835a8efd58841fe8f458c0a36063978c5d512cf6cd10033c440ac6ef98da3ea22b666d2ceecec48fe71c853b"
        );
        assert_eq!(open(&sealed, &nonce(), &key()).unwrap(), message.as_bytes());
    }

    #[test]
    fn forged_messages_do_not_open() {
        let mut sealed = seal(b"secret", &nonce(), &key());
        assert!(open(&sealed, &nonce(), &[1u8; 32]).is_none());
        sealed[20] ^= 1;
        assert!(open(&sealed, &nonce(), &key()).is_none());
        assert!(open(&[0u8; 4], &nonce(), &key()).is_none());
    }
}
