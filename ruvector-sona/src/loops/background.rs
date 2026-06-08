//! Loop B - Background Learning
//!
//! Hourly pattern extraction and base LoRA updates.

use crate::ewc::EwcPlusPlus;
use crate::lora::BaseLoRA;
use crate::reasoning_bank::ReasoningBank;
use crate::time_compat::Instant;
use crate::types::{QueryTrajectory, SonaConfig};
use parking_lot::RwLock;
use std::sync::Arc;
use std::time::Duration;

/// Background loop configuration
#[derive(Clone, Debug)]
pub struct BackgroundLoopConfig {
    /// Minimum trajectories to process
    pub min_trajectories: usize,
    /// Base LoRA learning rate
    pub base_lora_lr: f32,
    /// EWC lambda
    pub ewc_lambda: f32,
    /// Pattern extraction interval
    pub extraction_interval: Duration,
}

impl Default for BackgroundLoopConfig {
    fn default() -> Self {
        Self {
            min_trajectories: 10, // Was 100; lowered so patterns crystallize from fewer trajectories
            base_lora_lr: 0.0001,
            ewc_lambda: 1000.0,
            extraction_interval: Duration::from_secs(3600),
        }
    }
}

impl From<&SonaConfig> for BackgroundLoopConfig {
    fn from(config: &SonaConfig) -> Self {
        Self {
            min_trajectories: 10, // Was 100; lowered so patterns crystallize from fewer trajectories
            base_lora_lr: config.base_lora_lr,
            ewc_lambda: config.ewc_lambda,
            extraction_interval: Duration::from_millis(config.background_interval_ms),
        }
    }
}

/// Background cycle result
#[derive(Debug)]
pub struct BackgroundResult {
    pub trajectories_processed: usize,
    pub patterns_extracted: usize,
    pub ewc_updated: bool,
    pub elapsed: Duration,
    pub status: String,
}

impl BackgroundResult {
    fn skipped(reason: &str) -> Self {
        Self {
            trajectories_processed: 0,
            patterns_extracted: 0,
            ewc_updated: false,
            elapsed: Duration::ZERO,
            status: format!("skipped: {}", reason),
        }
    }
}

/// Background learning loop (Loop B)
pub struct BackgroundLoop {
    /// Configuration
    config: BackgroundLoopConfig,
    /// ReasoningBank for pattern storage
    reasoning_bank: Arc<RwLock<ReasoningBank>>,
    /// EWC++ for forgetting prevention
    ewc: Arc<RwLock<EwcPlusPlus>>,
    /// Base LoRA
    base_lora: Arc<RwLock<BaseLoRA>>,
    /// Last extraction time
    last_extraction: RwLock<Instant>,
}

impl BackgroundLoop {
    /// Create new background loop
    pub fn new(
        config: BackgroundLoopConfig,
        reasoning_bank: Arc<RwLock<ReasoningBank>>,
        ewc: Arc<RwLock<EwcPlusPlus>>,
        base_lora: Arc<RwLock<BaseLoRA>>,
    ) -> Self {
        Self {
            config,
            reasoning_bank,
            ewc,
            base_lora,
            last_extraction: RwLock::new(Instant::now()),
        }
    }

    /// Check if it's time for background cycle
    pub fn should_run(&self) -> bool {
        self.last_extraction.read().elapsed() >= self.config.extraction_interval
    }

    /// Run background learning cycle
    ///
    /// If `force` is true, bypasses the minimum trajectory check (for forceLearn API)
    pub fn run_cycle(&self, trajectories: Vec<QueryTrajectory>, force: bool) -> BackgroundResult {
        if !force && trajectories.len() < self.config.min_trajectories {
            return BackgroundResult::skipped(&format!(
                "insufficient trajectories ({} < {} minimum, use forceLearn to bypass)",
                trajectories.len(),
                self.config.min_trajectories
            ));
        }

        if trajectories.is_empty() {
            return BackgroundResult::skipped("no trajectories to process");
        }

        let start = Instant::now();

        // 1. Add trajectories to reasoning bank
        {
            let mut bank = self.reasoning_bank.write();
            for trajectory in &trajectories {
                bank.add_trajectory(trajectory);
            }
        }

        // 2. Extract patterns
        let patterns = {
            let mut bank = self.reasoning_bank.write();
            bank.extract_patterns()
        };

        // 3. Compute gradients from trajectories
        let gradients = self.compute_trajectory_gradients(&trajectories);

        // 4. Apply EWC++ constraints
        let constrained_gradients = {
            let ewc = self.ewc.read();
            ewc.apply_constraints(&gradients)
        };

        // 5. Check for task boundary
        let task_boundary = {
            let ewc = self.ewc.read();
            ewc.detect_task_boundary(&gradients)
        };

        if task_boundary {
            let mut ewc = self.ewc.write();
            ewc.start_new_task();
        }

        // 6. Update EWC++ Fisher
        {
            let mut ewc = self.ewc.write();
            ewc.update_fisher(&constrained_gradients);
        }

        // 7. Update base LoRA
        self.update_base_lora(&constrained_gradients);

        // Update last extraction time
        *self.last_extraction.write() = Instant::now();

        BackgroundResult {
            trajectories_processed: trajectories.len(),
            patterns_extracted: patterns.len(),
            ewc_updated: true,
            elapsed: start.elapsed(),
            status: "completed".to_string(),
        }
    }

    #[allow(clippy::needless_range_loop)]
    fn compute_trajectory_gradients(&self, trajectories: &[QueryTrajectory]) -> Vec<f32> {
        let lora = self.base_lora.read();
        let hidden_dim = lora.hidden_dim;
        let rank = lora.rank;
        let scale = lora.alpha / rank as f32;

        let total_params = lora.param_count();
        if total_params == 0 || trajectories.is_empty() {
            return vec![0.0; total_params];
        }

        let mut global_gradients = vec![0.0f32; total_params];
        let mut total_weight = 0.0f32;

        for trajectory in trajectories {
            if trajectory.steps.is_empty() {
                continue;
            }

            // Compute trajectory baseline reward
            let baseline = trajectory.steps.iter().map(|s| s.reward).sum::<f32>()
                / trajectory.steps.len() as f32;

            for step in &trajectory.steps {
                let advantage = step.reward - baseline;
                let query = &step.activations;
                let target = &step.attention_weights;

                if query.len() != hidden_dim || target.len() != hidden_dim {
                    continue;
                }

                // Desired change in query: target - query
                let grad_out: Vec<f32> = query
                    .iter()
                    .zip(target.iter())
                    .map(|(&q, &t)| advantage * (t - q))
                    .collect();

                let per_layer_params = hidden_dim * rank * 2;

                // Backpropagate to all layers in BaseLoRA
                for (layer_idx, layer) in lora.layers.iter().enumerate() {
                    let start = layer_idx * per_layer_params;

                    // 1. Compute intermediate activation: inter = query @ down
                    let mut inter = vec![0.0f32; rank];
                    for r in 0..rank {
                        let offset = r * hidden_dim;
                        let mut sum = 0.0f32;
                        for i in 0..hidden_dim {
                            sum += query[i] * layer.down_proj[offset + i];
                        }
                        inter[r] = sum;
                    }

                    // 2. Compute up_proj gradient: d(loss)/d(W_up) = grad_out * inter * scale
                    let down_proj_size = hidden_dim * rank;
                    for r in 0..rank {
                        for i in 0..hidden_dim {
                            let grad_idx = start + down_proj_size + (r * hidden_dim + i);
                            global_gradients[grad_idx] += grad_out[i] * inter[r] * scale;
                        }
                    }

                    // 3. Compute down_proj gradient: d(loss)/d(W_down) = (grad_out @ W_up) * query
                    for r in 0..rank {
                        let mut err_inter_r = 0.0f32;
                        for i in 0..hidden_dim {
                            err_inter_r += grad_out[i] * layer.up_proj[r * hidden_dim + i] * scale;
                        }

                        let offset = r * hidden_dim;
                        for i in 0..hidden_dim {
                            let grad_idx = start + (offset + i);
                            global_gradients[grad_idx] += err_inter_r * query[i];
                        }
                    }
                }
                total_weight += 1.0;
            }
        }

        if total_weight > 0.0 {
            for g in &mut global_gradients {
                *g /= total_weight;
            }
        }

        global_gradients
    }

    fn update_base_lora(&self, gradients: &[f32]) {
        let mut lora = self.base_lora.write();
        let num_layers = lora.num_layers();

        if num_layers == 0 || gradients.is_empty() {
            return;
        }

        let per_layer = gradients.len() / num_layers;
        let hidden_dim = lora.hidden_dim;
        let rank = lora.rank;
        let expected_per_layer = hidden_dim * rank * 2;

        if per_layer != expected_per_layer {
            return;
        }

        let down_size = hidden_dim * rank;
        let up_size = rank * hidden_dim;

        for (layer_idx, layer) in lora.layers.iter_mut().enumerate() {
            let start = layer_idx * per_layer;

            // 1. Update down_proj
            for i in 0..down_size {
                if start + i < gradients.len() {
                    layer.down_proj[i] += gradients[start + i] * self.config.base_lora_lr;
                }
            }

            // 2. Update up_proj
            for i in 0..up_size {
                if start + down_size + i < gradients.len() {
                    layer.up_proj[i] += gradients[start + down_size + i] * self.config.base_lora_lr;
                }
            }
        }
    }

    /// Get reasoning bank reference
    pub fn reasoning_bank(&self) -> &Arc<RwLock<ReasoningBank>> {
        &self.reasoning_bank
    }

    /// Get EWC reference
    pub fn ewc(&self) -> &Arc<RwLock<EwcPlusPlus>> {
        &self.ewc
    }

    /// Get base LoRA reference
    pub fn base_lora(&self) -> &Arc<RwLock<BaseLoRA>> {
        &self.base_lora
    }
}
