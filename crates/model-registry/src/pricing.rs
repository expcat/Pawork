//! 模型定价与费用估算。

use agent_domain::{Cost, TokenUsage};

/// 百万 token 维度的线性定价，金额使用微单位（micros）避免浮点误差。
///
/// `*_per_mtoken_micros` 表示「每 1_000_000 个该类 token 的费用（最小货币单位的百万分之一）」。
/// 费用估算公式（整数 micro 口径）：
/// `token * per_mtoken_micros / 1_000_000`，四类分量相加。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelPricing {
    pub input_per_mtoken_micros: u64,
    pub output_per_mtoken_micros: u64,
    pub cache_read_per_mtoken_micros: u64,
    pub cache_write_per_mtoken_micros: u64,
    pub currency: String,
}

impl serde::Serialize for ModelPricing {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("ModelPricing", 5)?;
        state.serialize_field("input_per_mtoken_micros", &self.input_per_mtoken_micros)?;
        state.serialize_field("output_per_mtoken_micros", &self.output_per_mtoken_micros)?;
        state.serialize_field(
            "cache_read_per_mtoken_micros",
            &self.cache_read_per_mtoken_micros,
        )?;
        state.serialize_field(
            "cache_write_per_mtoken_micros",
            &self.cache_write_per_mtoken_micros,
        )?;
        state.serialize_field("currency", &self.currency)?;
        state.end()
    }
}

impl<'de> serde::Deserialize<'de> for ModelPricing {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(serde::Deserialize)]
        #[serde(default)]
        struct Raw {
            input_per_mtoken_micros: u64,
            output_per_mtoken_micros: u64,
            cache_read_per_mtoken_micros: u64,
            cache_write_per_mtoken_micros: u64,
            currency: String,
        }

        impl Default for Raw {
            fn default() -> Self {
                Self {
                    input_per_mtoken_micros: 0,
                    output_per_mtoken_micros: 0,
                    cache_read_per_mtoken_micros: 0,
                    cache_write_per_mtoken_micros: 0,
                    currency: "USD".to_string(),
                }
            }
        }

        let raw = Raw::deserialize(deserializer)?;
        Ok(ModelPricing {
            input_per_mtoken_micros: raw.input_per_mtoken_micros,
            output_per_mtoken_micros: raw.output_per_mtoken_micros,
            cache_read_per_mtoken_micros: raw.cache_read_per_mtoken_micros,
            cache_write_per_mtoken_micros: raw.cache_write_per_mtoken_micros,
            currency: raw.currency,
        })
    }
}

/// `1_000_000`：per-million-token 单价到 per-token 的除数。
pub const MILLION: u64 = 1_000_000;

/// 按定价与实际 usage 估算费用（整数 micro 口径，无浮点）。
pub fn estimate_cost(usage: &TokenUsage, pricing: &ModelPricing) -> Cost {
    let input = scale(usage.input_tokens, pricing.input_per_mtoken_micros);
    let output = scale(usage.output_tokens, pricing.output_per_mtoken_micros);
    let cache_read = scale(
        usage.cache_read_tokens,
        pricing.cache_read_per_mtoken_micros,
    );
    let cache_write = scale(
        usage.cache_write_tokens,
        pricing.cache_write_per_mtoken_micros,
    );
    Cost {
        currency: pricing.currency.clone(),
        amount_micros: input + output + cache_read + cache_write,
    }
}

/// `tokens * per_million_micros / 1_000_000`，u128 中间运算防溢出。
fn scale(tokens: u64, per_million_micros: u64) -> u64 {
    if tokens == 0 || per_million_micros == 0 {
        return 0;
    }
    let value = (tokens as u128) * (per_million_micros as u128) / (MILLION as u128);
    u64::try_from(value).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimate_cost_uses_integer_micro_math() {
        // 1000 input tokens @ 5000 micros/Mtoken = 5 micros
        let pricing = ModelPricing {
            input_per_mtoken_micros: 5_000_000,
            output_per_mtoken_micros: 15_000_000,
            cache_read_per_mtoken_micros: 1_000_000,
            cache_write_per_mtoken_micros: 0,
            currency: "USD".into(),
        };
        let usage = TokenUsage {
            input_tokens: 1_000_000,
            output_tokens: 1_000_000,
            cache_read_tokens: 1_000_000,
            cache_write_tokens: 0,
        };
        let cost = estimate_cost(&usage, &pricing);
        assert_eq!(cost.currency, "USD");
        assert_eq!(cost.amount_micros, 5_000_000 + 15_000_000 + 1_000_000);
    }

    #[test]
    fn estimate_cost_handles_fractional_micros_via_truncation() {
        // 4000 tokens @ 2500 micros/Mtoken -> 4000 * 2500 / 1_000_000 = 10 micros (整数截断)
        let pricing = ModelPricing {
            input_per_mtoken_micros: 2_500,
            output_per_mtoken_micros: 0,
            cache_read_per_mtoken_micros: 0,
            cache_write_per_mtoken_micros: 0,
            currency: "USD".into(),
        };
        let usage = TokenUsage {
            input_tokens: 4_000,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
        };
        let cost = estimate_cost(&usage, &pricing);
        assert_eq!(cost.amount_micros, 10);
    }

    #[test]
    fn pricing_round_trips_through_serde() {
        let pricing = ModelPricing {
            input_per_mtoken_micros: 100,
            output_per_mtoken_micros: 300,
            cache_read_per_mtoken_micros: 50,
            cache_write_per_mtoken_micros: 125,
            currency: "USD".into(),
        };
        let json = serde_json::to_string(&pricing).expect("serialize");
        let decoded: ModelPricing = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded, pricing);
    }
}
