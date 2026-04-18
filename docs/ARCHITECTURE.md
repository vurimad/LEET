# LEET Engine - Architecture

## Overview

LEET is designed as a modular game engine with a clear separation of concerns. The engine is split into multiple crates, each responsible for a specific domain.

## Workspace Structure

```
leet/
├── crates/
│   ├── leet_core/      # Foundation: errors, result types, core traits
│   ├── leet_math/      # Math primitives: Vec2, Vec3, matrices
│   ├── leet_ecs/       # Entity Component System
│   ├── leet_macros/    # Procedural macros (#[leet_main])
│   └── leet_app/       # Application lifecycle and framework
├── examples/           # Example games demonstrating features
├── docs/              # Documentation
└── Cargo.toml         # Workspace configuration
```

## Crate Dependencies

```
leet_app
├── leet_core
├── leet_math (→ leet_core)
├── leet_ecs (→ leet_core)
└── leet_macros
```

## Core Concepts

### 1. Entity Component System (ECS)

LEET uses an ECS architecture for game object management:

- **Entities**: Lightweight identifiers (just IDs)
- **Components**: Pure data (position, velocity, health, etc.)
- **Systems**: Logic that operates on entities with specific components

```rust
// Spawn an entity
let player = world.spawn_entity();

// Add components (coming soon)
// world.add_component(player, Position { x: 0.0, y: 0.0 });

// Systems process entities
struct MovementSystem;
impl System for MovementSystem {
    fn run(&mut self, world: &mut World) {
        // Update positions based on velocities
    }
}
```

### 2. Application Lifecycle

```
┌─────────────────┐
│ App::new()      │  ← Create and configure
└────────┬────────┘
         │
┌────────▼────────┐
│ add_system()    │  ← Register systems
└────────┬────────┘
         │
┌────────▼────────┐
│ app.run()       │  ← Enter game loop
└────────┬────────┘
         │
    ┌────▼─────┐
    │ Frame 1  │  → Run all systems
    ├──────────┤
    │ Frame 2  │  → Run all systems
    ├──────────┤
    │ Frame 3  │  → Run all systems
    └────┬─────┘
         │
┌────────▼────────┐
│ Shutdown        │  ← Cleanup
└─────────────────┘
```

### 3. Dual Entry Mode System

LEET supports two entry point modes:

#### Managed Mode (via Macro)

The `#[leet_main]` macro expands to:

```rust
#[leet_main]
fn game_setup(app: &mut App) {
    // Your code
}

// Expands to:

fn game_setup(app: &mut App) {
    // Your code
}

fn main() {
    println!("[LEET Runtime] Starting in managed mode...");
    let mut app = App::new();
    game_setup(&mut app);
    app.run();
}
```

#### Manual Mode

You write `main()` directly - no magic, full control.

## Design Principles

1. **Modularity**: Each crate has a single, well-defined purpose
2. **Pay for What You Use**: Optional features, minimal dependencies
3. **Rust-First**: Leverage Rust's type safety and performance
4. **Beginner-Friendly**: Simple API for common tasks
5. **Advanced-User-Friendly**: Escape hatches for full control
6. **Future-Proof**: Architecture supports future editor integration

## Future Architecture

### Planned Systems

- **leet_renderer**: Graphics rendering (via wgpu)
- **leet_physics**: Physics simulation (via Rapier)
- **leet_audio**: Audio playback and 3D sound
- **leet_input**: Input handling (keyboard, mouse, gamepad)
- **leet_assets**: Asset loading and hot-reloading
- **leet_net**: Networking and multiplayer
- **leet_scripting**: Lua/Rhai scripting support
- **leet_editor**: Visual editor and tooling

### Rendering Architecture (Planned)

```
Game Code
    ↓
leet_renderer (API)
    ↓
wgpu (Graphics abstraction)
    ↓
Vulkan / DirectX 12 / Metal
```

### Plugin System (Planned)

```rust
struct PhysicsPlugin;

impl Plugin for PhysicsPlugin {
    fn build(&self, app: &mut App) {
        app.add_system(physics_system);
    }
}

app.add_plugin(PhysicsPlugin);
```

## Performance Considerations

- **ECS**: Cache-friendly, data-oriented design
- **Parallel Systems**: Systems can run in parallel (future)
- **Compile-Time Optimization**: Rust's zero-cost abstractions
- **Profile-Guided**: Different build profiles for dev/release/distribution

## Contributing

When adding new features, follow these guidelines:

1. New functionality should be in its own crate when possible
2. Keep dependencies minimal
3. Document public APIs thoroughly
4. Write tests for core functionality
5. Follow the existing code style (rustfmt)

## Questions?

- Check the [Getting Started Guide](GETTING_STARTED.md)
- Review the [examples](../examples/)
- Read the inline documentation: `cargo doc --open`
