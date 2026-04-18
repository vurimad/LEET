# LEET Engine - Getting Started

## Overview

LEET supports two ways to create games:

1. **Managed Mode** - Engine handles main(), perfect for beginners and future editor integration
2. **Manual Mode** - You control main(), perfect for advanced users who need full control

## Managed Mode (Recommended)

The engine generates the `main()` function for you. Just use the `#[leet_main]` macro:

### Example

```rust
use leet_app::leet_main;
use leet_app::prelude::*;

struct MySystem;

impl System for MySystem {
    fn run(&mut self, _world: &mut World) {
        println!("System running!");
    }
}

#[leet_main]
fn game_setup(app: &mut App) {
    println!("Setting up game...");
    
    app.add_system(MySystem);
}
```

### To Run

```bash
cargo run
```

The macro will:
1. Generate a `main()` function
2. Create and initialize the App
3. Call your `game_setup` function
4. Run the game loop

## Manual Mode (Advanced)

You write your own `main()` function for full control:

### Example

```rust
use leet_app::prelude::*;

struct MySystem;

impl System for MySystem {
    fn run(&mut self, _world: &mut World) {
        println!("System running!");
    }
}

fn main () {
    println!("I have full control!");
    
    let mut app = App::with_config(AppConfig {
        title: "My Game".to_string(),
        width: 1920,
        height: 1080,
    });
    
    app.add_system(MySystem);
    
    app.run();
    
    println!("Game ended, doing cleanup...");
}
```

### To Run

```bash
cargo run
```

## Which Mode Should I Use?

### Use **Managed Mode** if:
- You're new to game development
- You want the simplest setup
- You plan to use the LEET editor in the future
- You want hot-reloading support (coming soon)

### Use **Manual Mode** if:
- You need full control over initialization
- You're integrating with other libraries
- You want custom startup/shutdown logic
- You're building tools/utilities with the engine

## Project Structure

### Managed Mode Project

```
my_game/
├── Cargo.toml
└── src/
    └── main.rs    # Contains #[leet_main] function
```

### Manual Mode Project

```
my_game/
├── Cargo.toml
└── src/
    └── main.rs    # Contains main() function
```

## Next Steps

- Check out the [examples](../examples/) directory
- Read the [Architecture Documentation](ARCHITECTURE.md)
- Join the community (links TBD)

## Running the Examples

```bash
# Manual mode example
cargo run --package example_manual

# Managed mode example
cargo run --package example_managed
```
