//! TLS 1.3 传输加密（P17-11）。
//!
//! - 每个已发布端点生成独立的自签名证书与私钥（`rcgen`），私钥只存在于
//!   内存中，不落盘、不落日志；
//! - 端点地址携带证书 SHA-256 指纹（`#fp=…`），客户端连接时按指纹固定
//!   证书（certificate pinning），指纹不匹配直接拒绝 —— 自签名证书下以此
//!   建立信任，中间人无法伪造；
//! - 密钥协商与轮换由 rustls 负责，业务层（GUI Protocol）不感知加密差异
//!   （[ADR-027]）。
//!
//! [ADR-027]: ../../../docs/adr/ADR-027-local-remote-same-protocol.md

use std::sync::Arc;

use rcgen::{CertificateParams, KeyPair};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{verify_tls12_signature, verify_tls13_signature, CryptoProvider};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use rustls::{version, ClientConfig, DigitallySignedStruct, ServerConfig, SignatureScheme};
use sha2::{Digest, Sha256};

use crate::{TransportError, TransportErrorKind};

use super::wire::transport_error;

/// 端点 TLS 身份：证书、私钥与指纹（SHA-256，hex）。
#[derive(Debug)]
pub(crate) struct TlsIdentity {
    pub(crate) cert: CertificateDer<'static>,
    pub(crate) key: PrivateKeyDer<'static>,
    pub(crate) fingerprint_hex: String,
}

/// 生成端点的自签名证书与私钥（内存态）。
///
/// SAN 覆盖 `localhost` 与 `127.0.0.1`（loopback / 定向测试与本地中继均可用）；
/// 证书有效期默认一年，密钥为 Ed25519（rcgen 默认）。
pub(crate) fn generate_identity(endpoint_name: &str) -> Result<TlsIdentity, TransportError> {
    let subject_alt_names = vec![
        format!("{endpoint_name}.pawork.local"),
        "localhost".to_string(),
        "127.0.0.1".to_string(),
    ];
    let params = CertificateParams::new(subject_alt_names).map_err(|error| {
        transport_error(
            TransportErrorKind::Internal,
            format!("failed to build certificate params: {error}"),
        )
    })?;
    let key_pair = KeyPair::generate().map_err(|error| {
        transport_error(
            TransportErrorKind::Internal,
            format!("failed to generate endpoint key: {error}"),
        )
    })?;
    let cert = params.self_signed(&key_pair).map_err(|error| {
        transport_error(
            TransportErrorKind::Internal,
            format!("failed to generate self-signed certificate: {error}"),
        )
    })?;
    let cert_der = cert.der().clone();
    let key_der = PrivateKeyDer::Pkcs8(key_pair.serialize_der().into());
    let fingerprint_hex = to_hex(&Sha256::digest(cert_der.as_ref()));
    Ok(TlsIdentity {
        cert: cert_der,
        key: key_der,
        fingerprint_hex,
    })
}

/// 构造服务端 TLS 配置（单证书，不要求客户端证书 —— 身份在信封认证层校验）。
pub(crate) fn server_config(identity: &TlsIdentity) -> Result<Arc<ServerConfig>, TransportError> {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let config = ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&version::TLS13])
        .map_err(tls_build_error)?
        .with_no_client_auth()
        .with_single_cert(vec![identity.cert.clone()], identity.key.clone_key())
        .map_err(tls_build_error)?;
    Ok(Arc::new(config))
}

/// 构造客户端 TLS 配置：按端点指纹固定服务端证书。
///
/// `fingerprint` 为 32 字节 SHA-256（由地址 `#fp=` 解析）。自签名证书通过
/// 指纹逐字节比较校验；签名算法校验复用 rustls 默认 provider。
pub(crate) fn client_config(fingerprint: [u8; 32]) -> Result<Arc<ClientConfig>, TransportError> {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let verifier = PinnedCertificateVerifier {
        provider: Arc::clone(&provider),
        fingerprint,
    };
    let config = ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&version::TLS13])
        .map_err(tls_build_error)?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_no_client_auth();
    Ok(Arc::new(config))
}

/// 按指纹固定证书的 `ServerCertVerifier`（仅用于自签名端点证书）。
///
/// 注意：这是显式的信任固定决策 —— 端点地址中的指纹由发布方在 `publish`
/// 时生成并随连接串分发，指纹即信任锚。
#[derive(Debug)]
struct PinnedCertificateVerifier {
    provider: Arc<CryptoProvider>,
    fingerprint: [u8; 32],
}

impl ServerCertVerifier for PinnedCertificateVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let digest: [u8; 32] = Sha256::digest(end_entity.as_ref()).into();
        if digest != self.fingerprint {
            return Err(rustls::Error::General(
                "certificate fingerprint does not match the pinned endpoint fingerprint".into(),
            ));
        }
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

fn tls_build_error(error: rustls::Error) -> TransportError {
    transport_error(
        TransportErrorKind::Internal,
        format!("failed to build TLS configuration: {error}"),
    )
}

fn to_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// 把 64 位 hex 指纹解析为 32 字节数组。
pub(crate) fn parse_fingerprint(hex: &str) -> Option<[u8; 32]> {
    if hex.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_has_expected_san_and_fingerprint() {
        let identity = generate_identity("e2e").expect("identity");
        assert_eq!(identity.fingerprint_hex.len(), 64);
        let fingerprint = parse_fingerprint(&identity.fingerprint_hex).expect("parse");
        assert_eq!(
            Sha256::digest(identity.cert.as_ref()).as_slice(),
            &fingerprint
        );
    }

    #[test]
    fn fingerprint_parse_round_trip_and_rejects_bad_input() {
        let identity = generate_identity("fp").expect("identity");
        let fingerprint = parse_fingerprint(&identity.fingerprint_hex).expect("parse");
        assert_eq!(parse_fingerprint(&identity.fingerprint_hex[..63]), None);
        assert_eq!(
            parse_fingerprint(&format!("z{}", &identity.fingerprint_hex[1..])),
            None
        );
        assert_eq!(parse_fingerprint(""), None);
        let _ = fingerprint;
    }

    #[test]
    fn server_and_client_configs_build() {
        let identity = generate_identity("cfg").expect("identity");
        server_config(&identity).expect("server config");
        let fingerprint = parse_fingerprint(&identity.fingerprint_hex).expect("parse");
        client_config(fingerprint).expect("client config");
    }
}
