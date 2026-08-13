//! Route → Lease 后的 Provider 装配组合点（P18-3，ADR-032/033）。
//!
//! 路由决策产出 [`CredentialLease`]（P18-4 池准入）后，宿主在此组合点装配
//! **真实** Provider 并消费其 `builtin_models()`；同时为 reasoning 构造**按
//! `(provider_id, session_id)` 作用域**的持久 protector。
//!
//! ## 组合契约（review 项）
//!
//! - **公开 `compose` caller 只传 `lease` + `CredentialMetadata`**，不得传
//!   `ResolvedCredential` 或 builder；工厂内部持 resolver + builder registry +
//!   protector 工厂，校验 tenant/account/provider 一致后**内部 resolve**。
//! - reasoning protector **复用** `provider_runtime::ReasoningProtector`（删除本模块
//!   重复的 `SessionProtector` trait）；builder 只收 canonical run scope
//!   ([`SessionRunScope`])，正式 host 后续注入真实 Provider builder。
//!
//! ## 红线（ADR-032 收口验收项）
//!
//! - protector **必须**按实际 `(provider_id, session_id)` / run scope 构造或选择；
//! - **禁止**把捕获单一 `BlobScope` 的实例注册为跨 Session 共享的 Provider
//!   全局状态——不同 session 的 protector 互相 fail-closed；
//! - 明文 secret 绝不扩散到 protector；`ResolvedCredential` 仅由 builder 短暂消费；
//! - 不扩张 [`provider_api::ModelProvider`] contract（不新增账号 / 租户 / 客户端职责）。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use agent_domain::{AccountId, ProtectedBlobRef, ProviderId, SessionId, TenantId};
use async_trait::async_trait;
use provider_api::{ModelDefinition, ModelProvider, ResolvedCredential};
use provider_runtime::reasoning::ReasoningProtector;
use thiserror::Error;

use crate::account::{Clock, CredentialKind, CredentialMetadata, NotUsableReason};
use crate::credential::CredentialResolver;
use crate::CredentialLease;

/// 单次 run 的 protector 作用域：`(provider_id, session_id)`。
///
/// 这只是 scope **键**，本身不捕获任何 `BlobScope` / 密钥 / 明文。protector 工厂
/// 据此构造或选择对应作用域的实例。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SessionRunScope {
    pub provider_id: ProviderId,
    pub session_id: SessionId,
}

impl SessionRunScope {
    /// 由 route + lease 决定的 run scope。
    pub fn new(provider_id: ProviderId, session_id: SessionId) -> Self {
        Self {
            provider_id,
            session_id,
        }
    }

    /// 从 lease 推导 scope（lease 的 provider / session 即 route 决策结果）。
    pub fn from_lease(lease: &CredentialLease) -> Self {
        Self::new(lease.provider_id.clone(), lease.session_id.clone())
    }
}

/// 按 [`SessionRunScope`] 构造或选择 reasoning protector。
///
/// review 项：删除本模块重复的 `SessionProtector` trait，**复用**
/// `provider_runtime::ReasoningProtector`。本 trait 只表达「按 scope 构造」的
/// ADR-032 契约：**必须**按实际 `(provider_id, session_id)` 构造或选择 protector；
/// **禁止**返回跨 Session 共享、捕获单一固定 `BlobScope` 的全局实例。不同 scope
/// 的 protector 互相 fail-closed。
pub trait ProtectorFactory: Send + Sync {
    /// 构造 / 选择属于该 scope 的 reasoning protector。
    fn build(&self, scope: &SessionRunScope) -> Arc<dyn ReasoningProtector>;
}

/// 真实 Provider 装配器：把 canonical run scope + 短生命周期 credential +
/// session-scoped protector 组合成 `Arc<dyn ModelProvider>`。宿主提供具体实现
/// （如 `OpenAiProvider::new(...).with_reasoning_protector(protector)`）。
///
/// review 项：builder 可**内部消费** [`ResolvedCredential`]；它只从工厂接收
/// canonical run scope（不是任意 `BlobScope`）。
pub trait ProviderBuilder: Send + Sync {
    /// 该 builder 装配的 Provider id。
    fn provider_id(&self) -> ProviderId;

    /// 装配真实 Provider（消费 credential 与 protector，**不扩张 ModelProvider**）。
    fn build(
        &self,
        scope: &SessionRunScope,
        credential: ResolvedCredential,
        protector: Arc<dyn ReasoningProtector>,
    ) -> Arc<dyn ModelProvider>;
}

/// 真实 Provider 的目录贡献：`builtin_models()` 在组合点的消费形态。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderDescriptor {
    pub id: ProviderId,
    /// 该 Provider 的 `builtin_models()`，供 model-registry / 协商消费。
    pub builtin_models: Vec<ModelDefinition>,
}

/// 组合结果：真实 Provider + session-scoped reasoning protector + 作用域。
pub struct ComposedProvider {
    pub provider: Arc<dyn ModelProvider>,
    pub protector: Arc<dyn ReasoningProtector>,
    pub scope: SessionRunScope,
}

/// 组合错误（**任何变体都不得携带明文 secret**）。
#[derive(Debug, Error)]
pub enum FactoryError {
    /// metadata 与 lease 的 tenant 不一致（跨租户装配被拒绝）。
    #[error("metadata tenant {metadata} does not match lease tenant {lease}")]
    TenantMismatch { metadata: TenantId, lease: TenantId },
    /// metadata 与 lease 的 account 不一致。
    #[error("metadata account {metadata} does not match lease account {lease}")]
    AccountMismatch {
        metadata: AccountId,
        lease: AccountId,
    },
    /// metadata 与 lease 的 provider 不一致。
    #[error("metadata provider {metadata} does not match lease provider {lease}")]
    ProviderMismatch {
        metadata: ProviderId,
        lease: ProviderId,
    },
    /// 没有该 provider 的 builder / descriptor（未注册）。
    #[error("no provider builder registered for {0}")]
    MissingBuilder(ProviderId),
    /// 没有该 provider 的 descriptor（builtin_models 未注册）。
    #[error("no provider descriptor registered for {0}")]
    MissingDescriptor(ProviderId),
    /// 凭据不可取用（state 非 Active / refresh_state 为 Refreshing|Failed /
    /// expires_at 已过期）。携带脱敏原因，**绝不携带明文**。
    #[error("credential not usable for take: {reason}")]
    CredentialNotUsable { reason: NotUsableReason },
    /// 凭据 metadata 的 `secret_ref` 无法解析（已删除 / 未回灌 / 合成 sentinel）。
    #[error("credential secret unresolvable for provider {0}")]
    Unresolvable(ProviderId),
}

/// Route → Lease 后的 Provider 装配工厂。
///
/// review 项：工厂内部持 [`CredentialResolver`] + builder registry + [`ProtectorFactory`]；
/// 公开 [`ProviderFactory::compose`] 只收 `lease` + `CredentialMetadata`，校验
/// tenant/account/provider 一致后**内部 resolve**——caller 不得传
/// `ResolvedCredential` 或 builder。
#[async_trait]
pub trait ProviderFactory: Send + Sync {
    /// 已注册的真实 Provider descriptor（含 `builtin_models()`）。
    fn descriptors(&self) -> &[ProviderDescriptor];

    /// 组合：derive scope from lease → 校验 metadata↔lease 一致 → resolve
    /// secret_ref → build session-scoped protector → build provider。
    async fn compose(
        &self,
        lease: &CredentialLease,
        metadata: &CredentialMetadata,
    ) -> Result<ComposedProvider, FactoryError>;

    /// Optional synthetic health probe for `(provider_id, account_id)`.
    ///
    /// Default: none. Expensive probes stay off unless a factory opts in via
    /// this extension point. Core never branches on Provider name to decide
    /// whether or how to probe.
    fn health_probe(
        &self,
        _provider_id: &ProviderId,
        _account_id: &AccountId,
    ) -> Option<Arc<dyn crate::health::HealthProbe>> {
        None
    }
}

// ---------------------------------------------------------------------------
// 进程内参考实现（测试 + 组合层开发用）
// ---------------------------------------------------------------------------

/// 进程内 session protector：按 scope 隔离 blob，跨 scope fail-closed。
///
/// protect 产出的 ref 形如 `scope:<provider>:<session>:<seq>`；resolve 时校验
/// ref 的 scope 前缀与本句柄 scope 一致，否则 `Unavailable`。这精确镜像 ADR-032
/// 「跨 Session fail-closed」语义，证明「不得把单一 BlobScope 实例共享为跨
/// Session 全局状态」。
pub struct InMemorySessionProtector {
    scope: SessionRunScope,
    blobs: Mutex<HashMap<ProtectedBlobRef, Vec<u8>>>,
    next: AtomicU64,
}

impl InMemorySessionProtector {
    fn new(scope: SessionRunScope) -> Self {
        Self {
            scope,
            blobs: Mutex::new(HashMap::new()),
            next: AtomicU64::new(0),
        }
    }

    fn prefix(scope: &SessionRunScope) -> String {
        format!("scope:{}:{}:", scope.provider_id, scope.session_id)
    }

    /// 本句柄绑定的 run scope（供组合层 / 测试断言作用域一致性）。
    pub fn scope(&self) -> SessionRunScope {
        self.scope.clone()
    }
}

#[async_trait]
impl ReasoningProtector for InMemorySessionProtector {
    async fn protect(
        &self,
        payload: &[u8],
    ) -> Result<ProtectedBlobRef, provider_runtime::reasoning::ReasoningProtectError> {
        let seq = self.next.fetch_add(1, Ordering::Relaxed);
        let blob_ref = ProtectedBlobRef::new(format!("{}{seq}", Self::prefix(&self.scope)));
        self.blobs
            .lock()
            .expect("protector blobs mutex poisoned")
            .insert(blob_ref.clone(), payload.to_vec());
        Ok(blob_ref)
    }

    async fn resolve(
        &self,
        blob_ref: &ProtectedBlobRef,
    ) -> Result<
        protected_blob_store::ProtectedBlob,
        provider_runtime::reasoning::ReasoningProtectError,
    > {
        let expected = Self::prefix(&self.scope);
        if !blob_ref.as_str().starts_with(&expected) {
            // 跨 scope 访问：fail-closed（不泄漏存在性）。
            return Err(provider_runtime::reasoning::ReasoningProtectError::Unavailable);
        }
        self.blobs
            .lock()
            .expect("protector blobs mutex poisoned")
            .get(blob_ref)
            .cloned()
            .map(protected_blob_store::ProtectedBlob::new)
            .ok_or(provider_runtime::reasoning::ReasoningProtectError::Unavailable)
    }
}

/// 进程内 protector 工厂：每次 `build(scope)` 返回绑定该 scope 的新实例。
///
/// 不缓存跨 compose 的全局实例——满足「按实际 scope 构造或选择，禁止跨 Session
/// 共享单一 BlobScope」。记录已构造的 scope 供测试断言。
pub struct InMemorySessionProtectorFactory {
    built_scopes: Mutex<Vec<SessionRunScope>>,
}

impl Default for InMemorySessionProtectorFactory {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemorySessionProtectorFactory {
    /// 创建空工厂。
    pub fn new() -> Self {
        Self {
            built_scopes: Mutex::new(Vec::new()),
        }
    }

    /// 已构造 protector 的 scope 序列（测试断言用）。
    pub fn built_scopes(&self) -> Vec<SessionRunScope> {
        self.built_scopes
            .lock()
            .expect("built_scopes mutex poisoned")
            .clone()
    }
}

impl ProtectorFactory for InMemorySessionProtectorFactory {
    fn build(&self, scope: &SessionRunScope) -> Arc<dyn ReasoningProtector> {
        let protector = Arc::new(InMemorySessionProtector::new(scope.clone()));
        self.built_scopes
            .lock()
            .expect("built_scopes mutex poisoned")
            .push(scope.clone());
        protector
    }
}

/// 进程内 Provider 工厂：持有 descriptor 集 + builder registry +
/// credential resolver + protector 工厂。
///
/// review 项：`compose` 内部完成 resolve；caller 不得传 `ResolvedCredential` /
/// builder。
pub struct InMemoryProviderFactory {
    descriptors: Vec<ProviderDescriptor>,
    builders: HashMap<ProviderId, Arc<dyn ProviderBuilder>>,
    resolver: Arc<dyn CredentialResolver>,
    protector_factory: Arc<dyn ProtectorFactory>,
    clock: Arc<dyn Clock>,
}

impl InMemoryProviderFactory {
    /// 以 descriptor 集、builder registry、resolver 与 protector 工厂构造。
    pub fn new(
        descriptors: Vec<ProviderDescriptor>,
        builders: HashMap<ProviderId, Arc<dyn ProviderBuilder>>,
        resolver: Arc<dyn CredentialResolver>,
        protector_factory: Arc<dyn ProtectorFactory>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            descriptors,
            builders,
            resolver,
            protector_factory,
            clock,
        }
    }

    fn builder_for(&self, provider_id: &ProviderId) -> Option<&Arc<dyn ProviderBuilder>> {
        self.builders.get(provider_id)
    }
}

#[async_trait]
impl ProviderFactory for InMemoryProviderFactory {
    fn descriptors(&self) -> &[ProviderDescriptor] {
        &self.descriptors
    }

    async fn compose(
        &self,
        lease: &CredentialLease,
        metadata: &CredentialMetadata,
    ) -> Result<ComposedProvider, FactoryError> {
        // 1. 校验 metadata ↔ lease 的 tenant / account / provider 一致（跨租户装配拒绝）。
        if metadata.tenant_id != lease.tenant_id {
            return Err(FactoryError::TenantMismatch {
                metadata: metadata.tenant_id.clone(),
                lease: lease.tenant_id.clone(),
            });
        }
        if metadata.account_id != lease.account_id {
            return Err(FactoryError::AccountMismatch {
                metadata: metadata.account_id.clone(),
                lease: lease.account_id.clone(),
            });
        }
        if metadata.provider_id != lease.provider_id {
            return Err(FactoryError::ProviderMismatch {
                metadata: metadata.provider_id.clone(),
                lease: lease.provider_id.clone(),
            });
        }
        // 2. 校验 descriptor 存在（review 项：descriptor / builder / lease 三者
        //    provider_id 必须一致）。
        if !self
            .descriptors
            .iter()
            .any(|descriptor| descriptor.id == lease.provider_id)
        {
            return Err(FactoryError::MissingDescriptor(lease.provider_id.clone()));
        }
        // 3. 查 builder 并校验 builder.provider_id 与 registry key / lease 一致。
        let builder = self
            .builder_for(&lease.provider_id)
            .cloned()
            .ok_or_else(|| FactoryError::MissingBuilder(lease.provider_id.clone()))?;
        if builder.provider_id() != lease.provider_id {
            return Err(FactoryError::MissingBuilder(lease.provider_id.clone()));
        }
        // 4. fail-closed 准入闸门：凭据必须可取用（注入 clock 判定 expires_at，
        //    review 项：拒绝非 Active / refreshing|failed / 已过期）。
        metadata
            .usable_for_take(self.clock.now())
            .map_err(|reason| FactoryError::CredentialNotUsable { reason })?;
        // 5. 内部 resolve：把 metadata.secret_ref 解析为短生命周期 credential。
        let kind = map_credential_kind(metadata.kind);
        let credential = self
            .resolver
            .resolve(&metadata.secret_ref, kind)
            .await
            .map_err(|_| FactoryError::Unresolvable(lease.provider_id.clone()))?;
        // 6. derive scope → build session-scoped protector → build provider。
        let scope = SessionRunScope::from_lease(lease);
        let protector = self.protector_factory.build(&scope);
        let provider = builder.build(&scope, credential, protector.clone());
        Ok(ComposedProvider {
            provider,
            protector,
            scope,
        })
    }
}

/// 把控制面 [`CredentialKind`] 映射到 provider-api 的 canonical 凭据种类。
fn map_credential_kind(kind: CredentialKind) -> provider_api::CredentialKind {
    match kind {
        CredentialKind::ApiKey => provider_api::CredentialKind::ApiKey,
        CredentialKind::OAuth => provider_api::CredentialKind::OAuthBearer,
        CredentialKind::Other => provider_api::CredentialKind::SessionToken,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_domain::CancellationToken;
    use provider_api::{
        CanonicalModelRequest, ModelCapabilities, ProviderError, ProviderEventSink,
    };

    fn lease(provider: &str, session: &str) -> CredentialLease {
        CredentialLease {
            lease_id: crate::LeaseId::new("lease-1"),
            schema_version: crate::CONTROL_PLANE_SCHEMA_VERSION,
            credential_id: agent_domain::CredentialId::new("cred-1"),
            account_id: agent_domain::AccountId::new("acct-1"),
            provider_id: ProviderId::new(provider),
            agent_id: agent_domain::AgentId::new("agent-1"),
            session_id: SessionId::new(session),
            principal_id: agent_domain::PrincipalId::new("principal-1"),
            tenant_id: agent_domain::TenantId::new("tenant-a"),
            acquired_at_ms: 0,
            expires_at_ms: 0,
            version: 2,
        }
    }

    fn descriptor(provider: &str) -> ProviderDescriptor {
        ProviderDescriptor {
            id: ProviderId::new(provider),
            builtin_models: vec![ModelDefinition {
                id: agent_domain::ModelId::new("model-1"),
                display_name: "Model One".into(),
                context_window_tokens: 128_000,
                max_output_tokens: 8_192,
                capabilities: ModelCapabilities {
                    text: true,
                    ..ModelCapabilities::default()
                },
            }],
        }
    }

    /// 透明记录 protector scope 的 mock builder，返回一个记录 id 的 mock provider。
    struct RecordingBuilder {
        id: ProviderId,
        last_scope: Mutex<Option<SessionRunScope>>,
    }

    impl ProviderBuilder for RecordingBuilder {
        fn provider_id(&self) -> ProviderId {
            self.id.clone()
        }
        fn build(
            &self,
            scope: &SessionRunScope,
            _credential: ResolvedCredential,
            _protector: Arc<dyn ReasoningProtector>,
        ) -> Arc<dyn ModelProvider> {
            *self.last_scope.lock().expect("builder mutex poisoned") = Some(scope.clone());
            Arc::new(NoopProvider {
                id: self.id.clone(),
            })
        }
    }

    struct NoopProvider {
        id: ProviderId,
    }

    #[async_trait::async_trait]
    impl ModelProvider for NoopProvider {
        fn id(&self) -> ProviderId {
            self.id.clone()
        }
        async fn list_models(
            &self,
            _credential: Option<&ResolvedCredential>,
        ) -> Result<Vec<ModelDefinition>, ProviderError> {
            Ok(Vec::new())
        }
        async fn stream(
            &self,
            _request: CanonicalModelRequest,
            _sink: &dyn ProviderEventSink,
            _cancel: CancellationToken,
        ) -> Result<provider_api::ModelResponseSummary, ProviderError> {
            Err(ProviderError::new(
                provider_api::ProviderErrorKind::Unknown,
                "noop",
            ))
        }
    }

    fn factory(
        resolver: Arc<crate::credential::InMemoryCredentialResolver>,
    ) -> (
        InMemoryProviderFactory,
        Arc<InMemorySessionProtectorFactory>,
    ) {
        let protector_factory = Arc::new(InMemorySessionProtectorFactory::new());
        let builders: HashMap<ProviderId, Arc<dyn ProviderBuilder>> = [
            (
                ProviderId::new("openai"),
                Arc::new(RecordingBuilder {
                    id: ProviderId::new("openai"),
                    last_scope: Mutex::new(None),
                }) as Arc<dyn ProviderBuilder>,
            ),
            (
                ProviderId::new("anthropic"),
                Arc::new(RecordingBuilder {
                    id: ProviderId::new("anthropic"),
                    last_scope: Mutex::new(None),
                }) as Arc<dyn ProviderBuilder>,
            ),
        ]
        .into_iter()
        .collect();
        let factory = InMemoryProviderFactory::new(
            vec![descriptor("openai"), descriptor("anthropic")],
            builders,
            resolver,
            protector_factory.clone(),
            // 固定时钟（now=1_000_000ms）使 expires_at 断言确定，避免 wall-clock flaky。
            Arc::new(crate::account::FixedClock::new(
                agent_domain::Timestamp::from_unix_millis(1_000_000),
            )),
        );
        (factory, protector_factory)
    }

    /// lease（provider/openai, acct-1, tenant-a, session）+ 对齐的 credential metadata。
    fn openai_metadata() -> crate::account::CredentialMetadata {
        let mut m = crate::account::CredentialMetadata::legacy_synthetic_default();
        m.tenant_id = agent_domain::TenantId::new("tenant-a");
        m.account_id = agent_domain::AccountId::new("acct-1");
        m.provider_id = ProviderId::new("openai");
        m.credential_id = agent_domain::CredentialId::new("cred-1");
        m.synthetic = false;
        m.secret_ref = crate::account::SecretRef::new("pawork.openai", "cred-1");
        m
    }

    #[tokio::test]
    async fn compose_consumes_builtin_models_and_builds_session_scoped_provider() {
        let resolver = Arc::new(crate::credential::InMemoryCredentialResolver::new());
        resolver.put(&openai_metadata().secret_ref, "sk-redacted");
        let (factory, _) = factory(resolver);
        assert_eq!(factory.descriptors().len(), 2);
        // builtin_models 已在组合点消费为 descriptor。
        let openai = factory
            .descriptors()
            .iter()
            .find(|descriptor| descriptor.id.as_str() == "openai")
            .unwrap();
        assert_eq!(openai.builtin_models.len(), 1);

        // review 项：公开 compose caller 只传 lease + metadata，不传 credential/builder。
        let lease = lease("openai", "session-1");
        let composed = factory
            .compose(&lease, &openai_metadata())
            .await
            .expect("compose");

        // protector scope = lease 的 (provider, session)，builder 也看到同 scope。
        assert_eq!(
            composed.scope,
            SessionRunScope::new(ProviderId::new("openai"), SessionId::new("session-1"))
        );
        assert_eq!(composed.provider.id(), ProviderId::new("openai"));
    }

    #[tokio::test]
    async fn distinct_sessions_never_share_a_global_protector_scope() {
        let resolver = Arc::new(crate::credential::InMemoryCredentialResolver::new());
        resolver.put(&openai_metadata().secret_ref, "sk-a");
        let (factory, protector_factory) = factory(resolver);

        let lease_a = lease("openai", "session-a");
        let composed_a = factory
            .compose(&lease_a, &openai_metadata())
            .await
            .expect("compose a");
        let lease_b = lease("openai", "session-b");
        let composed_b = factory
            .compose(&lease_b, &openai_metadata())
            .await
            .expect("compose b");

        // 两个 session 的 scope 不同，protector 不同实例。
        assert_ne!(composed_a.scope, composed_b.scope);
        assert!(!Arc::ptr_eq(&composed_a.protector, &composed_b.protector));
        // 工厂按每次 compose 构造独立 protector（无全局共享单一 scope 实例）。
        assert_eq!(protector_factory.built_scopes().len(), 2);
        assert!(protector_factory
            .built_scopes()
            .iter()
            .all(|scope| scope.provider_id.as_str() == "openai"));

        // 跨 session fail-closed：session-a 的 ref 在 session-b protector 下不可解析。
        let blob_ref = composed_a
            .protector
            .protect(b"reasoning-continuation-bytes")
            .await
            .expect("protect");
        assert!(composed_b.protector.resolve(&blob_ref).await.is_err());
        // 同 session 仍可解析。
        let resolved = composed_a
            .protector
            .resolve(&blob_ref)
            .await
            .expect("resolve same scope");
        assert_eq!(resolved.expose(), b"reasoning-continuation-bytes");
    }

    #[tokio::test]
    async fn compose_rejects_missing_descriptor_builder_and_cross_tenant() {
        let resolver = Arc::new(crate::credential::InMemoryCredentialResolver::new());
        let (factory, _) = factory(resolver);

        // 未注册 provider 的 descriptor 与 builder（mistral 两者皆无）→
        // MissingDescriptor（descriptor 先于 builder 校验）。
        let mut meta = openai_metadata();
        meta.provider_id = ProviderId::new("mistral");
        let lease_mistral = CredentialLease {
            lease_id: crate::LeaseId::new("lease-x"),
            schema_version: crate::CONTROL_PLANE_SCHEMA_VERSION,
            credential_id: agent_domain::CredentialId::new("cred-x"),
            account_id: agent_domain::AccountId::new("acct-1"),
            provider_id: ProviderId::new("mistral"),
            agent_id: agent_domain::AgentId::new("agent-1"),
            session_id: SessionId::new("session-1"),
            principal_id: agent_domain::PrincipalId::new("principal-1"),
            tenant_id: agent_domain::TenantId::new("tenant-a"),
            acquired_at_ms: 0,
            expires_at_ms: 0,
            version: 2,
        };
        assert!(matches!(
            factory.compose(&lease_mistral, &meta).await,
            Err(FactoryError::MissingDescriptor(ref id)) if id.as_str() == "mistral"
        ));

        // metadata 的 tenant 与 lease 不一致 → TenantMismatch（跨租户装配拒绝）。
        let mut cross = openai_metadata();
        cross.tenant_id = agent_domain::TenantId::new("tenant-other");
        let lease_openai = lease("openai", "session-1");
        assert!(matches!(
            factory.compose(&lease_openai, &cross).await,
            Err(FactoryError::TenantMismatch { .. })
        ));

        // metadata 的 provider 与 lease 不一致 → ProviderMismatch。
        let mut prov_mismatch = openai_metadata();
        prov_mismatch.provider_id = ProviderId::new("anthropic");
        assert!(matches!(
            factory.compose(&lease_openai, &prov_mismatch).await,
            Err(FactoryError::ProviderMismatch { .. })
        ));

        // secret_ref 未回灌 → Unresolvable（合成 sentinel fail-closed）。
        let mut missing_secret = openai_metadata();
        missing_secret.secret_ref = crate::account::SecretRef::new("pawork.openai", "never-stored");
        assert!(matches!(
            factory.compose(&lease_openai, &missing_secret).await,
            Err(FactoryError::Unresolvable(ref id)) if id.as_str() == "openai"
        ));
    }

    #[tokio::test]
    async fn compose_rejects_missing_builder_when_descriptor_present() {
        // review 项：descriptor 存在但 builder 缺失 → MissingBuilder。
        let resolver = Arc::new(crate::credential::InMemoryCredentialResolver::new());
        let protector_factory = Arc::new(InMemorySessionProtectorFactory::new());
        // 只注册 anthropic 的 builder，但注册 openai + anthropic 的 descriptor。
        let builders: HashMap<ProviderId, Arc<dyn ProviderBuilder>> = [(
            ProviderId::new("anthropic"),
            Arc::new(RecordingBuilder {
                id: ProviderId::new("anthropic"),
                last_scope: Mutex::new(None),
            }) as Arc<dyn ProviderBuilder>,
        )]
        .into_iter()
        .collect();
        let factory = InMemoryProviderFactory::new(
            vec![descriptor("openai"), descriptor("anthropic")],
            builders,
            resolver,
            protector_factory,
            Arc::new(crate::account::FixedClock::new(
                agent_domain::Timestamp::from_unix_millis(1_000_000),
            )),
        );

        // openai 有 descriptor 但无 builder → MissingBuilder。
        let lease_openai = lease("openai", "session-1");
        assert!(matches!(
            factory.compose(&lease_openai, &openai_metadata()).await,
            Err(FactoryError::MissingBuilder(ref id)) if id.as_str() == "openai"
        ));
    }

    #[tokio::test]
    async fn compose_rejects_credential_not_usable_for_take() {
        // review 项：fail-closed 拒绝非 Active / refreshing|failed / expires_at 已过期。
        let resolver = Arc::new(crate::credential::InMemoryCredentialResolver::new());
        resolver.put(&openai_metadata().secret_ref, "sk-redacted");
        let (factory, _) = factory(resolver);
        let lease_openai = lease("openai", "session-1");

        // 已过期：expires_at < now(1_000_000)。
        let mut expired = openai_metadata();
        expired.expires_at = Some(agent_domain::Timestamp::from_unix_millis(999_999));
        assert!(matches!(
            factory.compose(&lease_openai, &expired).await,
            Err(FactoryError::CredentialNotUsable {
                reason: crate::account::NotUsableReason::Expired
            })
        ));

        // refresh_state = Refreshing。
        let mut refreshing = openai_metadata();
        refreshing.refresh_state = crate::account::RefreshState::Refreshing;
        assert!(matches!(
            factory.compose(&lease_openai, &refreshing).await,
            Err(FactoryError::CredentialNotUsable {
                reason: crate::account::NotUsableReason::Refreshing
            })
        ));

        // refresh_state = Failed。
        let mut failed = openai_metadata();
        failed.refresh_state = crate::account::RefreshState::Failed;
        assert!(matches!(
            factory.compose(&lease_openai, &failed).await,
            Err(FactoryError::CredentialNotUsable {
                reason: crate::account::NotUsableReason::RefreshFailed
            })
        ));

        // 非 Active 状态。
        let mut disabled = openai_metadata();
        disabled.state = crate::account::CredentialState::Disabled;
        assert!(matches!(
            factory.compose(&lease_openai, &disabled).await,
            Err(FactoryError::CredentialNotUsable {
                reason: crate::account::NotUsableReason::Disabled
            })
        ));
    }

    #[tokio::test]
    async fn factory_and_traits_are_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<InMemoryProviderFactory>();
        assert_send_sync::<InMemorySessionProtectorFactory>();
        assert_send_sync::<&dyn ProviderFactory>();
        assert_send_sync::<&dyn ProtectorFactory>();
        assert_send_sync::<&dyn ProviderBuilder>();
    }
}
