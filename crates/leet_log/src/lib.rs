// Re-export so other crates just use leet_log::info! etc.
pub use tracing::{debug, error, info, trace, warn};

#[macro_export]
macro_rules! LeetFatal {
    ($($arg:tt)*) => {{
        let message = ::std::format!($($arg)*);
        $crate::error!("{}", message);
        panic!("{}", message);
    }};
}

pub fn init() {
    let _ = tracing_subscriber::fmt()
        .with_target(false) // don't show module path
        .with_level(true) // show log level
        .try_init();
}
