//! 配对与设备凭证：仅哈希存储 + 可吊销。
//!
//! 安全约定：
//!
//! - 配对码与设备凭证只以 **加盐 SHA-256 摘要** 存储；明文仅在签发返回值中
//!   出现一次，之后不可读取，不进入日志、审计、Debug 输出；
//! - 摘要比较使用常数时间比较；
//! - pending 配对与已配对设备均为有界槽位，配对码 TTL 到期自动清除；
//! - `revoke` 后凭证立即失效（后续认证返回 [`PairingError::DeviceRevoked`]）。
//!
//! 时间以调用方传入的 Unix 毫秒为准，便于测试注入合成时钟。

use std::collections::VecDeque;
use std::fmt;
use std::sync::{Arc, Mutex};

use rand::Rng;
use sha2::{Digest, Sha256};

/// 默认 pending 配对槽位上限。
pub const DEFAULT_MAX_PENDING: usize = 4;
/// 默认配对码 TTL（毫秒）。
pub const DEFAULT_PENDING_TTL_MS: u64 = 5 * 60 * 1000;
/// 默认已配对设备上限。
pub const DEFAULT_MAX_DEVICES: usize = 16;
/// 默认配对码长度。
pub const DEFAULT_CODE_LENGTH: usize = 8;
/// 默认设备凭证长度。
pub const DEFAULT_CREDENTIAL_LENGTH: usize = 40;

/// 配对码字母表（去掉易混淆字符 i/l/o/0/1）。
const PAIRING_ALPHABET: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";
/// 设备凭证字母表。
const CREDENTIAL_ALPHABET: &[u8] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";

/// 配对/设备策略。
#[derive(Clone, Debug)]
pub struct PairingConfig {
    pub max_pending: usize,
    pub pending_ttl_ms: u64,
    pub max_devices: usize,
    pub pairing_code_length: usize,
    pub credential_length: usize,
}

impl Default for PairingConfig {
    fn default() -> Self {
        Self {
            max_pending: DEFAULT_MAX_PENDING,
            pending_ttl_ms: DEFAULT_PENDING_TTL_MS,
            max_devices: DEFAULT_MAX_DEVICES,
            pairing_code_length: DEFAULT_CODE_LENGTH,
            credential_length: DEFAULT_CREDENTIAL_LENGTH,
        }
    }
}

/// 配对/认证错误（文案不回显任何 Secret）。
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PairingError {
    #[error("pending pairing capacity exhausted")]
    CapacityExhausted,
    #[error("device capacity exhausted")]
    DeviceCapacityExhausted,
    #[error("pairing code is invalid or expired")]
    CodeInvalidOrExpired,
    #[error("device not found")]
    DeviceNotFound,
    #[error("device credential has been revoked")]
    DeviceRevoked,
    #[error("device credential is invalid")]
    CredentialInvalid,
}

/// 配对挑战签发结果（配对码明文仅在此出现一次）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IssuedPairing {
    pub pairing_id: String,
    pub pairing_code: String,
    pub expires_in_ms: u64,
}

/// 激活结果（设备凭证明文仅在此出现一次）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Activation {
    pub device_id: String,
    pub device_label: String,
    pub credential: String,
}

struct PendingPairing {
    device_label: String,
    salt: [u8; 16],
    code_hash: [u8; 32],
    expires_at_ms: u64,
}

struct DeviceRecord {
    device_id: String,
    salt: [u8; 16],
    credential_hash: [u8; 32],
    last_authenticated_ms: Option<u64>,
    revoked: bool,
}

struct Inner {
    pending: VecDeque<PendingPairing>,
    devices: Vec<DeviceRecord>,
    next_pairing: u64,
    next_device: u64,
}

/// 配对/设备注册表（克隆廉价，内部共享同一状态）。
#[derive(Clone)]
pub struct PairingRegistry {
    config: PairingConfig,
    inner: Arc<Mutex<Inner>>,
}

impl PairingRegistry {
    pub fn new() -> Self {
        Self::with_config(PairingConfig::default())
    }

    pub fn with_config(config: PairingConfig) -> Self {
        Self {
            config,
            inner: Arc::new(Mutex::new(Inner {
                pending: VecDeque::new(),
                devices: Vec::new(),
                next_pairing: 1,
                next_device: 1,
            })),
        }
    }

    pub fn config(&self) -> &PairingConfig {
        &self.config
    }

    /// 配对第 1 步：签发配对码。明文仅经返回值出现一次，注册表只存摘要。
    pub fn issue_pairing(
        &self,
        device_label: &str,
        now_ms: u64,
    ) -> Result<IssuedPairing, PairingError> {
        let mut inner = lock(&self.inner);
        purge_expired(&mut inner, now_ms);
        if inner.pending.len() >= self.config.max_pending {
            return Err(PairingError::CapacityExhausted);
        }
        let pairing_id = format!("pairing-{:04}", inner.next_pairing);
        inner.next_pairing += 1;
        let pairing_code = random_string(PAIRING_ALPHABET, self.config.pairing_code_length.max(4));
        let salt = random_salt();
        let code_hash = hash_secret(&salt, &pairing_code);
        inner.pending.push_back(PendingPairing {
            device_label: device_label.to_string(),
            salt,
            code_hash,
            expires_at_ms: now_ms.saturating_add(self.config.pending_ttl_ms),
        });
        Ok(IssuedPairing {
            pairing_id,
            pairing_code,
            expires_in_ms: self.config.pending_ttl_ms,
        })
    }

    /// 配对第 2 步：兑换配对码，签发 device_id 与一次性设备凭证。
    /// 配对码一经尝试兑换即被消费（防重放/防暴力穷举）。
    pub fn activate(&self, pairing_code: &str, now_ms: u64) -> Result<Activation, PairingError> {
        let mut inner = lock(&self.inner);
        purge_expired(&mut inner, now_ms);
        let candidate_hash_index = inner.pending.iter().position(|pending| {
            constant_time_eq(
                &pending.code_hash,
                &hash_secret(&pending.salt, pairing_code),
            )
        });
        let Some(index) = candidate_hash_index else {
            return Err(PairingError::CodeInvalidOrExpired);
        };
        let pending = inner.pending.remove(index).expect("position valid");
        if inner.devices.len() >= self.config.max_devices {
            // 设备槽位已满：fail-closed。配对码已消费，不回滚。
            return Err(PairingError::DeviceCapacityExhausted);
        }
        let device_id = format!("device-{:04}", inner.next_device);
        inner.next_device += 1;
        let credential = random_string(CREDENTIAL_ALPHABET, self.config.credential_length.max(16));
        let salt = random_salt();
        let credential_hash = hash_secret(&salt, &credential);
        inner.devices.push(DeviceRecord {
            device_id: device_id.clone(),
            salt,
            credential_hash,
            last_authenticated_ms: None,
            revoked: false,
        });
        Ok(Activation {
            device_id,
            device_label: pending.device_label,
            credential,
        })
    }

    /// 后续连接认证：device_id + 凭证。
    pub fn authenticate(
        &self,
        device_id: &str,
        credential: &str,
        now_ms: u64,
    ) -> Result<(), PairingError> {
        let mut inner = lock(&self.inner);
        let Some(device) = inner
            .devices
            .iter_mut()
            .find(|device| device.device_id == device_id)
        else {
            return Err(PairingError::DeviceNotFound);
        };
        if device.revoked {
            return Err(PairingError::DeviceRevoked);
        };
        if !constant_time_eq(
            &device.credential_hash,
            &hash_secret(&device.salt, credential),
        ) {
            return Err(PairingError::CredentialInvalid);
        }
        device.last_authenticated_ms = Some(now_ms);
        Ok(())
    }

    /// 吊销设备凭证（立即生效）。返回是否发生了状态变化。
    pub fn revoke(&self, device_id: &str) -> bool {
        let mut inner = lock(&self.inner);
        if let Some(device) = inner
            .devices
            .iter_mut()
            .find(|device| device.device_id == device_id && !device.revoked)
        {
            device.revoked = true;
            true
        } else {
            false
        }
    }

    /// 吊销全部设备；返回本次被吊销的数量。
    pub fn revoke_all(&self) -> usize {
        let mut inner = lock(&self.inner);
        let mut count = 0usize;
        for device in inner.devices.iter_mut() {
            if !device.revoked {
                device.revoked = true;
                count += 1;
            }
        }
        count
    }

    /// 设备是否存在且未被吊销。
    pub fn is_active(&self, device_id: &str) -> bool {
        lock(&self.inner)
            .devices
            .iter()
            .any(|device| device.device_id == device_id && !device.revoked)
    }

    /// 设备是否已被吊销。
    pub fn is_revoked(&self, device_id: &str) -> bool {
        lock(&self.inner)
            .devices
            .iter()
            .any(|device| device.device_id == device_id && device.revoked)
    }

    /// 未过期的 pending 配对数量（只读，不清理）。
    pub fn pending_count(&self) -> usize {
        lock(&self.inner).pending.len()
    }

    /// 活跃（未吊销）设备 id 列表（不含任何凭证材料）。
    pub fn active_device_ids(&self) -> Vec<String> {
        lock(&self.inner)
            .devices
            .iter()
            .filter(|device| !device.revoked)
            .map(|device| device.device_id.clone())
            .collect()
    }
}

impl Default for PairingRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Debug 输出脱敏：不包含任何配对码/凭证/摘要。
impl fmt::Debug for PairingRegistry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let inner = lock(&self.inner);
        formatter
            .debug_struct("PairingRegistry")
            .field("pending", &inner.pending.len())
            .field("devices", &inner.devices.len())
            .field("secrets", &"<redacted>")
            .finish()
    }
}

fn purge_expired(inner: &mut Inner, now_ms: u64) {
    inner
        .pending
        .retain(|pending| pending.expires_at_ms > now_ms);
}

fn random_string(alphabet: &[u8], length: usize) -> String {
    let mut rng = rand::thread_rng();
    (0..length)
        .map(|_| alphabet[rng.gen_range(0..alphabet.len())] as char)
        .collect()
}

fn random_salt() -> [u8; 16] {
    let mut salt = [0u8; 16];
    rand::thread_rng().fill(&mut salt[..]);
    salt
}

fn hash_secret(salt: &[u8; 16], secret: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(salt);
    hasher.update(secret.as_bytes());
    hasher.finalize().into()
}

/// 常数时间比较（定长摘要；长度本身不是 Secret）。
pub(crate) fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in left.iter().zip(right.iter()) {
        diff |= a ^ b;
    }
    diff == 0
}

fn lock(inner: &Arc<Mutex<Inner>>) -> std::sync::MutexGuard<'_, Inner> {
    inner
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> PairingRegistry {
        PairingRegistry::with_config(PairingConfig {
            max_pending: 2,
            pending_ttl_ms: 1_000,
            max_devices: 2,
            pairing_code_length: 8,
            credential_length: 32,
        })
    }

    #[test]
    fn full_pairing_lifecycle_with_hash_only_storage() {
        let pairing = registry();
        let issued = pairing.issue_pairing("phone", 100).expect("issue");
        assert_eq!(issued.pairing_code.len(), 8);
        assert_eq!(issued.expires_in_ms, 1_000);

        let activation = pairing
            .activate(&issued.pairing_code, 200)
            .expect("activate");
        assert_eq!(activation.device_label, "phone");
        assert_eq!(activation.credential.len(), 32);
        assert!(pairing.is_active(&activation.device_id));

        // 配对码一次性：重复兑换失败。
        assert_eq!(
            pairing.activate(&issued.pairing_code, 201),
            Err(PairingError::CodeInvalidOrExpired)
        );

        // 凭证认证：正确通过，错误拒绝。
        pairing
            .authenticate(&activation.device_id, &activation.credential, 300)
            .expect("authenticate");
        assert_eq!(
            pairing.authenticate(&activation.device_id, "wrong", 301),
            Err(PairingError::CredentialInvalid)
        );

        // 吊销后立即失效。
        assert!(pairing.revoke(&activation.device_id));
        assert!(!pairing.revoke(&activation.device_id));
        assert!(pairing.is_revoked(&activation.device_id));
        assert!(!pairing.is_active(&activation.device_id));
        assert_eq!(
            pairing.authenticate(&activation.device_id, &activation.credential, 400),
            Err(PairingError::DeviceRevoked)
        );
    }

    #[test]
    fn secrets_are_not_readable_from_registry() {
        let pairing = PairingRegistry::new();
        let issued = pairing.issue_pairing("phone", 0).expect("issue");
        let activation = pairing.activate(&issued.pairing_code, 1).expect("activate");
        let debug = format!("{pairing:?}");
        assert!(!debug.contains(&issued.pairing_code), "Debug 泄漏配对码");
        assert!(!debug.contains(&activation.credential), "Debug 泄漏凭证");
        // 注册表不提供任何读回 Secret 的接口：活跃设备只能取 id。
        assert_eq!(
            pairing.active_device_ids(),
            vec![activation.device_id.clone()]
        );
    }

    #[test]
    fn expired_pairing_code_is_rejected() {
        let pairing = registry();
        let issued = pairing.issue_pairing("phone", 100).expect("issue");
        // TTL 1000ms：到期后兑换失败且 pending 被清除。
        assert_eq!(
            pairing.activate(&issued.pairing_code, 100 + 1_001),
            Err(PairingError::CodeInvalidOrExpired)
        );
        assert_eq!(pairing.pending_count(), 0);
    }

    #[test]
    fn pending_and_device_slots_are_bounded() {
        let pairing = registry(); // max_pending=2, max_devices=2
        pairing.issue_pairing("a", 0).expect("issue 1");
        pairing.issue_pairing("b", 0).expect("issue 2");
        assert_eq!(
            pairing.issue_pairing("c", 0),
            Err(PairingError::CapacityExhausted)
        );

        // 设备槽位：换新注册表验证，两台设备后第三台 fail-closed
        // （activate 会释放 pending 槽位，issue 不会撞 pending 上限）。
        let registry2 = registry();
        let first = registry2.issue_pairing("x", 0).expect("issue");
        registry2
            .activate(&first.pairing_code, 1)
            .expect("activate");
        let second = registry2.issue_pairing("y", 0).expect("issue");
        registry2
            .activate(&second.pairing_code, 1)
            .expect("activate");
        let third = registry2.issue_pairing("z", 0).expect("issue");
        assert_eq!(
            registry2.activate(&third.pairing_code, 1),
            Err(PairingError::DeviceCapacityExhausted)
        );
    }

    #[test]
    fn revoke_all_invalidates_every_device() {
        let pairing = registry();
        let mut ids = Vec::new();
        for index in 0..2 {
            let issued = pairing
                .issue_pairing(&format!("d{index}"), 0)
                .expect("issue");
            let activation = pairing.activate(&issued.pairing_code, 1).expect("activate");
            ids.push(activation.device_id);
        }
        assert_eq!(pairing.revoke_all(), 2);
        assert_eq!(pairing.revoke_all(), 0);
        for id in &ids {
            assert!(!pairing.is_active(id));
        }
    }

    #[test]
    fn constant_time_eq_matches_semantics() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
    }
}
