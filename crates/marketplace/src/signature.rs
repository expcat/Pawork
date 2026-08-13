//! Ed25519 package 签名：绑定**已校验**的 manifest + 全部内容条目（P17-3）。
//!
//! 规范签名载荷由已校验归档派生：包身份 + 全部内容条目（path / blake3 / size，
//! 按路径字典序）。entries 已覆盖 package.toml（manifest）摘要，因此签名传递性
//! 绑定 manifest 与每个资源文件；安装时重新计算同一载荷并对照解析出的
//! (id, version) 身份交叉校验——任何不一致一律 fail-closed。

use std::collections::BTreeMap;

use base64::Engine as _;
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use plugin_package::{PackageArchive, PackageId};
use semver::Version;
use serde::{Deserialize, Serialize};

use crate::error::MarketplaceError;

/// 规范载荷格式标识（预留演进）。
pub const SIGNATURE_PAYLOAD_FORMAT: &str = "pawork.package.v1";

/// 承载于 source 索引的 Ed25519 签名信封。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageSignature {
    /// keyring 键 id（不含公钥本体）。
    pub key_id: String,
    /// base64(Ed25519 签名，消息为规范载荷)。
    pub signature_base64: String,
}

/// 规范载荷：确定性 JSON（固定字段顺序 + entries 按路径字典序）。
pub fn canonical_payload(archive: &PackageArchive) -> Vec<u8> {
    #[derive(Serialize)]
    struct Entry {
        path: String,
        blake3: String,
        size: u64,
    }
    #[derive(Serialize)]
    struct Payload {
        format: String,
        id: String,
        version: String,
        entries: Vec<Entry>,
    }
    let mut entries: Vec<Entry> = archive
        .entries
        .iter()
        .map(|entry| Entry {
            path: entry.path.to_posix_string(),
            blake3: entry.blake3_hex.clone(),
            size: entry.size,
        })
        .collect();
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    let payload = Payload {
        format: SIGNATURE_PAYLOAD_FORMAT.to_string(),
        id: archive.manifest.id.as_str().to_string(),
        version: archive.manifest.version.to_string(),
        entries,
    };
    serde_json::to_vec(&payload).expect("canonical payload serialization is infallible")
}

/// 内容摘要：规范载荷的 blake3（内容寻址身份，hash pin 使用）。
pub fn content_digest(archive: &PackageArchive) -> [u8; 32] {
    blake3::hash(&canonical_payload(archive)).into()
}

/// 内容摘要 hex。
pub fn content_digest_hex(archive: &PackageArchive) -> String {
    blake3::hash(&canonical_payload(archive))
        .to_hex()
        .to_string()
}

/// 生成签名信封（发布端 / 测试用）。
pub fn sign_archive(
    key_id: impl Into<String>,
    key: &SigningKey,
    archive: &PackageArchive,
) -> PackageSignature {
    let signature = key.sign(&canonical_payload(archive));
    PackageSignature {
        key_id: key_id.into(),
        signature_base64: base64::engine::general_purpose::STANDARD.encode(signature.to_bytes()),
    }
}

/// 公钥环：key id → Ed25519 公钥。未知 key id 一律 fail-closed。
#[derive(Clone, Debug, Default)]
pub struct Keyring {
    keys: BTreeMap<String, VerifyingKey>,
}

impl Keyring {
    pub fn insert(&mut self, key_id: impl Into<String>, key: VerifyingKey) {
        self.keys.insert(key_id.into(), key);
    }

    pub fn contains(&self, key_id: &str) -> bool {
        self.keys.contains_key(key_id)
    }

    pub fn verifying_key(&self, key_id: &str) -> Option<&VerifyingKey> {
        self.keys.get(key_id)
    }

    /// 校验签名绑定到解析出的 (id, version) 与已校验归档内容。
    ///
    /// manifest 身份不符、未知 key id、base64 / 签名格式非法、验签失败——
    /// 任一情形都返回 Signature 错误。
    pub fn verify_archive(
        &self,
        id: &PackageId,
        version: &Version,
        archive: &PackageArchive,
        signature: &PackageSignature,
    ) -> Result<(), MarketplaceError> {
        let signature_error = |message: String| MarketplaceError::Signature {
            id: id.as_str().to_string(),
            version: version.to_string(),
            message,
        };
        if archive.manifest.id != *id {
            return Err(signature_error(format!(
                "manifest id {} does not match resolved id",
                archive.manifest.id.as_str()
            )));
        }
        if &archive.manifest.version != version {
            return Err(signature_error(format!(
                "manifest version {} does not match resolved version",
                archive.manifest.version
            )));
        }
        let key = self.keys.get(&signature.key_id).ok_or_else(|| {
            signature_error(format!("unknown signing key id {}", signature.key_id))
        })?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(signature.signature_base64.as_bytes())
            .map_err(|error| signature_error(format!("signature is not valid base64: {error}")))?;
        let signature_value = Signature::from_slice(&bytes)
            .map_err(|error| signature_error(format!("malformed ed25519 signature: {error}")))?;
        key.verify_strict(&canonical_payload(archive), &signature_value)
            .map_err(|error| signature_error(format!("ed25519 verification failed: {error}")))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use plugin_package::{read_archive, write_archive, PackageManifest, PackageScope};
    use std::fs;
    use std::path::Path;

    fn signing_key() -> SigningKey {
        SigningKey::from_bytes(&[7u8; 32])
    }

    /// 在 dir 内写一个最小归档并读回（经完整性校验）。
    fn tiny_archive(dir: &Path, id: &str, version: &str, content: &[u8]) -> PackageArchive {
        let manifest = PackageManifest {
            manifest_version: plugin_package::PACKAGE_MANIFEST_VERSION,
            id: PackageId::new(id).unwrap(),
            name: "Tiny".into(),
            version: Version::parse(version).unwrap(),
            license: None,
            description: None,
            entrypoint: None,
            scope: PackageScope::Global,
            dependencies: Vec::new(),
            skills: Vec::new(),
            agents: Vec::new(),
            hooks: Vec::new(),
            mcp: Vec::new(),
            lsp: Vec::new(),
            monitors: Vec::new(),
        };
        fs::write(dir.join("README.md"), content).unwrap();
        write_archive(dir, &manifest).unwrap();
        read_archive(dir).unwrap()
    }

    #[test]
    fn payload_is_deterministic_and_content_bound() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let c = tempfile::tempdir().unwrap();
        let archive_a = tiny_archive(a.path(), "acme.pkg", "1.0.0", b"hello");
        let archive_b = tiny_archive(b.path(), "acme.pkg", "1.0.0", b"hello");
        let archive_c = tiny_archive(c.path(), "acme.pkg", "1.0.0", b"tampered");
        assert_eq!(canonical_payload(&archive_a), canonical_payload(&archive_b));
        assert_eq!(content_digest(&archive_a), content_digest(&archive_b));
        assert_ne!(content_digest(&archive_a), content_digest(&archive_c));
    }

    #[test]
    fn sign_and_verify_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let archive = tiny_archive(dir.path(), "acme.pkg", "1.0.0", b"hello");
        let key = signing_key();
        let mut keyring = Keyring::default();
        keyring.insert("key-1", key.verifying_key());
        let signature = sign_archive("key-1", &key, &archive);
        keyring
            .verify_archive(
                &PackageId::new("acme.pkg").unwrap(),
                &Version::new(1, 0, 0),
                &archive,
                &signature,
            )
            .expect("valid signature verifies");
    }

    #[test]
    fn wrong_key_unknown_key_id_and_bad_encoding_fail_closed() {
        let dir = tempfile::tempdir().unwrap();
        let archive = tiny_archive(dir.path(), "acme.pkg", "1.0.0", b"hello");
        let key = signing_key();
        let mut keyring = Keyring::default();
        keyring.insert("key-1", key.verifying_key());
        let id = PackageId::new("acme.pkg").unwrap();
        let version = Version::new(1, 0, 0);

        // 错误密钥签名 → 验签失败。
        let other = SigningKey::from_bytes(&[9u8; 32]);
        let bad = sign_archive("key-1", &other, &archive);
        assert!(keyring
            .verify_archive(&id, &version, &archive, &bad)
            .is_err());

        // 未知 key id → 拒绝。
        let unknown = sign_archive("nope", &key, &archive);
        assert!(keyring
            .verify_archive(&id, &version, &archive, &unknown)
            .is_err());

        // 非法 base64 → 拒绝。
        let bad_encoding = PackageSignature {
            key_id: "key-1".into(),
            signature_base64: "@@@".into(),
        };
        assert!(keyring
            .verify_archive(&id, &version, &archive, &bad_encoding)
            .is_err());
    }

    #[test]
    fn signature_is_bound_to_identity_and_content() {
        let dir_a = tempfile::tempdir().unwrap();
        let dir_b = tempfile::tempdir().unwrap();
        let archive_a = tiny_archive(dir_a.path(), "acme.pkg", "1.0.0", b"hello");
        let archive_b = tiny_archive(dir_b.path(), "acme.pkg", "1.0.0", b"other");
        let key = signing_key();
        let mut keyring = Keyring::default();
        keyring.insert("key-1", key.verifying_key());
        let signature = sign_archive("key-1", &key, &archive_a);
        let id = PackageId::new("acme.pkg").unwrap();
        let version = Version::new(1, 0, 0);

        // 换内容 → 失败。
        assert!(keyring
            .verify_archive(&id, &version, &archive_b, &signature)
            .is_err());
        // 换解析版本身份 → 失败。
        assert!(keyring
            .verify_archive(&id, &Version::new(1, 1, 0), &archive_a, &signature)
            .is_err());
        // 换解析包 id → 失败。
        assert!(keyring
            .verify_archive(
                &PackageId::new("acme.other").unwrap(),
                &version,
                &archive_a,
                &signature
            )
            .is_err());
    }
}
