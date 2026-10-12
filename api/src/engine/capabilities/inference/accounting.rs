use std::sync::OnceLock;

use anyhow::Result;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use tiktoken_rs::{o200k_base, CoreBPE};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TokenizerKind {
    O200kBase,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InputScope {
    SubmittedContent,
    FullKnownContext,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReportedUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_input_tokens: Option<u64>,
    pub reasoning_output_tokens: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TheoreticalUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub tokenizer: TokenizerKind,
    pub input_scope: InputScope,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Money {
    pub amount: Decimal,
    pub currency: Currency,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Currency {
    Usd,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PricingReference {
    pub provider: String,
    pub model: String,
    pub version: String,
    pub effective_from: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TokenPricing {
    pub input_per_million: Decimal,
    pub cached_input_per_million: Decimal,
    pub output_per_million: Decimal,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TieredTokenPricing {
    pub tiers: Vec<TokenPricingTier>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TokenPricingTier {
    pub minimum_input_tokens: u64,
    pub rates: TokenPricing,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", content = "configuration", rename_all = "snake_case")]
pub enum PricingModel {
    TokenBased(TokenPricing),
    TieredToken(TieredTokenPricing),
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UsageSource {
    Reported,
    Theoretical,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CostSource {
    ProviderReported { provider: String },
    Calculated {
        pricing: PricingReference,
        usage_source: UsageSource,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CostAssessment {
    pub source: CostSource,
    pub amount: Money,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InputTokenComponent {
    pub input_type: String,
    pub input_source: String,
    pub label: String,
    pub tokens: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct CompositeInputSummary {
    pub theoretical_tokens: u64,
    pub component_count: usize,
    pub by_type: std::collections::BTreeMap<String, u64>,
    pub by_source: std::collections::BTreeMap<String, u64>,
}

impl CompositeInputSummary {
    pub fn from_components(components: &[InputTokenComponent]) -> Self {
        let mut summary = Self::default();
        summary.component_count = components.len();
        for component in components {
            summary.theoretical_tokens += component.tokens;
            *summary.by_type.entry(component.input_type.clone()).or_default() += component.tokens;
            *summary.by_source.entry(component.input_source.clone()).or_default() += component.tokens;
        }
        summary
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct InferenceAccounting {
    pub reported_usage: Option<ReportedUsage>,
    pub theoretical_usage: Option<TheoreticalUsage>,
    #[serde(default)]
    pub composite_input: CompositeInputSummary,
    #[serde(default)]
    pub input_components: Vec<InputTokenComponent>,
    pub costs: Vec<CostAssessment>,
}

fn tokenizer() -> Result<&'static CoreBPE> {
    static TOKENIZER: OnceLock<CoreBPE> = OnceLock::new();
    if let Some(tokenizer) = TOKENIZER.get() {
        return Ok(tokenizer);
    }
    let initialized = o200k_base()?;
    Ok(TOKENIZER.get_or_init(|| initialized))
}

pub fn count_tokens(text: &str) -> Result<u64> {
    Ok(tokenizer()?.encode_ordinary(text).len() as u64)
}

pub fn estimate_usage(
    input_text: &str,
    output_text: &str,
    input_scope: InputScope,
) -> Result<TheoreticalUsage> {
    Ok(TheoreticalUsage {
        input_tokens: count_tokens(input_text)?,
        output_tokens: count_tokens(output_text)?,
        tokenizer: TokenizerKind::O200kBase,
        input_scope,
    })
}

pub fn calculate_token_cost(
    input_tokens: u64,
    output_tokens: u64,
    cached_input_tokens: u64,
    rates: &TokenPricing,
) -> Result<Money> {
    anyhow::ensure!(
        cached_input_tokens <= input_tokens,
        "cached input tokens exceed total input tokens"
    );
    anyhow::ensure!(
        rates.input_per_million >= Decimal::ZERO
            && rates.cached_input_per_million >= Decimal::ZERO
            && rates.output_per_million >= Decimal::ZERO,
        "pricing rates must be nonnegative"
    );
    let uncached = input_tokens - cached_input_tokens;
    let million = Decimal::from(1_000_000u64);
    let amount = (
        Decimal::from(uncached) * rates.input_per_million
            + Decimal::from(cached_input_tokens) * rates.cached_input_per_million
            + Decimal::from(output_tokens) * rates.output_per_million
    ) / million;
    Ok(Money {
        amount,
        currency: Currency::Usd,
    })
}

impl PricingModel {
    pub fn calculate(
        &self,
        input_tokens: u64,
        output_tokens: u64,
        cached_input_tokens: u64,
    ) -> Result<Money> {
        let rates = match self {
            Self::TokenBased(rates) => rates,
            Self::TieredToken(pricing) => pricing
                .tiers
                .iter()
                .filter(|tier| input_tokens >= tier.minimum_input_tokens)
                .max_by_key(|tier| tier.minimum_input_tokens)
                .map(|tier| &tier.rates)
                .ok_or_else(|| anyhow::anyhow!("no applicable pricing tier"))?,
        };
        calculate_token_cost(
            input_tokens,
            output_tokens,
            cached_input_tokens,
            rates,
        )
    }
}

impl InferenceAccounting {
    pub fn new(
        reported_usage: Option<ReportedUsage>,
        theoretical_usage: Option<TheoreticalUsage>,
    ) -> Self {
        Self {
            reported_usage,
            theoretical_usage,
            composite_input: CompositeInputSummary::default(),
            input_components: Vec::new(),
            costs: Vec::new(),
        }
    }

    pub fn add_provider_cost(&mut self, provider: String, amount: Money) {
        self.costs.push(CostAssessment {
            source: CostSource::ProviderReported { provider },
            amount,
        });
    }

    pub fn calculate_costs(
        &mut self,
        pricing: &PricingModel,
        reference: &PricingReference,
    ) -> Result<()> {
        if let Some(usage) = &self.reported_usage {
            let amount = pricing.calculate(
                usage.input_tokens,
                usage.output_tokens,
                usage.cached_input_tokens.unwrap_or(0),
            )?;
            self.costs.push(CostAssessment {
                source: CostSource::Calculated {
                    pricing: reference.clone(),
                    usage_source: UsageSource::Reported,
                },
                amount,
            });
        }
        if let Some(usage) = &self.theoretical_usage {
            let amount = pricing.calculate(
                usage.input_tokens,
                usage.output_tokens,
                0,
            )?;
            self.costs.push(CostAssessment {
                source: CostSource::Calculated {
                    pricing: reference.clone(),
                    usage_source: UsageSource::Theoretical,
                },
                amount,
            });
        }
        Ok(())
    }
}

