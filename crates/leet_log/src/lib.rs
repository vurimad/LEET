//! LEET logging surface.

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
        .with_target(false)
        .with_level(true)
        .try_init();
}
