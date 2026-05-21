use ort::ep::coreml::{ComputeUnits, ModelFormat, SpecializationStrategy};
use ort::ep::{CoreML, ExecutionProviderDispatch};

pub fn register() -> Vec<ExecutionProviderDispatch> {
    let mut ep = CoreML::default()
        .with_compute_units(ComputeUnits::CPUAndGPU)
        .with_model_format(ModelFormat::MLProgram)
        .with_specialization_strategy(SpecializationStrategy::Default)
        .with_static_input_shapes(true)
        .with_subgraphs(true)
        .with_low_precision_accumulation_on_gpu(false);

    match zti_common::paths::models_dir() {
        Ok(dir) => {
            let cache = dir.join("coreml_cache");
            match std::fs::create_dir_all(&cache) {
                Ok(()) => match cache.to_str() {
                    Some(path) => {
                        ep = ep.with_model_cache_dir(path);
                        tracing::debug!(cache = path, "coreml cache enabled");
                    }
                    None => tracing::warn!(
                        "coreml cache path is not valid UTF-8; running without cache"
                    ),
                },
                Err(e) => tracing::warn!(
                    error = %e,
                    "coreml cache dir create failed; running without cache"
                ),
            }
        }
        Err(e) => tracing::warn!(error = %e, "models_dir() failed; coreml cache disabled"),
    }

    tracing::debug!(
        "configuring CoreML execution provider (CPUAndGPU, MLProgram, FP32 accumulation, static shapes)"
    );
    vec![ep.build()]
}
