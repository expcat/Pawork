//! 握手服务端逻辑：版本协商、capabilities 筛选、ClientAuthentication 验证钩子，
//! 以及信封 api_version 与协商结果的解码校验（IncompatibleVersion 产生路径）。

use pawork_domain::{ConnectionId, CoreInstanceId, GuiClientId};
use crate::app::{ApiHandle, ApiVersion, GlobalSequence};

use crate::resume::{compute_resume_disposition, ResumeContext};
use crate::{
    ClientAuthentication, ClientFrame, GuiCapability, HandshakeRequest, HandshakeResponse,
    ProtocolError, ResumeDisposition, ServerFrame,
};

/// 在客户端候选与服务端单个版本之间协商。
///
/// 取 major 相同的最高共同 minor；无共同 major 时返回 `None`。
pub fn negotiate_api_version(
    client_supported: &[ApiVersion],
    server: ApiVersion,
) -> Option<ApiVersion> {
    negotiate_api_version_with(client_supported, std::slice::from_ref(&server))
}

/// 在客户端候选与服务端版本表之间协商，取 major 交集中最高共同 minor。
pub fn negotiate_api_version_with(
    client_supported: &[ApiVersion],
    server_supported: &[ApiVersion],
) -> Option<ApiVersion> {
    client_supported
        .iter()
        .copied()
        .filter_map(|client| {
            server_supported
                .iter()
                .copied()
                .filter(|server| server.major == client.major)
                .map(|server| ApiVersion {
                    major: client.major,
                    minor: client.minor.min(server.minor),
                })
                .max()
        })
        .max()
}

/// ClientAuthentication 验证钩子：由宿主注入真实认证实现（token、签名等）。
///
/// 返回 `Err` 时握手以 `Rejected` 结束，错误原样进入响应。
pub trait ClientAuthenticator: Send + Sync {
    fn authenticate(&self, authentication: &ClientAuthentication) -> Result<(), ProtocolError>;
}

/// 单次握手的服务端输入：连接身份与重连历史。
///
/// `client_id` / `connection_id` 由宿主分配；`resume_context` 与服务端记录的
/// `last_global_sequence` 用于计算 `HandshakeResponse::Accepted.resume`。
pub struct HandshakeSession {
    pub client_id: GuiClientId,
    pub connection_id: ConnectionId,
    pub resume_context: Option<ResumeContext>,
    pub last_global_sequence: Option<GlobalSequence>,
}

impl HandshakeSession {
    pub fn new(client_id: GuiClientId, connection_id: ConnectionId) -> Self {
        Self {
            client_id,
            connection_id,
            resume_context: None,
            last_global_sequence: None,
        }
    }

    pub fn with_resume_context(mut self, context: ResumeContext) -> Self {
        self.resume_context = Some(context);
        self
    }

    pub fn with_last_global_sequence(mut self, last_global_sequence: GlobalSequence) -> Self {
        self.last_global_sequence = Some(last_global_sequence);
        self
    }
}

/// 握手服务端逻辑：版本协商、认证、capabilities 筛选与 resume disposition 计算。
///
/// 构造后按连接复用（`accept` 只读配置）；`supported_api_versions` 建议来自
/// [`crate::SUPPORTED_API_VERSIONS`]。
pub struct HandshakeService {
    instance_id: CoreInstanceId,
    supported_api_versions: Vec<ApiVersion>,
    supported_capabilities: Vec<GuiCapability>,
    authenticator: Option<Box<dyn ClientAuthenticator>>,
}

impl HandshakeService {
    pub fn new(
        instance_id: CoreInstanceId,
        supported_api_versions: Vec<ApiVersion>,
        supported_capabilities: Vec<GuiCapability>,
    ) -> Self {
        Self {
            instance_id,
            supported_api_versions,
            supported_capabilities,
            authenticator: None,
        }
    }

    /// 注入认证钩子；配置后客户端必须提交可验证的 `authentication`。
    pub fn with_authenticator(mut self, authenticator: Box<dyn ClientAuthenticator>) -> Self {
        self.authenticator = Some(authenticator);
        self
    }

    pub fn supported_api_versions(&self) -> &[ApiVersion] {
        &self.supported_api_versions
    }

    pub fn supported_capabilities(&self) -> &[GuiCapability] {
        &self.supported_capabilities
    }

    /// 处理一次握手请求，返回 Accepted（协商成功）或 Rejected（版本/认证失败）。
    pub fn accept(
        &self,
        request: &HandshakeRequest,
        session: HandshakeSession,
    ) -> HandshakeResponse {
        let request_id = request.request_id.clone();

        let Some(negotiated) = negotiate_api_version_with(
            &request.supported_api_versions,
            &self.supported_api_versions,
        ) else {
            return HandshakeResponse::Rejected {
                request_id,
                error: ProtocolError::incompatible_version(format!(
                    "no compatible API version: client supports {:?}, server supports {:?}",
                    request.supported_api_versions, self.supported_api_versions
                )),
            };
        };

        if let Some(authenticator) = &self.authenticator {
            let Some(authentication) = &request.authentication else {
                return HandshakeResponse::Rejected {
                    request_id,
                    error: ProtocolError::authentication_failed(
                        "client authentication is required",
                    ),
                };
            };
            if let Err(error) = authenticator.authenticate(authentication) {
                return HandshakeResponse::Rejected { request_id, error };
            }
        }

        let capabilities = request
            .capabilities
            .iter()
            .filter(|capability| self.supported_capabilities.contains(capability))
            .cloned()
            .collect();

        let resume = match session.resume_context {
            Some(context) => match session.last_global_sequence {
                Some(last) => {
                    compute_resume_disposition(context.earliest_available, context.current, last)
                }
                None => ResumeDisposition::SnapshotRequired {
                    earliest_available_sequence: context.earliest_available,
                },
            },
            None => ResumeDisposition::SnapshotRequired {
                earliest_available_sequence: GlobalSequence(0),
            },
        };

        HandshakeResponse::Accepted {
            request_id,
            selected_api_version: negotiated,
            handle: ApiHandle {
                instance_id: self.instance_id.clone(),
                api_version: negotiated,
            },
            client_id: session.client_id,
            connection_id: session.connection_id,
            resume,
            capabilities,
        }
    }
}

/// 校验信封 api_version 与协商结果兼容（major 相同且 minor 不高于协商值）。
///
/// 不兼容时产生 `IncompatibleVersion` 错误（[ADR-036]）。
///
/// [ADR-036]: ../../docs/adr/ADR-036-pawork-protocol-versioning.md
pub fn ensure_compatible_api_version(
    envelope_version: ApiVersion,
    negotiated: ApiVersion,
) -> Result<(), ProtocolError> {
    if envelope_version.major == negotiated.major && envelope_version.minor <= negotiated.minor {
        Ok(())
    } else {
        Err(ProtocolError::incompatible_version(format!(
            "envelope api_version {envelope_version:?} is not compatible with negotiated \
             {negotiated:?}"
        )))
    }
}

/// 校验入站 `ClientFrame` 携带的信封 api_version。
pub fn validate_client_frame_api_version(
    frame: &ClientFrame,
    negotiated: ApiVersion,
) -> Result<(), ProtocolError> {
    match frame {
        ClientFrame::Command(envelope) => {
            ensure_compatible_api_version(envelope.api_version, negotiated)
        }
        ClientFrame::Query(envelope) => {
            ensure_compatible_api_version(envelope.api_version, negotiated)
        }
        _ => Ok(()),
    }
}

/// 校验出站 `ServerFrame` 携带的信封 api_version。
pub fn validate_server_frame_api_version(
    frame: &ServerFrame,
    negotiated: ApiVersion,
) -> Result<(), ProtocolError> {
    match frame {
        ServerFrame::Response(envelope) => {
            ensure_compatible_api_version(envelope.api_version, negotiated)
        }
        ServerFrame::Event(envelope) => {
            ensure_compatible_api_version(envelope.api_version, negotiated)
        }
        _ => Ok(()),
    }
}

/// 解码 `ClientFrame` 并校验信封版本；编解码错误映射为线上 `ProtocolError`。
pub fn decode_client_frame_checked(
    bytes: &[u8],
    negotiated: ApiVersion,
) -> Result<ClientFrame, ProtocolError> {
    let frame = crate::codec::decode_client_frame(bytes)?;
    validate_client_frame_api_version(&frame, negotiated)?;
    Ok(frame)
}

/// 解码 `ServerFrame` 并校验信封版本；编解码错误映射为线上 `ProtocolError`。
pub fn decode_server_frame_checked(
    bytes: &[u8],
    negotiated: ApiVersion,
) -> Result<ServerFrame, ProtocolError> {
    let frame = crate::codec::decode_server_frame(bytes)?;
    validate_server_frame_api_version(&frame, negotiated)?;
    Ok(frame)
}
