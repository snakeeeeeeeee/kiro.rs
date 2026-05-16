//! Additional AWS/Kiro streaming endpoints.
//!
//! These mirror the alternate upstream entries used by Kiro-Go. They share the
//! same request body shape as the IDE endpoint but use a different host and/or
//! `X-Amz-Target` header.

use reqwest::RequestBuilder;
use uuid::Uuid;

use super::{KiroEndpoint, RequestContext};

pub const CODEWHISPERER_ENDPOINT_NAME: &str = "codewhisperer";
pub const AMAZONQ_ENDPOINT_NAME: &str = "amazonq";

#[derive(Clone, Copy)]
struct AwsStreamingEndpoint {
    name: &'static str,
    host_prefix: &'static str,
    amz_target: &'static str,
}

impl AwsStreamingEndpoint {
    fn api_region<'a>(&self, ctx: &'a RequestContext<'_>) -> &'a str {
        ctx.credentials.effective_api_region(ctx.config)
    }

    fn host(&self, ctx: &RequestContext<'_>) -> String {
        format!(
            "{}.{}.amazonaws.com",
            self.host_prefix,
            self.api_region(ctx)
        )
    }

    fn api_url(&self, ctx: &RequestContext<'_>) -> String {
        format!("https://{}/generateAssistantResponse", self.host(ctx))
    }

    fn x_amz_user_agent(&self, ctx: &RequestContext<'_>) -> String {
        format!(
            "aws-sdk-js/1.0.34 KiroIDE-{}-{}",
            ctx.config.kiro_version, ctx.machine_id
        )
    }

    fn user_agent(&self, ctx: &RequestContext<'_>) -> String {
        format!(
            "aws-sdk-js/1.0.34 ua/2.1 os/{} lang/js md/nodejs#{} api/codewhispererstreaming#1.0.34 m/E KiroIDE-{}-{}",
            ctx.config.system_version,
            ctx.config.node_version,
            ctx.config.kiro_version,
            ctx.machine_id
        )
    }

    fn decorate_api(&self, req: RequestBuilder, ctx: &RequestContext<'_>) -> RequestBuilder {
        let mut req = req
            .header("x-amzn-codewhisperer-optout", "true")
            .header("x-amzn-kiro-agent-mode", "vibe")
            .header("x-amz-user-agent", self.x_amz_user_agent(ctx))
            .header("user-agent", self.user_agent(ctx))
            .header("host", self.host(ctx))
            .header("amz-sdk-invocation-id", Uuid::new_v4().to_string())
            .header("amz-sdk-request", "attempt=1; max=3")
            .header("Authorization", format!("Bearer {}", ctx.token));

        if !self.amz_target.is_empty() {
            req = req.header("x-amz-target", self.amz_target);
        }
        if ctx.credentials.is_api_key_credential() {
            req = req.header("tokentype", "API_KEY");
        }
        req
    }
}

pub struct CodeWhispererEndpoint(AwsStreamingEndpoint);

impl CodeWhispererEndpoint {
    pub fn new() -> Self {
        Self(AwsStreamingEndpoint {
            name: CODEWHISPERER_ENDPOINT_NAME,
            host_prefix: "codewhisperer",
            amz_target: "AmazonCodeWhispererStreamingService.GenerateAssistantResponse",
        })
    }
}

impl Default for CodeWhispererEndpoint {
    fn default() -> Self {
        Self::new()
    }
}

impl KiroEndpoint for CodeWhispererEndpoint {
    fn name(&self) -> &'static str {
        self.0.name
    }

    fn api_url(&self, ctx: &RequestContext<'_>) -> String {
        self.0.api_url(ctx)
    }

    fn mcp_url(&self, ctx: &RequestContext<'_>) -> String {
        format!("https://q.{}.amazonaws.com/mcp", self.0.api_region(ctx))
    }

    fn decorate_api(&self, req: RequestBuilder, ctx: &RequestContext<'_>) -> RequestBuilder {
        self.0.decorate_api(req, ctx)
    }

    fn decorate_mcp(&self, req: RequestBuilder, ctx: &RequestContext<'_>) -> RequestBuilder {
        super::ide::decorate_ide_mcp(req, ctx)
    }

    fn transform_api_body(&self, body: &str, ctx: &RequestContext<'_>) -> String {
        super::ide::inject_profile_arn(body, &ctx.credentials.profile_arn)
    }
}

pub struct AmazonQEndpoint(AwsStreamingEndpoint);

impl AmazonQEndpoint {
    pub fn new() -> Self {
        Self(AwsStreamingEndpoint {
            name: AMAZONQ_ENDPOINT_NAME,
            host_prefix: "q",
            amz_target: "AmazonQDeveloperStreamingService.SendMessage",
        })
    }
}

impl Default for AmazonQEndpoint {
    fn default() -> Self {
        Self::new()
    }
}

impl KiroEndpoint for AmazonQEndpoint {
    fn name(&self) -> &'static str {
        self.0.name
    }

    fn api_url(&self, ctx: &RequestContext<'_>) -> String {
        self.0.api_url(ctx)
    }

    fn mcp_url(&self, ctx: &RequestContext<'_>) -> String {
        format!("https://q.{}.amazonaws.com/mcp", self.0.api_region(ctx))
    }

    fn decorate_api(&self, req: RequestBuilder, ctx: &RequestContext<'_>) -> RequestBuilder {
        self.0.decorate_api(req, ctx)
    }

    fn decorate_mcp(&self, req: RequestBuilder, ctx: &RequestContext<'_>) -> RequestBuilder {
        super::ide::decorate_ide_mcp(req, ctx)
    }

    fn transform_api_body(&self, body: &str, ctx: &RequestContext<'_>) -> String {
        super::ide::inject_profile_arn(body, &ctx.credentials.profile_arn)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kiro::model::credentials::KiroCredentials;
    use crate::model::config::Config;

    #[test]
    fn codewhisperer_url_uses_api_region() {
        let endpoint = CodeWhispererEndpoint::new();
        let mut config = Config::default();
        config.api_region = Some("eu-central-1".to_string());
        let credentials = KiroCredentials::default();
        let ctx = RequestContext {
            credentials: &credentials,
            token: "token",
            machine_id: "machine",
            config: &config,
        };

        assert_eq!(
            endpoint.api_url(&ctx),
            "https://codewhisperer.eu-central-1.amazonaws.com/generateAssistantResponse"
        );
    }

    #[test]
    fn amazonq_url_uses_q_host() {
        let endpoint = AmazonQEndpoint::new();
        let config = Config::default();
        let credentials = KiroCredentials::default();
        let ctx = RequestContext {
            credentials: &credentials,
            token: "token",
            machine_id: "machine",
            config: &config,
        };

        assert_eq!(
            endpoint.api_url(&ctx),
            "https://q.us-east-1.amazonaws.com/generateAssistantResponse"
        );
    }
}
