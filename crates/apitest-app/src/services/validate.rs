use std::collections::HashSet;

use apitest_core::OAuth2Grant;

use crate::app::ApiTestApp;
use crate::draft::{AuthDraft, ProxyDraft};

impl ApiTestApp {
    pub(crate) fn validate_auth(&self, auth: &AuthDraft) -> Result<(), String> {
        match auth {
            AuthDraft::None | AuthDraft::Unsupported(_) => Ok(()),
            AuthDraft::Basic { username, password } => {
                if username.trim().is_empty() {
                    return Err(self
                        .tr("请输入 Basic 用户名", "Enter the Basic username")
                        .into());
                }
                if !password.is_ready() {
                    return Err(self
                        .tr("请输入 Basic 密码", "Enter the Basic password")
                        .into());
                }
                Ok(())
            }
            AuthDraft::Bearer { token } => {
                if token.is_ready() {
                    Ok(())
                } else {
                    Err(self
                        .tr("请输入 Bearer Token", "Enter the Bearer token")
                        .into())
                }
            }
            AuthDraft::ApiKey { name, value, .. } => {
                if name.trim().is_empty() {
                    return Err(self
                        .tr("请输入 API Key 名称", "Enter the API key name")
                        .into());
                }
                if !value.is_ready() {
                    return Err(self
                        .tr("请输入 API Key 值", "Enter the API key value")
                        .into());
                }
                Ok(())
            }
            AuthDraft::OAuth2 {
                grant,
                token_url,
                client_id,
                username,
                password,
                access_token,
                ..
            } => {
                if access_token.is_ready() {
                    return Ok(());
                }
                if *grant == OAuth2Grant::AuthorizationCodePkce {
                    return Err(self
                        .tr(
                            "授权码 PKCE 流程需要先配置访问令牌",
                            "Authorization Code PKCE requires a cached access token",
                        )
                        .into());
                }
                if token_url.trim().is_empty() {
                    return Err(self
                        .tr("请输入 OAuth2 Token URL", "Enter the OAuth2 token URL")
                        .into());
                }
                if client_id.trim().is_empty() {
                    return Err(self
                        .tr("请输入 OAuth2 Client ID", "Enter the OAuth2 client ID")
                        .into());
                }
                if *grant == OAuth2Grant::Password {
                    if username.trim().is_empty() {
                        return Err(self
                            .tr("请输入 OAuth2 用户名", "Enter the OAuth2 username")
                            .into());
                    }
                    if !password.is_ready() {
                        return Err(self
                            .tr("请输入 OAuth2 密码", "Enter the OAuth2 password")
                            .into());
                    }
                }
                Ok(())
            }
            AuthDraft::Digest { username, password } => {
                if username.trim().is_empty() {
                    return Err(self
                        .tr("请输入 Digest 用户名", "Enter the Digest username")
                        .into());
                }
                if !password.is_ready() {
                    return Err(self
                        .tr("请输入 Digest 密码", "Enter the Digest password")
                        .into());
                }
                Ok(())
            }
            AuthDraft::AwsSigV4 {
                access_key,
                secret_key,
                region,
                service,
                ..
            } => {
                if !access_key.is_ready() {
                    return Err(self
                        .tr("请输入 AWS Access Key", "Enter the AWS access key")
                        .into());
                }
                if !secret_key.is_ready() {
                    return Err(self
                        .tr("请输入 AWS Secret Key", "Enter the AWS secret key")
                        .into());
                }
                if region.trim().is_empty() || service.trim().is_empty() {
                    return Err(self
                        .tr(
                            "请输入 AWS Region 和 Service",
                            "Enter the AWS region and service",
                        )
                        .into());
                }
                Ok(())
            }
        }
    }

    pub(crate) fn validate_proxy(&self, proxy: Option<&ProxyDraft>) -> Result<(), String> {
        let Some(proxy) = proxy else {
            return Ok(());
        };
        if proxy.url.trim().is_empty() {
            return Err(self.tr("请输入代理地址", "Enter the proxy URL").into());
        }
        if proxy.password.is_ready() && proxy.username.trim().is_empty() {
            return Err(self
                .tr(
                    "配置代理密码时必须填写用户名",
                    "A proxy username is required when a password is configured",
                )
                .into());
        }
        Ok(())
    }

    pub(crate) fn validate_environment(&self, index: usize) -> Result<(), String> {
        let environment = &self.environments[index];
        if environment.name.trim().is_empty() {
            return Err(self
                .tr("环境名称不能为空", "Environment name cannot be empty")
                .into());
        }
        if self.environments.iter().enumerate().any(|(other, value)| {
            other != index && value.name.eq_ignore_ascii_case(&environment.name)
        }) {
            return Err(self
                .tr("环境名称不能重复", "Environment names must be unique")
                .into());
        }
        let mut names = HashSet::new();
        for variable in environment
            .variables
            .iter()
            .filter(|variable| !variable.is_empty())
        {
            if variable.name.trim().is_empty() {
                return Err(self
                    .tr("变量名称不能为空", "Variable name cannot be empty")
                    .into());
            }
            if !names.insert(variable.name.trim().to_owned()) {
                return Err(self
                    .tr(
                        "同一环境中的变量名称不能重复",
                        "Variable names must be unique",
                    )
                    .into());
            }
            if !variable.is_ready() {
                return Err(self.tr("请输入密钥值", "Enter the secret value").into());
            }
        }
        Ok(())
    }
}
