//! Frozen dictation metadata and deterministic app/website prompt matching.
use crate::{
    app_rules,
    error::{AppError, Result},
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Default, Serialize, Deserialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Context {
    pub app_identity: Option<String>,
    pub app_name: Option<String>,
    pub window_title: Option<String>,
    pub input_title: Option<String>,
    pub website_url: Option<String>,
}
#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Rule {
    pub id: Uuid,
    pub kind: String,
    pub target: String,
    pub display_name: String,
    pub prompt: String,
    pub enabled: bool,
}
fn valid_text(s: &str, max: usize) -> bool {
    s.len() <= max
        && !s
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
}
pub fn website(value: &str) -> Result<String> {
    let input = value.trim();
    let url = url::Url::parse(&if input.contains("://") {
        input.to_string()
    } else {
        format!("https://{input}")
    })
    .map_err(|_| AppError::invalid("请输入有效的网站地址。"))?;
    if !["http", "https"].contains(&url.scheme())
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(AppError::invalid(
            "请输入不含账号信息的 HTTP 或 HTTPS 网站地址。",
        ));
    }
    let host = url.host_str().unwrap().to_lowercase();
    let port = url.port().map(|p| format!(":{p}")).unwrap_or_default();
    let path = url.path().trim_end_matches('/');
    let value = format!("{host}{port}{path}");
    if value.len() > 1024 {
        return Err(AppError::invalid("网站地址过长。"));
    }
    Ok(value)
}
pub fn public_url(value: &str) -> Option<String> {
    let mut url = url::Url::parse(value).ok()?;
    if !["http", "https"].contains(&url.scheme()) || url.host_str().is_none() {
        return None;
    }
    url.set_username("").ok()?;
    url.set_password(None).ok()?;
    url.set_query(None);
    url.set_fragment(None);
    let value = url.to_string();
    (value.len() <= 1024).then_some(value)
}
pub fn validate(mut rule: Rule) -> Result<Rule> {
    rule.target = match rule.kind.as_str() {
        "app" => app_rules::macos_identity(&rule.target)?,
        "website" => website(&rule.target)?,
        _ => return Err(AppError::invalid("请选择应用或网站规则。")),
    };
    rule.prompt = rule.prompt.trim().into();
    rule.display_name = rule.display_name.trim().into();
    if rule.prompt.is_empty()
        || !valid_text(&rule.prompt, 4096)
        || !valid_text(&rule.display_name, 256)
    {
        return Err(AppError::invalid("请填写整理要求，最多 4096 字节。"));
    }
    Ok(rule)
}
pub fn matching<'a>(rules: &'a [Rule], context: &Context) -> Option<&'a Rule> {
    let website = context
        .website_url
        .as_deref()
        .and_then(|u| self::website(u).ok());
    if let Some(url) = website {
        if let Some(rule) = rules
            .iter()
            .filter(|r| {
                r.enabled
                    && r.kind == "website"
                    && (url == r.target
                        || url
                            .strip_prefix(&r.target)
                            .is_some_and(|tail| tail.starts_with('/')))
            })
            .max_by_key(|r| r.target.len())
        {
            return Some(rule);
        }
    }
    let identity = context
        .app_identity
        .as_deref()
        .and_then(|s| app_rules::macos_identity(s).ok());
    rules
        .iter()
        .find(|r| r.enabled && r.kind == "app" && identity.as_deref() == Some(&r.target))
}
impl Context {
    pub fn model_data(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".into())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn rule(kind: &str, target: &str) -> Rule {
        validate(Rule {
            id: Uuid::new_v4(),
            kind: kind.into(),
            target: target.into(),
            display_name: target.into(),
            prompt: "Keep it concise".into(),
            enabled: true,
        })
        .unwrap()
    }
    #[test]
    fn websites_match_by_host_and_path_boundary_with_specific_paths_first() {
        let rules = vec![
            rule("app", "com.google.Chrome"),
            rule("website", "example.com"),
            rule("website", "https://example.com/mail"),
        ];
        let mut ctx = Context {
            app_identity: Some("com.google.chrome".into()),
            website_url: Some("https://example.com/mail/thread?q=secret#tab".into()),
            ..Default::default()
        };
        assert_eq!(matching(&rules, &ctx).unwrap().target, "example.com/mail");
        ctx.website_url = Some("https://example.com/mailbox".into());
        assert_eq!(matching(&rules, &ctx).unwrap().target, "example.com");
        ctx.website_url = Some("https://example.com.attacker.test/mail".into());
        assert_eq!(matching(&rules, &ctx).unwrap().kind, "app");
        ctx.website_url = None;
        assert_eq!(matching(&rules, &ctx).unwrap().kind, "app");
        ctx.app_identity = None;
        assert!(matching(&rules, &ctx).is_none());
    }
    #[test]
    fn urls_drop_credentials_queries_and_fragments_and_rules_reject_bad_values() {
        assert_eq!(
            public_url("https://user:pass@example.com/mail?token=secret#part").unwrap(),
            "https://example.com/mail"
        );
        assert!(public_url("file:///private.txt").is_none());
        assert!(website("https://user:pass@example.com").is_err());
        let mut r = rule("app", "org.test.editor");
        r.prompt = "x".repeat(4097);
        assert!(validate(r).is_err());
    }
}
