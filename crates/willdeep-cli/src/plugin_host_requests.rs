//! 插件 MCP 进程反向请求宿主：出图、问模型。
//!
//! 与 macOS 宿主 `AgentMCPHostRequests` 同一份约定：方法名、参数与结果形状、
//! 错误码都一样，权限、路径钳制、输出目录与页面桥完全相同（直接复用页面桥那两条
//! 实现），凭据永远不出宿主。
//!
//! 只宣告真的实现了的方法。宿主代管 TTS（`willdeep/audio/synthesize`）两个宿主
//! 都还没有，不宣告：插件据此直接报「宿主不支持」，而不是发出来干等到超时。

use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use serde_json::Value;
use willdeep_core::mcp::HostRequestError;
use willdeep_core::plugin::host_requests::{AI_COMPLETE, IMAGE_GENERATE, ROUNDTABLE_RUN};
use willdeep_core::plugin::{PluginHostRequests, PluginPermission, PluginRequestContext};

use crate::plugin_capabilities::{INVALID_REQUEST_CODES, ImageRequest, PluginAiHost};
use crate::plugin_web::{AiCompleteRequest, PluginWebError};

pub(crate) struct HostRequests {
    home: PathBuf,
    config_path: PathBuf,
    /// 与聊天端、页面桥同一份工作区白名单（Web 进程里是同一个 Arc）。
    workspaces: Arc<RwLock<Vec<PathBuf>>>,
    usage_context: Option<(uuid::Uuid, PathBuf)>,
    production_prompt: bool,
}

impl HostRequests {
    pub(crate) fn new(
        home: PathBuf,
        config_path: PathBuf,
        workspaces: Arc<RwLock<Vec<PathBuf>>>,
    ) -> Self {
        Self {
            home,
            config_path,
            workspaces,
            usage_context: None,
            production_prompt: false,
        }
    }

    pub(crate) fn with_usage_context(mut self, session: uuid::Uuid, workspace: PathBuf) -> Self {
        self.usage_context = Some((session, workspace));
        self
    }

    pub(crate) fn with_production_prompt(mut self) -> Self {
        self.production_prompt = true;
        self
    }

    fn ai_host(&self) -> PluginAiHost<'_> {
        PluginAiHost {
            home: &self.home,
            config_path: &self.config_path,
            usage_context: self.usage_context.clone(),
            production_prompt: self.production_prompt,
            workspaces: self
                .workspaces
                .read()
                .unwrap_or_else(|error| error.into_inner())
                .clone(),
        }
    }
}

/// 页面桥的错误 → JSON-RPC 错误。参数问题 -32602，其余 -32000。
fn rpc_error(error: PluginWebError) -> HostRequestError {
    match error {
        PluginWebError::BadRequest(code) => {
            let bare = code.split(':').next().unwrap_or_default().trim();
            if INVALID_REQUEST_CODES.contains(&bare) {
                HostRequestError::invalid_params(code)
            } else {
                HostRequestError::failed(code)
            }
        }
        PluginWebError::Host(error) => HostRequestError::failed(error.to_string()),
        PluginWebError::Internal(message) => HostRequestError::failed(message),
    }
}

fn parse<T: serde::de::DeserializeOwned>(params: Value) -> Result<T, HostRequestError> {
    let params = if params.is_null() {
        Value::Object(Default::default())
    } else {
        params
    };
    serde_json::from_value(params)
        .map_err(|error| HostRequestError::invalid_params(format!("invalid params: {error}")))
}

#[async_trait]
impl PluginHostRequests for HostRequests {
    fn methods(&self) -> Vec<String> {
        vec![
            IMAGE_GENERATE.to_owned(),
            AI_COMPLETE.to_owned(),
            ROUNDTABLE_RUN.to_owned(),
            crate::plugin_screenplay::CONTROL.to_owned(),
            crate::plugin_production::CONTROL.to_owned(),
        ]
    }

    async fn handle(
        &self,
        context: &PluginRequestContext,
        method: &str,
        params: Value,
    ) -> Result<Value, HostRequestError> {
        match method {
            crate::plugin_production::CONTROL => {
                context.require(PluginPermission::AiChat)?;
                if context.plugin_id != "willdeep-video-studio" {
                    return Err(HostRequestError::failed("production_plugin_denied"));
                }
                let workspaces = self
                    .workspaces
                    .read()
                    .unwrap_or_else(|error| error.into_inner())
                    .clone();
                crate::plugin_production::control(
                    &self.home,
                    &workspaces,
                    self.usage_context.as_ref().map(|(id, _)| *id),
                    params,
                )
                .await
            }
            crate::plugin_screenplay::CONTROL => {
                context.require(PluginPermission::AiChat)?;
                if context.plugin_id != "willdeep-video-studio" {
                    return Err(HostRequestError::failed("screenplay_plugin_denied"));
                }
                let workspaces = self
                    .workspaces
                    .read()
                    .unwrap_or_else(|error| error.into_inner())
                    .clone();
                crate::plugin_screenplay::control(&self.home, &workspaces, params).await
            }
            ROUNDTABLE_RUN => {
                context.require(PluginPermission::AiChat)?;
                let request: crate::plugin_roundtable::Request = parse(params)?;
                request.validate()?;
                // 插件身份来自已批准的清单，仍检查路径分量，不允许跨插件写报告。
                if !context
                    .plugin_id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
                    || matches!(context.plugin_id.as_str(), "" | "." | "..")
                {
                    return Err(HostRequestError::failed("Invalid plugin identity."));
                }
                let model = RoundtableModel {
                    host: self,
                    context,
                };
                let result = crate::plugin_roundtable::run(request, &model).await?;
                crate::plugin_roundtable::save(
                    &self.home.join("plugin-data").join(&context.plugin_id),
                    &result,
                )?;
                Ok(result)
            }
            IMAGE_GENERATE => {
                context.require(PluginPermission::AiImage)?;
                let request: ImageRequest = parse(params)?;
                crate::plugin_capabilities::generate_image(
                    &self.ai_host(),
                    &context.plugin_id,
                    request,
                )
                .await
                .map_err(rpc_error)
            }
            AI_COMPLETE => {
                context.require(PluginPermission::AiChat)?;
                let request: AiCompleteRequest = parse(params)?;
                let permits = |permission: PluginPermission| -> Result<(), PluginWebError> {
                    context
                        .require(permission)
                        .map_err(|error| PluginWebError::BadRequest(error.message))
                };
                crate::plugin_web::complete(
                    &self.ai_host(),
                    &context.plugin_id,
                    request,
                    &permits,
                    None,
                )
                .await
                .map_err(rpc_error)
            }
            other => Err(HostRequestError::method_not_found(format!(
                "Method not found: {other}"
            ))),
        }
    }
}

struct RoundtableModel<'a> {
    host: &'a HostRequests,
    context: &'a PluginRequestContext,
}

#[async_trait]
impl crate::plugin_roundtable::Completion for RoundtableModel<'_> {
    async fn complete(
        &self,
        request: &crate::plugin_roundtable::Request,
        system: &str,
        user: &str,
        tokens: u32,
    ) -> Result<Value, HostRequestError> {
        let request: AiCompleteRequest = parse(serde_json::json!({
            "system": system, "messages": [{"role":"user", "content":user}],
            "provider":request.provider, "model":request.model, "max_output_tokens":tokens,
        }))?;
        let permits = |permission| {
            self.context
                .require(permission)
                .map_err(|error| PluginWebError::BadRequest(error.message))
        };
        crate::plugin_web::complete(
            &self.host.ai_host(),
            &self.context.plugin_id,
            request,
            &permits,
            None,
        )
        .await
        .map_err(rpc_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn production_prompt_scope_is_host_only_and_not_implied_by_usage_linkage() {
        let host = HostRequests::new(
            PathBuf::from("/tmp/fixture-home"),
            PathBuf::from("/tmp/fixture-config"),
            Arc::new(RwLock::new(vec![])),
        )
        .with_usage_context(
            uuid::Uuid::new_v4(),
            PathBuf::from("/tmp/fixture-workspace"),
        );
        assert!(!host.ai_host().production_prompt);
        assert!(host.with_production_prompt().ai_host().production_prompt);
    }
    use serde_json::json;

    fn scratch() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "willdeep-host-requests-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&root).expect("scratch");
        root
    }

    fn handler(home: &std::path::Path) -> HostRequests {
        HostRequests::new(
            home.to_path_buf(),
            home.join("config.toml"),
            Arc::new(RwLock::new(Vec::new())),
        )
    }

    fn context(permissions: Vec<PluginPermission>) -> PluginRequestContext {
        PluginRequestContext {
            plugin_id: "demo".to_owned(),
            permissions,
        }
    }

    #[test]
    fn advertises_only_what_is_implemented() {
        let home = scratch();
        let methods = handler(&home).methods();
        assert_eq!(
            methods,
            vec![
                IMAGE_GENERATE,
                AI_COMPLETE,
                ROUNDTABLE_RUN,
                crate::plugin_screenplay::CONTROL,
                crate::plugin_production::CONTROL
            ]
        );
        assert!(
            !methods
                .iter()
                .any(|method| method == willdeep_core::plugin::host_requests::AUDIO_SYNTHESIZE)
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    #[tokio::test]
    async fn permissions_come_from_the_manifest_not_the_request() {
        let home = scratch();
        let handler = handler(&home);
        let denied = handler
            .handle(&context(vec![]), IMAGE_GENERATE, json!({"prompt": "cat"}))
            .await
            .expect_err("no ai.image");
        assert_eq!(denied.code, HostRequestError::FAILED);
        assert!(denied.message.contains("ai.image"));
        let denied = handler
            .handle(
                &context(vec![PluginPermission::AiImage]),
                AI_COMPLETE,
                json!({"messages": [{"role": "user", "content": "hi"}]}),
            )
            .await
            .expect_err("no ai.chat");
        assert!(denied.message.contains("ai.chat"));
        let denied = handler
            .handle(&context(vec![]), ROUNDTABLE_RUN, json!({}))
            .await
            .expect_err("no ai.chat");
        assert!(denied.message.contains("ai.chat"));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[tokio::test]
    async fn invalid_requests_are_invalid_params_before_any_network() {
        let home = scratch();
        let handler = handler(&home);
        let image = context(vec![PluginPermission::AiImage, PluginPermission::AiChat]);
        for (method, params) in [
            (IMAGE_GENERATE, json!({"prompt": "  "})),
            (IMAGE_GENERATE, json!({"prompt": "cat", "model": "dall-e"})),
            (
                IMAGE_GENERATE,
                json!({"prompt": "cat", "provider": "openai"}),
            ),
            (IMAGE_GENERATE, json!({"prompt": "cat", "size": "7x7"})),
            (IMAGE_GENERATE, json!({"prompt": 5})),
            (AI_COMPLETE, json!({"messages": []})),
            (
                AI_COMPLETE,
                json!({"messages": [{"role": "user", "content": "x", "videoPaths": ["/a.mp4"]}]}),
            ),
        ] {
            let error = handler
                .handle(&image, method, params.clone())
                .await
                .expect_err("rejected");
            assert_eq!(
                error.code,
                HostRequestError::INVALID_PARAMS,
                "{method} {params} -> {}",
                error.message
            );
        }
        // 没有 some.im 凭据：请求本身没问题，是宿主给不了——-32000，不是 -32602。
        let unavailable = handler
            .handle(&image, IMAGE_GENERATE, json!({"prompt": "cat"}))
            .await
            .expect_err("no credentials");
        assert_eq!(unavailable.code, HostRequestError::FAILED);
        let _ = std::fs::remove_dir_all(&home);
    }
}
