use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelKind {
    Sonnet,
    Haiku,
    Opus,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelPricing {
    pub input_cost_per_million: f64,
    pub output_cost_per_million: f64,
    pub cache_write_cost_per_million: f64,
    pub cache_read_cost_per_million: f64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UsageTokens {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_write_tokens: u32,
    pub cache_read_tokens: u32,
}

impl ModelKind {
    pub fn from_name(model: &str) -> Option<Self> {
        let normalized = model.to_ascii_lowercase();
        if normalized.contains("haiku") {
            return Some(Self::Haiku);
        }
        if normalized.contains("opus") {
            return Some(Self::Opus);
        }
        if normalized.contains("sonnet") {
            return Some(Self::Sonnet);
        }
        None
    }

    pub const fn pricing(self) -> ModelPricing {
        match self {
            Self::Sonnet => ModelPricing {
                input_cost_per_million: 15.0,
                output_cost_per_million: 75.0,
                cache_write_cost_per_million: 18.75,
                cache_read_cost_per_million: 1.5,
            },
            Self::Haiku => ModelPricing {
                input_cost_per_million: 1.0,
                output_cost_per_million: 5.0,
                cache_write_cost_per_million: 1.25,
                cache_read_cost_per_million: 0.1,
            },
            Self::Opus => ModelPricing {
                input_cost_per_million: 15.0,
                output_cost_per_million: 75.0,
                cache_write_cost_per_million: 18.75,
                cache_read_cost_per_million: 1.5,
            },
        }
    }
}

pub fn pricing_for_model(model: Option<&str>) -> ModelPricing {
    model
        .and_then(ModelKind::from_name)
        .unwrap_or(ModelKind::Sonnet)
        .pricing()
}

pub fn detect_pricing_from_path(path: &Path) -> ModelPricing {
    pricing_for_model(path.file_name().and_then(|value| value.to_str()))
}

pub fn estimate_cost_usd(usage: UsageTokens, pricing: ModelPricing) -> f64 {
    token_cost(usage.input_tokens, pricing.input_cost_per_million)
        + token_cost(usage.output_tokens, pricing.output_cost_per_million)
        + token_cost(
            usage.cache_write_tokens,
            pricing.cache_write_cost_per_million,
        )
        + token_cost(usage.cache_read_tokens, pricing.cache_read_cost_per_million)
}

fn token_cost(tokens: u32, usd_per_million: f64) -> f64 {
    f64::from(tokens) / 1_000_000.0 * usd_per_million
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{
        detect_pricing_from_path, estimate_cost_usd, pricing_for_model, ModelKind, UsageTokens,
    };

    #[test]
    fn defaults_to_sonnet_pricing() {
        let pricing = pricing_for_model(None);
        assert_eq!(pricing, ModelKind::Sonnet.pricing());
    }

    #[test]
    fn detects_haiku_from_model_name() {
        let pricing = pricing_for_model(Some("claude-3-5-haiku"));
        assert_eq!(pricing, ModelKind::Haiku.pricing());
    }

    #[test]
    fn detects_opus_from_path() {
        let pricing = detect_pricing_from_path(Path::new("/tmp/opus-session.jsonl"));
        assert_eq!(pricing, ModelKind::Opus.pricing());
    }

    #[test]
    fn estimates_cost_from_usage_tokens() {
        let usage = UsageTokens {
            input_tokens: 1_000_000,
            output_tokens: 500_000,
            cache_write_tokens: 100_000,
            cache_read_tokens: 200_000,
        };

        let cost = estimate_cost_usd(usage, ModelKind::Sonnet.pricing());
        assert!((cost - 54.675).abs() < 1e-9);
    }
}
