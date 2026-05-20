use std::sync::OnceLock;

use zti_hw::{Device, Hardware};

use crate::model_registry::ModelProfile;

const F32: u64 = 4;

const ATTN_TENSORS: u64 = 4;
const FFN_TENSORS: u64 = 2;
const PIPELINE_LIVE: u64 = 2;

const BATCH_CEILING: usize = 64;

pub fn recommended_batch_size(profile: &ModelProfile, hw: &Hardware) -> usize {
    let per_sample: u64 = (profile.max_length as u64)
        .saturating_mul(profile.num_hidden_layers as u64)
        .saturating_mul(
            ATTN_TENSORS.saturating_mul(profile.hidden_size as u64)
                + FFN_TENSORS.saturating_mul(profile.intermediate_size as u64),
        )
        .saturating_mul(F32)
        .saturating_mul(PIPELINE_LIVE)
        .max(1);

    let (usable_num, usable_den) = usable_fraction(hw.device);

    let budget = hw.mem_avail.saturating_mul(usable_num) / usable_den;

    let weight_bytes = std::fs::metadata(&profile.onnx_path)
        .map(|m| m.len())
        .unwrap_or(0);
    let weight_overhead = weight_bytes.saturating_mul(2);

    let inference_budget = budget.saturating_sub(weight_overhead);

    let raw = (inference_budget / per_sample).max(1) as usize;
    let pow2 = prev_power_of_two(raw);
    pow2.min(BATCH_CEILING)
}

static FRAC_OVERRIDE: OnceLock<Option<(u64, u64)>> = OnceLock::new();

fn usable_fraction(device: Device) -> (u64, u64) {
    let cached = FRAC_OVERRIDE.get_or_init(|| {
        let s = std::env::var("ZTI_BATCH_MEM_FRAC").ok()?;
        let f = s.parse::<f64>().ok()?;
        (0.05..=0.95).contains(&f).then_some(((f * 100.0) as u64, 100))
    });
    if let Some(v) = cached {
        return *v;
    }
    match device {
        Device::Metal => (4, 10),
        Device::Cuda => (6, 10),
        Device::Vulkan => (5, 10),
        Device::Npu => (5, 10),
        Device::Cpu => (5, 10),
    }
}

#[inline]
fn prev_power_of_two(n: usize) -> usize {
    debug_assert!(n > 0);
    1usize << (usize::BITS - 1 - n.leading_zeros()) as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use zti_hw::{Device, Hardware};

    fn hw(device: Device, mem_avail_gib: u64) -> Hardware {
        Hardware {
            device,
            cpus: 8,
            mem_total: mem_avail_gib << 30,
            mem_avail: mem_avail_gib << 30,
        }
    }

    fn profile(
        hidden: usize,
        layers: usize,
        ffn: usize,
        heads: usize,
        seq: usize,
        onnx: &str,
    ) -> ModelProfile {
        ModelProfile {
            model_id: "test".into(),
            onnx_path: std::path::PathBuf::from(onnx),
            tokenizer_path: std::path::PathBuf::new(),
            dim: hidden,
            max_length: seq,
            pooling: crate::model_registry::PoolingStrategyEnum::Mean,
            query_prefix: None,
            hidden_size: hidden,
            num_hidden_layers: layers,
            intermediate_size: ffn,
            num_attention_heads: heads,
        }
    }

    #[test]
    fn nomic_on_8gib_metal_is_safe() {
        let p = profile(768, 12, 3072, 12, 512, "/nonexistent");
        let b = recommended_batch_size(&p, &hw(Device::Metal, 8));
        assert!((1..=16).contains(&b), "expected [1..=16], got {b}");
    }

    #[test]
    fn bge_small_on_8gib_metal_has_room() {
        let p = profile(384, 6, 1536, 6, 512, "/nonexistent");
        let b = recommended_batch_size(&p, &hw(Device::Metal, 8));
        assert!(b >= 8, "expected >= 8, got {b}");
    }

    #[test]
    fn pathological_clamps_to_one() {
        let p = profile(4096, 32, 16384, 32, 4096, "/nonexistent");
        let b = recommended_batch_size(&p, &hw(Device::Metal, 1));
        assert_eq!(b, 1);
    }
}
