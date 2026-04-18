# LEET Game Engine

A modern, modular game engine written in Rust.

## 🚀 Features

- **Modular Architecture**: Built with multiple crates for flexibility
- **Entity Component System**: Efficient ECS for game object management
- **Dual Entry Modes**: Managed mode for beginners, manual mode for advanced users
- **Modern Rust**: Leveraging Rust's safety and performance

## 📦 Workspace Structure

```
leet/
├── crates/
│   ├── leet/           # Meta crate (re-exports all modules)
│   ├── leet_core/      # Core types and utilities
│   ├── leet_app/       # Application framework
│   ├── leet_math/      # Math primitives
│   ├── leet_ecs/       # Entity Component System
│   └── leet_macros/    # Procedural macros
└── examples/           # Example games and demos
```

## 🎮 Usage

### Managed Mode (Recommended for beginners)

Create your game as a library crate and define a `leet_main` function:

```rust
// main.rs
use leet::prelude::*;

#[leet_main]
fn game_setup(app: &mut App) {
    app.add_system(MyGameSystem);
}
```

### Manual Mode (Full control)

Use LEET as a library and write your own `main()`:

```rust
// main.rs
use leet::prelude::*;

fn main() {
    let mut app = App::new();
    app.add_system(MyGameSystem);
    app.run();
}
```

## 🔧 Building

```bash
# Build all crates
cargo build

# Run tests
cargo test

# Build in release mode
cargo build --release

# Build with distribution profile
cargo build --profile dist
```

## 📖 Documentation

```bash
cargo doc --open
```

## 🤝 Contributing

Contributions are welcome! This is a long-term project.

## 📄 License

Licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.
