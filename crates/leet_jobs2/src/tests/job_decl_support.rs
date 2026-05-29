use super::*;
use crate::{
    counter::test_support as counter_test_support,
    dispatcher::test_support as dispatcher_test_support,
};

pub(crate) fn run_context(name: &'static str) -> RunContext {
    RunContext {
        name,
        thread_index: 0,
        parallel_for_index: -1,
        dispatcher: dispatcher_test_support::dispatcher_handle(),
        continuation: ContinuationContext {
            counter: counter_test_support::counter_entry(),
            param: ScheduleParam::default(),
        },
    }
}
