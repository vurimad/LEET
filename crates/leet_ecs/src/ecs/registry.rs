//! Global registry that owns and resolves all ECS worlds.

use super::world::World;
use leet_log::{debug, info};
use std::cell::UnsafeCell;
use std::collections::HashMap;
use std::sync::OnceLock;
#[cfg(debug_assertions)]
use std::thread::{self, ThreadId};

struct WorldSlot {
    world: Option<Box<World>>,
}

struct RegistryCell {
    inner: UnsafeCell<WorldRegistry>,
}

impl RegistryCell {
    fn new(registry: WorldRegistry) -> Self {
        Self {
            inner: UnsafeCell::new(registry),
        }
    }

    fn get(&self) -> &WorldRegistry {
        // SAFETY: registry initialization happens once and all external mutable
        // access routes are funneled through the main-thread contract.
        unsafe { &*self.inner.get() }
    }

    fn get_mut(&self) -> &mut WorldRegistry {
        // SAFETY: the engine mutates the registry from the main thread only.
        unsafe { &mut *self.inner.get() }
    }
}

// SAFETY: mutation is restricted to the engine main thread by contract.
unsafe impl Sync for RegistryCell {}

static REGISTRY: OnceLock<RegistryCell> = OnceLock::new();

pub struct WorldRegistry {
    worlds: Vec<WorldSlot>,
    name_to_index: HashMap<&'static str, u32>,
    free_list: Vec<u32>,
    #[cfg(debug_assertions)]
    main_thread_id: ThreadId,
}

impl WorldRegistry {
    /// Returns true when the global world registry has been initialized.
    pub fn is_initialized() -> bool {
        REGISTRY.get().is_some()
    }

    /// Initializes the global world registry once and ignores later calls.
    pub fn init_if_needed() {
        if !Self::is_initialized() {
            Self::init();
        }
    }

    /// Called once at engine startup from the main thread.
    /// Automatically creates the "main" world.
    /// Panics if called more than once.
    pub fn init() {
        #[cfg(debug_assertions)]
        let main_thread_id = thread::current().id();

        let mut registry = Self {
            worlds: Vec::new(),
            name_to_index: HashMap::new(),
            free_list: Vec::new(),
            #[cfg(debug_assertions)]
            main_thread_id,
        };

        #[cfg(debug_assertions)]
        registry.assert_main_thread();

        registry.create_world("main");

        REGISTRY
            .set(RegistryCell::new(registry))
            .unwrap_or_else(|_| {
                leet_log::LeetFatal!("WorldRegistry::init() called more than once")
            });

        info!("[LEET ECS] World registry initialized");
    }

    /// Returns a reference to the global registry.
    /// Panics if called before init().
    pub fn get() -> &'static WorldRegistry {
        REGISTRY
            .get()
            .unwrap_or_else(|| leet_log::LeetFatal!("WorldRegistry::get() called before init()"))
            .get()
    }

    /// Returns a mutable reference to the global registry.
    ///
    /// Callers must uphold the main-thread-only mutation contract.
    pub fn get_mut() -> &'static mut WorldRegistry {
        REGISTRY
            .get()
            .unwrap_or_else(|| {
                leet_log::LeetFatal!("WorldRegistry::get_mut() called before init()")
            })
            .get_mut()
    }

    /// Returns the main simulation world.
    pub fn main_world(&self) -> &World {
        self.world("main")
    }

    /// Returns a mutable reference to the main world.
    /// Only call from the main thread.
    pub fn main_world_mut(&mut self) -> &mut World {
        self.world_mut("main")
    }

    /// Returns a named world by doing a HashMap lookup then indexing.
    /// Panics if not found.
    pub fn world(&self, name: &'static str) -> &World {
        let index = self
            .name_to_index
            .get(name)
            .copied()
            .unwrap_or_else(|| leet_log::LeetFatal!("world `{name}` does not exist"));
        self.world_at(index)
    }

    /// Returns a mutable reference to a named world.
    pub fn world_mut(&mut self, name: &'static str) -> &mut World {
        let index = self
            .name_to_index
            .get(name)
            .copied()
            .unwrap_or_else(|| leet_log::LeetFatal!("world `{name}` does not exist"));
        self.world_at_mut(index)
    }

    /// Returns a world directly by index. Used by Entity methods - O(1), no HashMap.
    /// Panics if slot is empty (world was destroyed).
    pub fn world_at(&self, index: u32) -> &World {
        self.worlds
            .get(index as usize)
            .and_then(|slot| slot.world.as_deref())
            .unwrap_or_else(|| leet_log::LeetFatal!("world slot {index} is empty"))
    }

    /// Mutable version of world_at.
    pub fn world_at_mut(&mut self, index: u32) -> &mut World {
        self.worlds
            .get_mut(index as usize)
            .and_then(|slot| slot.world.as_deref_mut())
            .unwrap_or_else(|| leet_log::LeetFatal!("world slot {index} is empty"))
    }

    /// Creates a named world. Reuses a free slot if available.
    /// Returns the world_index assigned - used when constructing entities for that world.
    /// Call from main thread only.
    pub fn create_world(&mut self, name: &'static str) -> u32 {
        #[cfg(debug_assertions)]
        self.assert_main_thread();

        if self.name_to_index.contains_key(name) {
            leet_log::LeetFatal!("world `{name}` already exists");
        }

        let index = if let Some(index) = self.free_list.pop() {
            let slot = self
                .worlds
                .get_mut(index as usize)
                .unwrap_or_else(|| leet_log::LeetFatal!("free-list world slot {index} is missing"));
            debug_assert!(slot.world.is_none(), "free-list slot {index} was not empty");
            slot.world = Some(Box::new(World::new(index)));
            index
        } else {
            let index = self.worlds.len() as u32;
            self.worlds.push(WorldSlot {
                world: Some(Box::new(World::new(index))),
            });
            index
        };

        self.name_to_index.insert(name, index);
        debug!("[LEET ECS] Created world `{name}` in slot {index}");
        index
    }

    /// Destroys a named world. Slot is returned to the free list.
    /// All entities that belonged to this world are considered dead after this call.
    /// Call from main thread only.
    pub fn destroy_world(&mut self, name: &'static str) {
        #[cfg(debug_assertions)]
        self.assert_main_thread();

        let index = self
            .name_to_index
            .remove(name)
            .unwrap_or_else(|| leet_log::LeetFatal!("world `{name}` does not exist"));

        let slot = self
            .worlds
            .get_mut(index as usize)
            .unwrap_or_else(|| leet_log::LeetFatal!("world slot {index} is missing"));

        if slot.world.take().is_none() {
            leet_log::LeetFatal!("world `{name}` was already destroyed");
        }

        self.free_list.push(index);
        debug!("[LEET ECS] Destroyed world `{name}` from slot {index}");
    }

    #[cfg(debug_assertions)]
    fn assert_main_thread(&self) {
        debug_assert_eq!(
            self.main_thread_id,
            thread::current().id(),
            "WorldRegistry must only be mutated from the main thread",
        );
    }
}
