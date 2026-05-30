use super::extract_windows;
use crate::{ExtractSchedule, MainWorld, RenderWindowRegistry};
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
    render_world.init_resource::<RenderWindowRegistry>();
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
        let render_windows = render_world.resource::<RenderWindowRegistry>();
        let render_window = render_windows
            .get(&window_entity)
            .expect("window should be extracted");
        assert_eq!(render_windows.primary, Some(window_entity));
        assert_eq!(render_window.physical_width, 640);
        assert_eq!(render_window.physical_height, 360);
        assert!(!render_window.size_changed);
        assert!(!render_window.present_mode_changed);
        assert!(!render_window.handle_changed);
        assert!(!render_window.needs_surface_reconfigure);
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

    let render_windows = render_world.resource::<RenderWindowRegistry>();
    let render_window = render_windows
        .get(&window_entity)
        .expect("window should still be extracted");
    assert_eq!(render_window.physical_width, 800);
    assert_eq!(render_window.physical_height, 600);
    assert!(render_window.size_changed);
    assert!(render_window.present_mode_changed);
    assert!(render_window.needs_surface_reconfigure);
}

#[test]
fn removes_stale_windows_and_updates_primary_window() {
    let mut main_world = World::default();
    let first_window = spawn_window(&mut main_world, 1, 640, 360, PresentMode::Fifo, true);
    let second_window = spawn_window(&mut main_world, 2, 320, 180, PresentMode::Fifo, false);

    let (mut render_world, mut schedule) = run_extract(main_world);
    assert_eq!(
        render_world.resource::<RenderWindowRegistry>().primary,
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
        render_world.resource::<RenderWindowRegistry>().primary,
        Some(second_window)
    );

    {
        let mut main_world = render_world.resource_mut::<MainWorld>();
        main_world.despawn(first_window);
    }
    schedule.run(&mut render_world);

    let render_windows = render_world.resource::<RenderWindowRegistry>();
    assert!(!render_windows.contains_key(&first_window));
    assert!(render_windows.contains_key(&second_window));
    assert_eq!(render_windows.primary, Some(second_window));
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

    let render_windows = render_world.resource::<RenderWindowRegistry>();
    let render_window = render_windows
        .get(&window_entity)
        .expect("window should still be extracted");
    assert!(render_window.handle_changed);
    assert!(render_window.needs_surface_rebuild);
}
