//! 本地 base64url 编解码（RFC 4648 §5 URL 安全字母表，无填充）。
//!
//! 仅覆盖 Pawork 需要的 `URL_SAFE_NO_PAD` 子集：PKCE verifier / challenge、
//! 高熵 state 与 JWT payload 段。向量与错误行为与 `base64` crate 的
//! `URL_SAFE_NO_PAD` engine 逐字节对拍一致。

use std::fmt;

/// base64url 字母表（URL 安全变体：`+` → `-`、`/` → `_`）。
const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// base64url 解码错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Base64UrlDecodeError {
    /// 输入携带 `=` 填充（本模块只接受无填充形式）。
    Padding,
    /// 输入包含字母表之外的字符。
    InvalidCharacter { index: usize },
    /// 末尾符号携带非零余位（非规范编码）。
    InvalidLastSymbol { index: usize },
    /// 输入长度 mod 4 == 1，无法映射回整字节。
    InvalidLength,
}

impl fmt::Display for Base64UrlDecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Padding => {
                write!(formatter, "padding character '=' is not accepted (unpadded input required)")
            }
            Self::InvalidCharacter { index } => {
                write!(formatter, "invalid base64url symbol at offset {index}")
            }
            Self::InvalidLastSymbol { index } => {
                write!(formatter, "non-zero trailing bits in last symbol at offset {index}")
            }
            Self::InvalidLength => {
                write!(formatter, "encoded input length is invalid (len % 4 == 1)")
            }
        }
    }
}

impl std::error::Error for Base64UrlDecodeError {}

/// 无填充 base64url 编码：`&[u8]` → URL 安全 ASCII 字符串。
pub fn encode(input: &[u8]) -> String {
    let mut output = String::with_capacity((input.len() * 4 + 2) / 3);
    for chunk in input.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = chunk.get(1).map_or(0, |b| u32::from(*b));
        let b2 = chunk.get(2).map_or(0, |b| u32::from(*b));
        let bits = (b0 << 16) | (b1 << 8) | b2;
        output.push(ALPHABET[usize::try_from(bits >> 18).unwrap() & 0x3f] as char);
        output.push(ALPHABET[usize::try_from(bits >> 12).unwrap() & 0x3f] as char);
        if chunk.len() > 1 {
            output.push(ALPHABET[usize::try_from(bits >> 6).unwrap() & 0x3f] as char);
        }
        if chunk.len() > 2 {
            output.push(ALPHABET[usize::try_from(bits).unwrap() & 0x3f] as char);
        }
    }
    output
}

/// 无填充 base64url 解码：拒绝 `=` 填充、字母表外字符、len%4==1 与
/// 末符号非零余位（与 base64 crate 默认 canonical 行为一致）。
pub fn decode(input: &str) -> Result<Vec<u8>, Base64UrlDecodeError> {
    let bytes = input.as_bytes();
    if bytes.len() % 4 == 1 {
        return Err(Base64UrlDecodeError::InvalidLength);
    }
    let mut output = Vec::with_capacity(bytes.len() / 4 * 3 + 2);
    let mut buffer: u32 = 0;
    let mut buffered_bits: u32 = 0;
    for (index, &byte) in bytes.iter().enumerate() {
        let symbol = decode_symbol(byte, index)?;
        buffer = (buffer << 6) | symbol;
        buffered_bits += 6;
        if buffered_bits >= 8 {
            buffered_bits -= 8;
            output.push((buffer >> buffered_bits) as u8);
            buffer &= (1 << buffered_bits) - 1;
        }
    }
    if buffer != 0 {
        return Err(Base64UrlDecodeError::InvalidLastSymbol {
            index: bytes.len() - 1,
        });
    }
    Ok(output)
}

fn decode_symbol(byte: u8, index: usize) -> Result<u32, Base64UrlDecodeError> {
    match byte {
        b'A'..=b'Z' => Ok(u32::from(byte - b'A')),
        b'a'..=b'z' => Ok(u32::from(byte - b'a') + 26),
        b'0'..=b'9' => Ok(u32::from(byte - b'0') + 52),
        b'-' => Ok(62),
        b'_' => Ok(63),
        b'=' => Err(Base64UrlDecodeError::Padding),
        _ => Err(Base64UrlDecodeError::InvalidCharacter { index }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 固定向量表（向量经 base64 0.22.1 `URL_SAFE_NO_PAD` 逐字节对拍生成：
    /// 对拍阶段以确定性字节模式覆盖 len 0..=64 全量 encode/decode 双向比对
    /// 跑绿后固化；本表覆盖全部 mod-3 长度类，不再依赖 base64 crate）。
    const GOLDEN_VECTORS: &[(&[u8], &str)] = &[
        (&[], ""),
        (&[0x42], "Qg"),
        (&[0x47, 0x2E], "Ry4"),
        (&[0x4D, 0xFF, 0xAC], "Tf-s"),
        (&[0x52, 0xFE, 0x4F, 0x5C], "Uv5PXA"),
        (&[0x58, 0xCF, 0x93, 0xBC, 0xFA], "WM-TvPo"),
        (
            &[
                0xE9, 0x61, 0x4E, 0x7D, 0x0F, 0x94, 0xFA, 0x30, 0xCF, 0x15, 0xFA, 0xA9, 0xBE,
                0x07, 0xFF, 0xAD, 0xB0, 0xA3, 0xCE, 0x05, 0x6B, 0x85, 0x31, 0xF1, 0x64, 0x70,
                0x98, 0x50, 0x28, 0x15, 0x31,
            ],
            "6WFOfQ-U-jDPFfqpvgf_rbCjzgVrhTHxZHCYUCgVMQ",
        ),
        (
            &[
                0xEF, 0x60, 0xF1, 0xF0, 0xD8, 0xCF, 0x34, 0x6A, 0x9E, 0xD5, 0x15, 0x50, 0xD2,
                0x78, 0x52, 0x70, 0xE0, 0x36, 0x2F, 0x2E, 0xA8, 0xDB, 0xCA, 0x01, 0xD0, 0x67,
                0x68, 0xA7, 0x06, 0xBB, 0x00, 0x33,
            ],
            "72Dx8NjPNGqe1RVQ0nhScOA2Ly6o28oB0Gdopwa7ADM",
        ),
        (
            &[
                0xF4, 0x31, 0x35, 0x51, 0x4B, 0x8B, 0x91, 0xCE, 0x69, 0x78, 0x47, 0x98, 0xF1,
                0x01, 0x30, 0x45, 0xC1, 0xC1, 0xF6, 0x90, 0x82, 0x11, 0x0C, 0xEC, 0xFF, 0x7B,
                0x05, 0x74, 0x44, 0x25, 0xA2, 0x74, 0x27,
            ],
            "9DE1UUuLkc5peEeY8QEwRcHB9pCCEQzs_3sFdEQlonQn",
        ),
        (
            &[
                0x43, 0xE2, 0x86, 0x1B, 0xF4, 0x4C, 0xB1, 0x1E, 0x9C, 0x2E, 0x64, 0x1B, 0x5B,
                0xD8, 0x8A, 0x68, 0x32, 0x95, 0x0F, 0x58, 0x24, 0xE5, 0x0D, 0xC9, 0x3C, 0xC9,
                0x02, 0x73, 0x0A, 0x93, 0xBA, 0x27, 0x6C, 0x10, 0xE8, 0x4F, 0xB9, 0x06, 0xA4,
                0x69, 0x94, 0x4A, 0x39, 0x51, 0x17, 0xF9, 0xFF,
            ],
            "Q-KGG_RMsR6cLmQbW9iKaDKVD1gk5Q3JPMkCcwqTuidsEOhPuQakaZRKOVEX-f8",
        ),
        (
            &[
                0x48, 0xE2, 0x29, 0x8F, 0xBD, 0x87, 0xEB, 0x58, 0x6B, 0xEF, 0x7F, 0xC2, 0x6F,
                0x49, 0xDD, 0x2B, 0x61, 0x28, 0x6F, 0x81, 0x60, 0x3C, 0xA6, 0xD9, 0xA7, 0xC0,
                0xD2, 0xCA, 0xE7, 0x39, 0x89, 0xB6, 0xD6, 0x63, 0x37, 0x9B, 0xA5, 0x7B, 0xAE,
                0x1B, 0x57, 0x44, 0xBB, 0xD7, 0x24, 0x70, 0x28, 0x4A,
            ],
            "SOIpj72H61hr73_Cb0ndK2Eob4FgPKbZp8DSyuc5ibbWYzebpXuuG1dEu9ckcChK",
        ),
        (
            &[
                0x9C, 0x63, 0xBF, 0xB9, 0xDA, 0x05, 0x69, 0x0C, 0x69, 0x47, 0xCF, 0x8D, 0xF9,
                0xA9, 0x15, 0x23, 0xB4, 0x86, 0x4F, 0xAB, 0xDD, 0x46, 0xE9, 0xA1, 0x13, 0x23,
                0x6C, 0x96, 0xEB, 0x11, 0x43, 0xAA, 0x97, 0x88, 0x84, 0x54, 0xC8, 0x2F, 0xE9,
                0x36, 0x0A, 0xB8, 0x6A, 0xCE, 0x3E, 0x66, 0x37, 0x35, 0xFB, 0x06, 0xF9, 0x43,
                0xF2, 0x62, 0x98, 0xCA, 0xA0, 0x66, 0xF5, 0xC4, 0xEE, 0x8C, 0x4E,
            ],
            "nGO_udoFaQxpR8-N-akVI7SGT6vdRumhEyNslusRQ6qXiIRUyC_pNgq4as4-Zjc1-wb5Q_JimMqgZvXE7oxO",
        ),
        (
            &[
                0xA2, 0x63, 0x62, 0x2D, 0xA3, 0x40, 0xA2, 0x46, 0x38, 0x08, 0xEA, 0x34, 0x0C,
                0x1A, 0x69, 0xE6, 0xE3, 0x19, 0xB0, 0xD3, 0x19, 0x9C, 0x83, 0xB0, 0x7F, 0x19,
                0x3C, 0xED, 0xC8, 0xB7, 0x12, 0x3A, 0x01, 0xDB, 0xD3, 0xA0, 0xB4, 0xA4, 0xF3,
                0xE8, 0xCD, 0xB3, 0xEC, 0x54, 0x4B, 0xDC, 0x60, 0xFE, 0x56, 0xB8, 0x77, 0x48,
                0x59, 0x22, 0x5D, 0x38, 0x12, 0x17, 0xF1, 0xB7, 0xE4, 0x98, 0xF5, 0xF9,
            ],
            "omNiLaNAokY4COo0DBpp5uMZsNMZnIOwfxk87ci3EjoB29OgtKTz6M2z7FRL3GD-Vrh3SFkiXTgSF_G35Jj1-Q",
        ),
    ];

    #[test]
    fn golden_vectors_encode_byte_for_byte() {
        for (input, expected) in GOLDEN_VECTORS {
            assert_eq!(encode(input), *expected);
        }
    }

    #[test]
    fn golden_vectors_decode_and_roundtrip() {
        for (input, expected) in GOLDEN_VECTORS {
            assert_eq!(decode(expected).expect("golden decode"), *input);
            assert_eq!(
                decode(&encode(input)).expect("roundtrip decode"),
                *input
            );
        }
    }

    #[test]
    fn url_alphabet_symbols_are_covered() {
        // 62/63/62/63 → 显式覆盖 `-` 与 `_` 符号。
        assert_eq!(encode(&[0xFB, 0xFF, 0xBF]), "-_-_");
        assert_eq!(
            decode("-_-_").expect("decode -_-_"),
            vec![0xFB, 0xFF, 0xBF]
        );
    }

    #[test]
    fn decode_rejects_padding_invalid_symbols_length_and_trailing_bits() {
        // 长度检查先于符号扫描："=" 长度为 1，报 InvalidLength 而非 Padding。
        assert_eq!(decode("="), Err(Base64UrlDecodeError::InvalidLength));
        assert_eq!(decode("A"), Err(Base64UrlDecodeError::InvalidLength));
        assert_eq!(decode("A==="), Err(Base64UrlDecodeError::Padding));
        assert_eq!(decode("AB="), Err(Base64UrlDecodeError::Padding));
        assert_eq!(
            decode("A B"),
            Err(Base64UrlDecodeError::InvalidCharacter { index: 1 })
        );
        assert_eq!(
            decode("QR"),
            Err(Base64UrlDecodeError::InvalidLastSymbol { index: 1 })
        );
        assert_eq!(
            decode("AQB"),
            Err(Base64UrlDecodeError::InvalidLastSymbol { index: 2 })
        );
        assert_eq!(
            decode("éA"),
            Err(Base64UrlDecodeError::InvalidCharacter { index: 0 })
        );
    }
}
