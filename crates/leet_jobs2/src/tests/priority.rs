use super::*;

#[test]
fn priority_order_matches_dispatcher_pop_order() {
    assert!(Priority::Immediate > Priority::CriticalPath);
    assert!(Priority::CriticalPath > Priority::RenderPath);
    assert!(Priority::RenderPath > Priority::Latent);
}

#[test]
fn schedule_param_defaults_to_critical_path() {
    assert_eq!(ScheduleParam::default().priority, Priority::CriticalPath);
}
