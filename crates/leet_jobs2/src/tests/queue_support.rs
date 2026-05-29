use super::*;

pub(crate) fn lane_len(queues: &ReadyQueues, priority: Priority) -> usize {
    let state = queues.state.lock().expect("ready-queue lock poisoned");
    match priority {
        Priority::Latent => state.latent.len(),
        Priority::RenderPath => state.render_path.len(),
        Priority::CriticalPath => state.critical_path.len(),
        Priority::Immediate => state.immediate.len(),
    }
}

pub(crate) fn total_len(queues: &ReadyQueues) -> usize {
    let state = queues.state.lock().expect("ready-queue lock poisoned");
    state.latent.len() + state.render_path.len() + state.critical_path.len() + state.immediate.len()
}
