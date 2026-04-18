//! Event-Driven Window Example
//!
//! Demonstrates the `LeetRunLoop` trait and `LeetEventLoop::run_blocking`.
//! The engine loop runs *inside* the winit event loop — no `pump_app_events` hack.

use leet_core::LeetResult;
use leet_display::{
    LeetEvent, LeetEventLoop, LeetRunLoop, LeetWindow, LeetWindowId, WindowConfig, WindowEvent,
};

// =============================================================================
// Custom runner
// =============================================================================

struct GameRunner {
    main_window: Option<LeetWindow>,
    main_window_id: LeetWindowId,
    frame: u64,
    running: bool,
}

impl GameRunner {
    fn new(main_window_id: LeetWindowId) -> Self {
        Self {
            main_window: None,
            main_window_id,
            frame: 0,
            running: true,
        }
    }
}

impl LeetRunLoop for GameRunner {
    // Called once the OS window exists — store the handle for later
    fn on_window_ready(&mut self, id: LeetWindowId, window: LeetWindow) {
        if id == self.main_window_id {
            let (w, h) = window.inner_size();
            println!(
                "Window ready {:?}  {}x{}  scale={:.1}",
                id,
                w,
                h,
                window.scale_factor()
            );
            self.main_window = Some(window);
        }
    }

    fn on_event(&mut self, event: LeetEvent) {
        match event {
            LeetEvent::Window(WindowEvent::CloseRequested { id }) => {
                println!("Close requested ({:?}) — shutting down", id);
                self.running = false;
            }
            LeetEvent::Window(WindowEvent::Resized { id, width, height }) => {
                println!("Resized {:?}  {}x{}", id, width, height);
                // TODO: notify renderer
            }
            LeetEvent::Window(WindowEvent::Focused { id }) => {
                println!("Focused {:?}", id);
            }
            LeetEvent::Window(WindowEvent::Unfocused { id }) => {
                println!("Unfocused {:?}", id);
            }
            LeetEvent::Exit => {
                self.running = false;
            }
            _ => {}
        }
    }

    fn on_begin_frame(&mut self) {
        // e.g. clear input delta state
    }

    fn on_update(&mut self) {
        self.frame += 1;
        if self.frame % 300 == 0 {
            println!("Frame {}", self.frame);
        }
    }

    fn on_end_frame(&mut self) {
        // e.g. submit render work
    }

    fn should_exit(&self) -> bool {
        !self.running
    }
}

// =============================================================================
// main
// =============================================================================

fn main() -> LeetResult<()> {
    leet_log::init();
    println!("=== LEET Event-Driven Example ===\n");

    // 1. Create the event loop
    let mut event_loop = LeetEventLoop::new()?;

    // 2. Queue the main window — we get a stable ID before the OS window exists
    let window_id = event_loop.create_window(WindowConfig {
        title: "LEET Event-Driven".to_string(),
        width: 1024,
        height: 768,
        resizable: true,
    });

    // 3. Build our runner with the window ID it should watch for
    let runner = GameRunner::new(window_id);

    // 4. run_blocking — hands the thread to winit; returns only on exit
    event_loop.run_blocking(runner)?;

    println!("\nApplication exited cleanly.");
    Ok(())
}
