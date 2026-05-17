use ort::ep::coreml::{ComputeUnits, ModelFormat, SpecializationStrategy};
use ort::ep::{CoreML, ExecutionProviderDispatch};

pub fn register() -> Vec<ExecutionProviderDispatch> {
    let mut ep = CoreML::default()
        .with_compute_units(ComputeUnits::All)
        .with_model_format(ModelFormat::MLProgram)
        .with_specialization_strategy(SpecializationStrategy::FastPrediction)
        .with_static_input_shapes(false);

    match zti_common::paths::models_dir() {
        Ok(dir) => {
            let cache = dir.join("coreml_cache");
            if let Err(e) = std::fs::create_dir_all(&cache) {
                tracing::warn!(error = %e, "coreml cache dir create failed; running without cache");
            } else if let Some(path) = cache.to_str() {
                ep = ep.with_model_cache_dir(path);
                tracing::debug!(cache = path, "coreml cache enabled");
            } else {
                tracing::warn!("coreml cache path is not valid UTF-8; running without cache");
            }
        }
        Err(e) => tracing::warn!(error = %e, "models_dir() failed; coreml cache disabled"),
    }

    tracing::debug!("configuring CoreML execution provider (ALL compute units, MLProgram)");
    let mut out = Vec::with_capacity(1);
    out.push(ep.build());
    out
}
