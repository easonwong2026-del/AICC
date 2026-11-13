//! Qoder 桌面端的本地凭据信封（CN 与 International 共用同一套 Chromium 形态）。
//!
//! 这里只放**算法**，不放任何一版的密钥、路径或钥匙串名字：
//! 那些属于各版本的适配器（`providers::qoder` / `providers::qoder_global`）。
//!
//! 信封格式（macOS，实测 CN 与 International 的 `auth.v1.dat` 都是同一个头）：
//!
//! ```text
//! "v10" || AES-128-CBC(ciphertext)
//! ```
//!
//! - IV 固定 16 个空格
//! - PKCS7 填充
//! - key = PBKDF2-HMAC-SHA1(钥匙串口令, "saltysalt", 1003 轮, 16 字节)
//!
//! 之所以把"参数"和"密钥来源"分开：两个版本的口令分别在自己的钥匙串条目里，
//! 但派生方式由 Chromium 的 Safe Storage 决定，属于同一族客户端行为。
//! 任何一版换了参数，都应该在本模块之外报 `UnsupportedFormat`，而不是在这里猜。

use aes::cipher::{BlockDecrypt, KeyInit};

pub const FILE_MAGIC: &[u8] = b"v10";
pub const PBKDF2_SALT: &[u8] = b"saltysalt";
pub const PBKDF2_ROUNDS: u32 = 1003;
pub const KEY_LEN: usize = 16;
pub const IV: [u8; 16] = [0x20; 16];
const BLOCK: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CryptoError {
    /// 文件头不是 `v10`：格式没支持过，不能硬解。
    UnsupportedFormat(String),
    DecryptFailed(String),
}

/// 判断文件是否是我们认识的信封（只读前 3 字节，不碰钥匙串）。
pub fn has_envelope(raw: &[u8]) -> bool {
    raw.starts_with(FILE_MAGIC)
}

/// 拆信封：校验 magic，解出 JSON 明文。
pub fn decrypt_envelope(raw: &[u8], password: &str) -> Result<Vec<u8>, CryptoError> {
    if !has_envelope(raw) {
        return Err(CryptoError::UnsupportedFormat(format!(
            "文件头不是 v10（实际 {:?}），格式未支持",
            String::from_utf8_lossy(&raw[..3.min(raw.len())])
        )));
    }
    decrypt_cbc(&raw[FILE_MAGIC.len()..], password)
}

/// AES-128-CBC(IV=16 空格, PKCS7) + PBKDF2-HMAC-SHA1。
pub fn decrypt_cbc(cipher_text: &[u8], password: &str) -> Result<Vec<u8>, CryptoError> {
    if cipher_text.is_empty() || cipher_text.len() % BLOCK != 0 {
        return Err(CryptoError::DecryptFailed(format!(
            "密文长度 {} 不是 16 的整数倍",
            cipher_text.len()
        )));
    }

    let mut key = [0u8; KEY_LEN];
    pbkdf2::pbkdf2_hmac::<sha1::Sha1>(password.as_bytes(), PBKDF2_SALT, PBKDF2_ROUNDS, &mut key);
    let cipher = aes::Aes128::new_from_slice(&key)
        .map_err(|_| CryptoError::DecryptFailed("AES 密钥长度非法".into()))?;

    let mut prev = IV;
    let mut out = Vec::with_capacity(cipher_text.len());
    for chunk in cipher_text.chunks_exact(BLOCK) {
        let mut block = aes::Block::default();
        block.copy_from_slice(chunk);
        cipher.decrypt_block(&mut block);
        for (i, byte) in block.iter_mut().enumerate() {
            *byte ^= prev[i];
        }
        out.extend_from_slice(&block);
        prev.copy_from_slice(chunk);
    }

    unpad_pkcs7(out)
}

pub fn unpad_pkcs7(mut data: Vec<u8>) -> Result<Vec<u8>, CryptoError> {
    let pad = *data
        .last()
        .ok_or_else(|| CryptoError::DecryptFailed("解密结果为空".into()))? as usize;
    if pad == 0 || pad > BLOCK || pad > data.len() {
        return Err(CryptoError::DecryptFailed("PKCS7 填充非法".into()));
    }
    if !data[data.len() - pad..].iter().all(|b| *b as usize == pad) {
        return Err(CryptoError::DecryptFailed("PKCS7 填充校验失败".into()));
    }
    data.truncate(data.len() - pad);
    Ok(data)
}

/// 令牌形态白名单：只允许放进 HTTP 头和 JSON 都会原样转义的字符集。
pub fn token_shape_ok(token: &str) -> bool {
    token.len() <= 32768
        && !token.is_empty()
        && token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '~' | '+' | '/' | '-'))
}

/// 用同一族参数加密一段明文，供测试构造"真实形态"的凭据文件。
#[cfg(test)]
pub fn encrypt_for_test(plain: &[u8], password: &str) -> Vec<u8> {
    {
        use aes::cipher::BlockEncrypt;
        let mut key = [0u8; KEY_LEN];
        pbkdf2::pbkdf2_hmac::<sha1::Sha1>(
            password.as_bytes(),
            PBKDF2_SALT,
            PBKDF2_ROUNDS,
            &mut key,
        );
        let cipher = aes::Aes128::new_from_slice(&key).unwrap();

        let mut data = plain.to_vec();
        let pad = BLOCK - data.len() % BLOCK;
        data.extend(std::iter::repeat(pad as u8).take(pad));

        let mut prev = IV;
        let mut out = Vec::with_capacity(data.len());
        for chunk in data.chunks_exact(BLOCK) {
            let mut block = aes::Block::default();
            for i in 0..BLOCK {
                block[i] = chunk[i] ^ prev[i];
            }
            cipher.encrypt_block(&mut block);
            out.extend_from_slice(&block);
            prev.copy_from_slice(&block);
        }
        let mut file = FILE_MAGIC.to_vec();
        file.extend_from_slice(&out);
        file
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_envelope() {
        let plain = br#"{"token":"abc.def-123_XYZ"}"#;
        let file = encrypt_for_test(plain, "pw");
        assert!(has_envelope(&file));
        let out = decrypt_envelope(&file, "pw").unwrap();
        assert_eq!(out, plain);
    }

    #[test]
    fn wrong_password_fails() {
        let file = encrypt_for_test(br#"{"token":"x"}"#, "right");
        assert!(decrypt_envelope(&file, "wrong").is_err());
    }

    #[test]
    fn rejects_unknown_magic() {
        let err = decrypt_envelope(b"v99whatever", "pw").unwrap_err();
        assert!(matches!(err, CryptoError::UnsupportedFormat(_)));
    }

    #[test]
    fn rejects_non_block_multiple() {
        assert!(decrypt_cbc(&[1u8; 17], "p").is_err());
    }

    #[test]
    fn token_shape_rules() {
        assert!(token_shape_ok("a.b-c_d~e+f/g"));
        assert!(!token_shape_ok(""));
        assert!(!token_shape_ok("has space"));
        assert!(!token_shape_ok("newline\n"));
    }
}
