use super::*;

#[test]
fn default_config_uses_documented_starting_values() {
    let config = JobSystemConfig::default();

    assert_eq!(config.max_latent_jobs, 32 * 1024);
    assert_eq!(config.max_critical_path_jobs, 16 * 1024);
    assert_eq!(config.max_immediate_jobs, 2 * 1024);
    assert_eq!(config.worker_thread_stack_size, Some(1024 * 1024));
    assert!(config.max_threads >= 1);
    assert!(!config.all_jobs_critical_path);
    assert!(!config.use_debugger);
}

#[test]
fn editor_preset_expands_normal_queue_headroom() {
    let default = JobSystemConfig::default();
    let editor = JobSystemConfig::editor();

    assert_eq!(
        editor.max_latent_jobs,
        default.max_latent_jobs * EDITOR_QUEUE_MULTIPLIER
    );
    assert_eq!(
        editor.max_critical_path_jobs,
        default.max_critical_path_jobs * EDITOR_QUEUE_MULTIPLIER
    );
    assert_eq!(
        editor.max_immediate_jobs,
        default.max_immediate_jobs * EDITOR_QUEUE_MULTIPLIER
    );
    assert!(!editor.all_jobs_critical_path);
}

#[test]
fn tool_preset_collapses_work_to_critical_path() {
    let tool = JobSystemConfig::tool();

    assert_eq!(tool.max_latent_jobs, TOOL_MAX_LATENT_JOBS);
    assert_eq!(tool.max_critical_path_jobs, TOOL_MAX_CRITICAL_PATH_JOBS);
    assert_eq!(tool.max_immediate_jobs, TOOL_MAX_IMMEDIATE_JOBS);
    assert!(tool.all_jobs_critical_path);
}

#[test]
fn worker_count_resolution_treats_max_threads_as_a_cap() {
    assert_eq!(hardware_worker_count(1), 1);
    assert_eq!(hardware_worker_count(8), 7);

    assert_eq!(resolve_worker_thread_count(4, 7), 4);
    assert_eq!(resolve_worker_thread_count(64, 7), 7);
    assert_eq!(resolve_worker_thread_count(64, 0), 1);
}

#[test]
#[should_panic(expected = "job system must start at least one worker thread")]
fn zero_worker_cap_panics() {
    let _ = resolve_worker_thread_count(0, 8);
}
