//! Model routing based on task purpose.
//!
//! [`ModelRouter`] selects a provider/model based on [`RoutingPurpose`],
//! falling back to a default provider when no rule matches.

use serde::{Deserialize, Serialize};

/// A rule mapping a task purpose to a specific provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelRoutingRule {
    /// The purpose this rule applies to.
    pub purpose: RoutingPurpose,
    /// The provider name to use for this purpose.
    pub provider_name: String,
}

/// The purpose of an LLM invocation, used for cost-aware routing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RoutingPurpose {
    /// High-level task planning (typically needs a strong model).
    Planning,
    /// Task execution (can often use a cheaper model).
    Execution,
    /// Output verification and validation.
    Verification,
    /// Text embedding for vector search.
    Embedding,
}

/// Routes LLM requests to providers based on task purpose.
pub struct ModelRouter {
    rules: Vec<ModelRoutingRule>,
    default_provider: String,
}

impl ModelRouter {
    /// Create a new router with the given rules and default provider.
    pub fn new(rules: Vec<ModelRoutingRule>, default_provider: String) -> Self {
        Self {
            rules,
            default_provider,
        }
    }

    /// Return the provider name for the given purpose.
    ///
    /// Returns the first matching rule's provider, or the default provider if
    /// no rule matches.
    pub fn route(&self, purpose: &RoutingPurpose) -> &str {
        for rule in &self.rules {
            if &rule.purpose == purpose {
                return &rule.provider_name;
            }
        }
        &self.default_provider
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_by_purpose() {
        let rules = vec![
            ModelRoutingRule {
                purpose: RoutingPurpose::Planning,
                provider_name: "anthropic-opus".into(),
            },
            ModelRoutingRule {
                purpose: RoutingPurpose::Execution,
                provider_name: "openai-gpt4o-mini".into(),
            },
            ModelRoutingRule {
                purpose: RoutingPurpose::Embedding,
                provider_name: "openai-embedding".into(),
            },
        ];
        let router = ModelRouter::new(rules, "openai-gpt4o".into());

        assert_eq!(router.route(&RoutingPurpose::Planning), "anthropic-opus");
        assert_eq!(
            router.route(&RoutingPurpose::Execution),
            "openai-gpt4o-mini"
        );
        assert_eq!(
            router.route(&RoutingPurpose::Embedding),
            "openai-embedding"
        );
    }

    #[test]
    fn falls_back_to_default() {
        let rules = vec![ModelRoutingRule {
            purpose: RoutingPurpose::Planning,
            provider_name: "anthropic-opus".into(),
        }];
        let router = ModelRouter::new(rules, "openai-gpt4o".into());

        // Verification has no rule, should fall back to default
        assert_eq!(router.route(&RoutingPurpose::Verification), "openai-gpt4o");
        assert_eq!(router.route(&RoutingPurpose::Execution), "openai-gpt4o");
    }

    #[test]
    fn empty_rules_always_returns_default() {
        let router = ModelRouter::new(vec![], "default-provider".into());

        assert_eq!(router.route(&RoutingPurpose::Planning), "default-provider");
        assert_eq!(router.route(&RoutingPurpose::Execution), "default-provider");
        assert_eq!(
            router.route(&RoutingPurpose::Verification),
            "default-provider"
        );
        assert_eq!(router.route(&RoutingPurpose::Embedding), "default-provider");
    }

    #[test]
    fn first_matching_rule_wins() {
        let rules = vec![
            ModelRoutingRule {
                purpose: RoutingPurpose::Planning,
                provider_name: "first-provider".into(),
            },
            ModelRoutingRule {
                purpose: RoutingPurpose::Planning,
                provider_name: "second-provider".into(),
            },
        ];
        let router = ModelRouter::new(rules, "default".into());

        assert_eq!(router.route(&RoutingPurpose::Planning), "first-provider");
    }

    #[test]
    fn routing_rule_serde_roundtrip() {
        let rule = ModelRoutingRule {
            purpose: RoutingPurpose::Execution,
            provider_name: "cheap-model".into(),
        };
        let json = serde_json::to_string(&rule).unwrap();
        let parsed: ModelRoutingRule = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.purpose, RoutingPurpose::Execution);
        assert_eq!(parsed.provider_name, "cheap-model");
    }

    #[test]
    fn routing_purpose_serde_roundtrip() {
        for purpose in [
            RoutingPurpose::Planning,
            RoutingPurpose::Execution,
            RoutingPurpose::Verification,
            RoutingPurpose::Embedding,
        ] {
            let json = serde_json::to_string(&purpose).unwrap();
            let parsed: RoutingPurpose = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, purpose);
        }
    }
}
