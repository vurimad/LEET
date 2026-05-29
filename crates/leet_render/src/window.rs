use crate::{Extract, ExtractSchedule, RenderApp};
use bevy_app::{App, Plugin};
use bevy_ecs::{
    entity::{Entity, EntityHashMap, EntityHashSet},
    prelude::{Query, ResMut, Resource},
};
use bevy_window::{CompositeAlphaMode, PresentMode, PrimaryWindow, RawHandleWrapper, Window};
use std::{
    num::NonZero,
    ops::{Deref, DerefMut},
};
use wgpu::{TextureFormat, TextureView, TextureViewDescriptor};

/// Render-world snapshot of a window entity.
pub struct ExtractedWindow {
    pub entity: Entity,
    pub handle: RawHandleWrapper,
    pub physical_width: u32,
    pub physical_height: u32,
    pub present_mode: PresentMode,
    pub desired_maximum_frame_latency: Option<NonZero<u32>>,
    pub alpha_mode: CompositeAlphaMode,
    pub swap_chain_texture_view: Option<TextureView>,
    pub swap_chain_texture: Option<wgpu::SurfaceTexture>,
    pub swap_chain_texture_format: Option<TextureFormat>,
    pub swap_chain_texture_view_format: Option<TextureFormat>,
    pub handle_changed: bool,
    pub size_changed: bool,
    pub present_mode_changed: bool,
    pub needs_surface_reconfigure: bool,
    pub needs_surface_rebuild: bool,
    pub needs_initial_present: bool,
}

impl ExtractedWindow {
    pub fn set_swapchain_texture(&mut self, frame: wgpu::SurfaceTexture) {
        self.swap_chain_texture_view_format = Some(frame.texture.format().add_srgb_suffix());
        let texture_view_descriptor = TextureViewDescriptor {
            format: self.swap_chain_texture_view_format,
            ..Default::default()
        };
        self.swap_chain_texture_view = Some(frame.texture.create_view(&texture_view_descriptor));
        self.swap_chain_texture = Some(frame);
    }

    pub fn has_swapchain_texture(&self) -> bool {
        self.swap_chain_texture_view.is_some() && self.swap_chain_texture.is_some()
    }

    pub fn clear_swapchain_texture(&mut self) {
        self.swap_chain_texture_view = None;
        self.swap_chain_texture = None;
    }

    pub fn present(&mut self) {
        self.swap_chain_texture_view = None;
        if let Some(surface_texture) = self.swap_chain_texture.take() {
            surface_texture.present();
        }
    }
}

/// Render-world table of the windows that currently exist in the main world.
#[derive(Default, Resource)]
pub struct ExtractedWindows {
    pub primary: Option<Entity>,
    pub windows: EntityHashMap<ExtractedWindow>,
}

impl Deref for ExtractedWindows {
    type Target = EntityHashMap<ExtractedWindow>;

    fn deref(&self) -> &Self::Target {
        &self.windows
    }
}

impl DerefMut for ExtractedWindows {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.windows
    }
}

/// Registers window extraction into the LEET render app.
pub struct WindowRenderPlugin;

impl Plugin for WindowRenderPlugin {
    fn build(&self, app: &mut App) {
        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app
                .init_resource::<ExtractedWindows>()
                .add_systems(ExtractSchedule, extract_windows);
        }
    }
}

pub(crate) fn extract_windows(
    mut extracted_windows: ResMut<ExtractedWindows>,
    windows: Extract<Query<(Entity, &Window, &RawHandleWrapper, Option<&PrimaryWindow>)>>,
) {
    extracted_windows.primary = None;
    let mut live_windows = EntityHashSet::default();

    // Extraction explicitly iterates all current windows to snapshot the main-world
    // window state into render-world-owned storage.
    for (entity, window, handle, primary) in windows.iter() {
        live_windows.insert(entity);

        if primary.is_some() {
            extracted_windows.primary = Some(entity);
        }

        let new_width = window.resolution.physical_width().max(1);
        let new_height = window.resolution.physical_height().max(1);

        let extracted_window = extracted_windows
            .entry(entity)
            .or_insert_with(|| ExtractedWindow {
                entity,
                // This clone is intentional: the render world needs its own owned raw-handle
                // snapshot rather than borrowing the main-world component.
                handle: handle.clone(),
                physical_width: new_width,
                physical_height: new_height,
                present_mode: window.present_mode,
                desired_maximum_frame_latency: window.desired_maximum_frame_latency,
                alpha_mode: window.composite_alpha_mode,
                swap_chain_texture_view: None,
                swap_chain_texture: None,
                swap_chain_texture_format: None,
                swap_chain_texture_view_format: None,
                handle_changed: false,
                size_changed: false,
                present_mode_changed: false,
                needs_surface_reconfigure: false,
                needs_surface_rebuild: false,
                needs_initial_present: true,
            });

        if extracted_window.swap_chain_texture.is_none() {
            extracted_window.swap_chain_texture_view = None;
        }

        let previous_window_handle = extracted_window.handle.get_window_handle();
        let previous_display_handle = extracted_window.handle.get_display_handle();
        let new_window_handle = handle.get_window_handle();
        let new_display_handle = handle.get_display_handle();
        extracted_window.handle_changed = previous_window_handle != new_window_handle
            || previous_display_handle != new_display_handle;

        extracted_window.size_changed = new_width != extracted_window.physical_width
            || new_height != extracted_window.physical_height;
        extracted_window.present_mode_changed =
            window.present_mode != extracted_window.present_mode;

        if extracted_window.handle_changed {
            extracted_window.handle = handle.clone();
            extracted_window.needs_surface_rebuild = true;
        }

        if extracted_window.size_changed {
            extracted_window.physical_width = new_width;
            extracted_window.physical_height = new_height;
        }

        if extracted_window.present_mode_changed {
            extracted_window.present_mode = window.present_mode;
        }

        extracted_window.desired_maximum_frame_latency = window.desired_maximum_frame_latency;
        extracted_window.alpha_mode = window.composite_alpha_mode;
        extracted_window.needs_surface_reconfigure = extracted_window.size_changed
            || extracted_window.present_mode_changed
            || extracted_window.needs_surface_reconfigure;
    }

    // This explicitly scans the extracted window table and removes snapshots whose
    // source window entity no longer exists in the current main-world window query.
    let stale_windows: Vec<_> = extracted_windows
        .keys()
        .copied()
        .filter(|entity| !live_windows.contains(entity))
        .collect();
    for stale_window in stale_windows {
        extracted_windows.remove(&stale_window);
        if extracted_windows.primary == Some(stale_window) {
            extracted_windows.primary = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{extract_windows, ExtractedWindows};
    use crate::{ExtractSchedule, MainWorld};
    use bevy_ecs::prelude::{Entity, Schedule, World};
    use bevy_window::{
        PresentMode, PrimaryWindow, RawHandleWrapper, Window, WindowResolution, WindowWrapper,
    };
    use raw_window_handle::{
        DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, WebDisplayHandle,
        WebWindowHandle, WindowHandle,
    };

    #[derive(Clone)]
    struct FakeWindowHandleSource {
        id: u32,
    }

    impl HasWindowHandle for FakeWindowHandleSource {
        fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
            // SAFETY: The test only needs a deterministic raw handle identity for extraction
            // bookkeeping. It never uses these handles to create actual OS windows here.
            Ok(unsafe { WindowHandle::borrow_raw(WebWindowHandle::new(self.id).into()) })
        }
    }

    impl HasDisplayHandle for FakeWindowHandleSource {
        fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
            // SAFETY: See the matching window-handle implementation above.
            Ok(unsafe { DisplayHandle::borrow_raw(WebDisplayHandle::new().into()) })
        }
    }

    fn make_raw_handle(id: u32) -> RawHandleWrapper {
        let wrapper = WindowWrapper::new(FakeWindowHandleSource { id });
        RawHandleWrapper::new(&wrapper).expect("test raw handle wrapper should build")
    }

    fn make_window(width: u32, height: u32, present_mode: PresentMode) -> Window {
        let mut window = Window::default();
        window.resolution = WindowResolution::new(width, height);
        window.present_mode = present_mode;
        window
    }

    fn spawn_window(
        world: &mut World,
        handle_id: u32,
        width: u32,
        height: u32,
        present_mode: PresentMode,
        primary: bool,
    ) -> Entity {
        let mut entity = world.spawn((
            make_window(width, height, present_mode),
            make_raw_handle(handle_id),
        ));
        if primary {
            entity.insert(PrimaryWindow);
        }
        entity.id()
    }

    fn run_extract(main_world: World) -> (World, Schedule) {
        let mut render_world = World::default();
        render_world.init_resource::<ExtractedWindows>();
        render_world.insert_resource(MainWorld::from_world(main_world));

        let mut schedule = Schedule::new(ExtractSchedule);
        schedule.add_systems(extract_windows);
        schedule.run(&mut render_world);

        (render_world, schedule)
    }

    #[test]
    fn extracts_window_and_tracks_resize_and_present_mode_changes() {
        let mut main_world = World::default();
        let window_entity = spawn_window(&mut main_world, 1, 640, 360, PresentMode::Fifo, true);

        let (mut render_world, mut schedule) = run_extract(main_world);
        {
            let extracted_windows = render_world.resource::<ExtractedWindows>();
            let extracted_window = extracted_windows
                .get(&window_entity)
                .expect("window should be extracted");
            assert_eq!(extracted_windows.primary, Some(window_entity));
            assert_eq!(extracted_window.physical_width, 640);
            assert_eq!(extracted_window.physical_height, 360);
            assert!(!extracted_window.size_changed);
            assert!(!extracted_window.present_mode_changed);
            assert!(!extracted_window.handle_changed);
            assert!(!extracted_window.needs_surface_reconfigure);
        }

        {
            let mut main_world = render_world.resource_mut::<MainWorld>();
            let mut entity = main_world.entity_mut(window_entity);
            let mut window = entity
                .get_mut::<Window>()
                .expect("main-world window missing");
            window.resolution.set_physical_resolution(800, 600);
            window.present_mode = PresentMode::Mailbox;
        }

        schedule.run(&mut render_world);

        let extracted_windows = render_world.resource::<ExtractedWindows>();
        let extracted_window = extracted_windows
            .get(&window_entity)
            .expect("window should still be extracted");
        assert_eq!(extracted_window.physical_width, 800);
        assert_eq!(extracted_window.physical_height, 600);
        assert!(extracted_window.size_changed);
        assert!(extracted_window.present_mode_changed);
        assert!(extracted_window.needs_surface_reconfigure);
    }

    #[test]
    fn removes_stale_windows_and_updates_primary_window() {
        let mut main_world = World::default();
        let first_window = spawn_window(&mut main_world, 1, 640, 360, PresentMode::Fifo, true);
        let second_window = spawn_window(&mut main_world, 2, 320, 180, PresentMode::Fifo, false);

        let (mut render_world, mut schedule) = run_extract(main_world);
        assert_eq!(
            render_world.resource::<ExtractedWindows>().primary,
            Some(first_window)
        );

        {
            let mut main_world = render_world.resource_mut::<MainWorld>();
            main_world
                .entity_mut(first_window)
                .remove::<PrimaryWindow>();
            main_world.entity_mut(second_window).insert(PrimaryWindow);
        }
        schedule.run(&mut render_world);
        assert_eq!(
            render_world.resource::<ExtractedWindows>().primary,
            Some(second_window)
        );

        {
            let mut main_world = render_world.resource_mut::<MainWorld>();
            main_world.despawn(first_window);
        }
        schedule.run(&mut render_world);

        let extracted_windows = render_world.resource::<ExtractedWindows>();
        assert!(!extracted_windows.contains_key(&first_window));
        assert!(extracted_windows.contains_key(&second_window));
        assert_eq!(extracted_windows.primary, Some(second_window));
    }

    #[test]
    fn marks_handle_changes_for_surface_rebuild() {
        let mut main_world = World::default();
        let window_entity = spawn_window(&mut main_world, 1, 640, 360, PresentMode::Fifo, true);

        let (mut render_world, mut schedule) = run_extract(main_world);
        {
            let mut main_world = render_world.resource_mut::<MainWorld>();
            main_world
                .entity_mut(window_entity)
                .insert(make_raw_handle(99));
        }

        schedule.run(&mut render_world);

        let extracted_windows = render_world.resource::<ExtractedWindows>();
        let extracted_window = extracted_windows
            .get(&window_entity)
            .expect("window should still be extracted");
        assert!(extracted_window.handle_changed);
        assert!(extracted_window.needs_surface_rebuild);
    }
}
