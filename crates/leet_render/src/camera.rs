use crate::{
    sync_render_camera_storage,
    window::{extract_windows, ExtractedWindows},
    Extract, ExtractSchedule, ManualTextureViewPlugin, ManualTextureViews, Render, RenderApp,
    RenderCameraStorage, RenderSystems,
};
use bevy_app::{App, Plugin, PostStartup, PostUpdate};
use bevy_asset::Assets;
use bevy_camera::{
    Camera, CameraOutputMode, CameraUpdateSystems, ClearColorConfig, CompositingSpace, Exposure,
    Hdr, MsaaWriteback, NormalizedRenderTarget, Projection, RenderTarget, RenderTargetInfo,
    Viewport,
};
use bevy_ecs::{
    component::Component,
    entity::{ContainsEntity, Entity, EntityHashMap, EntityHashSet},
    prelude::{Query, Res, ResMut, Resource, With},
    query::Has,
    schedule::{InternedScheduleLabel, IntoScheduleConfigs, ScheduleLabel},
};
use bevy_image::Image;
use bevy_math::{Mat4, URect, UVec2, UVec4};
use bevy_transform::{components::GlobalTransform, TransformPlugin, TransformSystems};
use bevy_window::PrimaryWindow;
use std::{
    collections::HashMap,
    ops::{Deref, DerefMut},
};
use wgpu::TextureFormat;

#[derive(Component, Debug, Clone)]
pub struct CameraRenderGraph(pub InternedScheduleLabel);

impl Default for CameraRenderGraph {
    fn default() -> Self {
        Self(Render.intern())
    }
}

impl CameraRenderGraph {
    pub fn new<T: ScheduleLabel>(schedule: T) -> Self {
        Self(schedule.intern())
    }
}

impl Deref for CameraRenderGraph {
    type Target = InternedScheduleLabel;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for CameraRenderGraph {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

#[derive(Debug, Clone)]
pub struct ExtractedCamera {
    pub target: Option<NormalizedRenderTarget>,
    pub physical_viewport_size: Option<UVec2>,
    pub physical_target_size: Option<UVec2>,
    pub viewport: Option<Viewport>,
    pub schedule: InternedScheduleLabel,
    pub order: isize,
    pub output_mode: CameraOutputMode,
    pub msaa_writeback: MsaaWriteback,
    pub clear_color: ClearColorConfig,
    pub sorted_camera_index_for_target: usize,
    pub exposure: f32,
    pub hdr: bool,
    pub compositing_space: Option<CompositingSpace>,
}

#[derive(Debug, Clone)]
pub struct ExtractedView {
    pub clip_from_view: Mat4,
    pub world_from_view: GlobalTransform,
    pub target_format: TextureFormat,
    pub viewport: UVec4,
    pub invert_culling: bool,
}

#[derive(Default, Resource)]
pub struct CameraMainPassTextureFormats(pub EntityHashMap<TextureFormat>);

impl Deref for CameraMainPassTextureFormats {
    type Target = EntityHashMap<TextureFormat>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for CameraMainPassTextureFormats {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

#[derive(Default, Resource)]
pub struct ExtractedCameras(pub EntityHashMap<ExtractedCamera>);

impl Deref for ExtractedCameras {
    type Target = EntityHashMap<ExtractedCamera>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for ExtractedCameras {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

#[derive(Default, Resource)]
pub struct ExtractedViews(pub EntityHashMap<ExtractedView>);

impl Deref for ExtractedViews {
    type Target = EntityHashMap<ExtractedView>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for ExtractedViews {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

#[derive(Resource, Default)]
pub struct SortedCameras(pub Vec<SortedCamera>);

pub struct SortedCamera {
    pub entity: Entity,
    pub order: isize,
    pub target: Option<NormalizedRenderTarget>,
    pub hdr: bool,
    pub output_mode: CameraOutputMode,
}

pub struct CameraPlugin;

impl Plugin for CameraPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<TransformPlugin>() {
            app.add_plugins(TransformPlugin);
        }
        if !app.is_plugin_added::<ManualTextureViewPlugin>() {
            app.add_plugins(ManualTextureViewPlugin);
        }

        app.add_systems(PostStartup, update_cameras.in_set(CameraUpdateSystems))
            .add_systems(
                PostUpdate,
                update_cameras
                    .in_set(CameraUpdateSystems)
                    .after(TransformSystems::Propagate),
            );

        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app
                .init_resource::<CameraMainPassTextureFormats>()
                .init_resource::<ExtractedCameras>()
                .init_resource::<ExtractedViews>()
                .init_resource::<SortedCameras>()
                .init_resource::<RenderCameraStorage>()
                .add_systems(ExtractSchedule, extract_cameras.after(extract_windows))
                .add_systems(
                    Render,
                    (sort_cameras, sync_render_camera_storage)
                        .chain()
                        .in_set(RenderSystems::Prepare),
                );
        }
    }
}

fn update_cameras(
    primary_window: Query<Entity, With<PrimaryWindow>>,
    windows: Query<(Entity, &bevy_window::Window)>,
    images: Option<Res<Assets<Image>>>,
    manual_texture_views: Res<ManualTextureViews>,
    mut cameras: Query<(&mut Camera, &RenderTarget, &mut Projection)>,
) {
    let primary_window = primary_window.iter().next();

    // This is an explicit full scan over the main-world camera set each run.
    // Camera counts are tiny, and this avoids coupling LEET's custom renderer
    // foundation to Bevy's window/image message bootstrap.
    for (mut camera, render_target, mut projection) in &mut cameras {
        let mut viewport_size = camera
            .viewport
            .as_ref()
            .map(|viewport| viewport.physical_size);

        let Some(normalized_target) = render_target.normalize(primary_window) else {
            camera.computed.target_info = None;
            camera.computed.old_viewport_size = viewport_size;
            camera.computed.old_sub_camera_view = camera.sub_camera_view;
            continue;
        };

        let Some(new_target_info) = get_render_target_info(
            &normalized_target,
            &windows,
            images.as_deref(),
            &manual_texture_views,
        ) else {
            camera.computed.target_info = None;
            camera.computed.old_viewport_size = viewport_size;
            camera.computed.old_sub_camera_view = camera.sub_camera_view;
            continue;
        };

        if let Some(old_scale_factor) = camera
            .computed
            .target_info
            .as_ref()
            .map(|info| info.scale_factor)
        {
            if old_scale_factor != new_target_info.scale_factor {
                let resize_factor = new_target_info.scale_factor / old_scale_factor;
                if let Some(viewport) = &mut camera.viewport {
                    let resize = |vec: UVec2| (vec.as_vec2() * resize_factor).as_uvec2();
                    viewport.physical_position = resize(viewport.physical_position);
                    viewport.physical_size = resize(viewport.physical_size);
                    viewport_size = Some(viewport.physical_size);
                }
            }
        }

        if let Some(viewport) = &mut camera.viewport {
            viewport.clamp_to_size(new_target_info.physical_size);
            viewport_size = Some(viewport.physical_size);
        }

        camera.computed.target_info = Some(new_target_info);
        if let Some(size) = camera.logical_viewport_size() {
            if size.x != 0.0 && size.y != 0.0 {
                projection.update(size.x, size.y);
                camera.computed.clip_from_view = match &camera.sub_camera_view {
                    Some(sub_view) => projection.get_clip_from_view_for_sub(sub_view),
                    None => projection.get_clip_from_view(),
                };
            }
        }

        camera.computed.old_viewport_size = viewport_size;
        camera.computed.old_sub_camera_view = camera.sub_camera_view;
    }
}

pub fn extract_cameras(
    mut main_pass_formats: ResMut<CameraMainPassTextureFormats>,
    mut extracted_cameras: ResMut<ExtractedCameras>,
    mut extracted_views: ResMut<ExtractedViews>,
    query: Extract<
        Query<(
            Entity,
            &Camera,
            &RenderTarget,
            &GlobalTransform,
            (Has<Hdr>, Option<&CompositingSpace>, Option<&Exposure>),
            Option<&CameraRenderGraph>,
        )>,
    >,
    primary_window: Extract<Query<Entity, With<PrimaryWindow>>>,
    extracted_windows: Res<ExtractedWindows>,
    images: Extract<Option<Res<Assets<Image>>>>,
    manual_texture_views: Res<ManualTextureViews>,
) {
    main_pass_formats.clear();
    let primary_window = primary_window.iter().next();
    let mut live_cameras = EntityHashSet::default();

    // This explicitly iterates all main-world cameras and snapshots only the data
    // the renderer needs into render-world-owned camera and view tables.
    for (
        main_entity,
        camera,
        render_target,
        transform,
        (hdr, compositing_space, exposure),
        camera_render_graph,
    ) in query.iter()
    {
        live_cameras.insert(main_entity);

        if !camera.is_active {
            extracted_cameras.remove(&main_entity);
            extracted_views.remove(&main_entity);
            main_pass_formats.remove(&main_entity);
            continue;
        }

        let (
            Some(URect {
                min: viewport_origin,
                ..
            }),
            Some(viewport_size),
            Some(target_size),
        ) = (
            camera.physical_viewport_rect(),
            camera.physical_viewport_size(),
            camera.physical_target_size(),
        )
        else {
            extracted_cameras.remove(&main_entity);
            extracted_views.remove(&main_entity);
            main_pass_formats.remove(&main_entity);
            continue;
        };

        if target_size.x == 0 || target_size.y == 0 {
            extracted_cameras.remove(&main_entity);
            extracted_views.remove(&main_entity);
            main_pass_formats.remove(&main_entity);
            continue;
        }

        let target = render_target.normalize(primary_window);
        let output_texture_format = target
            .as_ref()
            .and_then(|target| {
                get_target_texture_view_format(
                    target,
                    &extracted_windows,
                    images.as_deref(),
                    &manual_texture_views,
                )
            })
            .map(|format| normalize_bgra8(format))
            .unwrap_or(TextureFormat::Rgba8UnormSrgb);
        let target_format = if hdr {
            TextureFormat::Rgba16Float
        } else if compositing_space.is_some_and(|space| *space == CompositingSpace::Srgb) {
            TextureFormat::Rgba8Unorm
        } else {
            output_texture_format
        };
        main_pass_formats.insert(main_entity, target_format);

        extracted_cameras.insert(
            main_entity,
            ExtractedCamera {
                target: target.clone(),
                viewport: camera.viewport.clone(),
                physical_viewport_size: Some(viewport_size),
                physical_target_size: Some(target_size),
                schedule: camera_render_graph
                    .map(|camera_render_graph| camera_render_graph.0)
                    .unwrap_or_else(|| Render.intern()),
                order: camera.order,
                output_mode: camera.output_mode,
                msaa_writeback: camera.msaa_writeback,
                clear_color: camera.clear_color.clone(),
                sorted_camera_index_for_target: 0,
                exposure: exposure
                    .map(Exposure::exposure)
                    .unwrap_or_else(|| Exposure::default().exposure()),
                hdr,
                compositing_space: compositing_space.copied(),
            },
        );

        extracted_views.insert(
            main_entity,
            ExtractedView {
                clip_from_view: camera.clip_from_view(),
                world_from_view: *transform,
                target_format,
                viewport: UVec4::new(
                    viewport_origin.x,
                    viewport_origin.y,
                    viewport_size.x,
                    viewport_size.y,
                ),
                invert_culling: camera.invert_culling,
            },
        );
    }

    // This explicitly scans the extracted camera table and removes any camera/view
    // snapshots whose source entity no longer exists in the current main-world camera query.
    let stale_cameras: Vec<_> = extracted_cameras
        .keys()
        .copied()
        .filter(|entity| !live_cameras.contains(entity))
        .collect();
    for stale_camera in stale_cameras {
        extracted_cameras.remove(&stale_camera);
        extracted_views.remove(&stale_camera);
        main_pass_formats.remove(&stale_camera);
    }
}

pub fn sort_cameras(
    mut sorted_cameras: ResMut<SortedCameras>,
    mut extracted_cameras: ResMut<ExtractedCameras>,
) {
    sorted_cameras.0.clear();
    for (entity, camera) in extracted_cameras.iter() {
        sorted_cameras.0.push(SortedCamera {
            entity: *entity,
            order: camera.order,
            target: camera.target.clone(),
            hdr: camera.hdr,
            output_mode: camera.output_mode,
        });
    }

    // This is an explicit sort over all extracted cameras each frame so cameras with the
    // same target are packed together in deterministic order for later graph/view work.
    sorted_cameras
        .0
        .sort_by(|c1, c2| (c1.order, &c1.target).cmp(&(c2.order, &c2.target)));

    let mut target_counts: HashMap<(NormalizedRenderTarget, bool), usize> = HashMap::new();
    for sorted_camera in &sorted_cameras.0 {
        if let Some(target) = &sorted_camera.target {
            let count = target_counts
                .entry((target.clone(), sorted_camera.hdr))
                .or_insert(0usize);
            if let Some(camera) = extracted_cameras.get_mut(&sorted_camera.entity) {
                camera.sorted_camera_index_for_target = *count;
            }
            *count += 1;
        }
    }
}

fn get_target_texture_view_format(
    target: &NormalizedRenderTarget,
    windows: &ExtractedWindows,
    images: Option<&Assets<Image>>,
    manual_texture_views: &ManualTextureViews,
) -> Option<TextureFormat> {
    match target {
        NormalizedRenderTarget::Window(window_ref) => windows
            .get(&window_ref.entity())
            .and_then(|window| window.swap_chain_texture_view_format),
        NormalizedRenderTarget::Image(image_target) => images
            .and_then(|images| images.get(&image_target.handle))
            .map(image_texture_view_format),
        NormalizedRenderTarget::TextureView(id) => {
            manual_texture_views.get(id).map(|view| view.view_format)
        }
        NormalizedRenderTarget::None { .. } => None,
    }
}

fn normalize_bgra8(format: TextureFormat) -> TextureFormat {
    if format == TextureFormat::Bgra8UnormSrgb {
        return TextureFormat::Rgba8UnormSrgb;
    }

    format
}

fn get_render_target_info(
    target: &NormalizedRenderTarget,
    windows: &Query<(Entity, &bevy_window::Window)>,
    images: Option<&Assets<Image>>,
    manual_texture_views: &ManualTextureViews,
) -> Option<RenderTargetInfo> {
    match target {
        NormalizedRenderTarget::Window(window_ref) => windows
            .iter()
            .find(|(entity, _)| *entity == window_ref.entity())
            .map(|(_, window)| RenderTargetInfo {
                physical_size: window.physical_size(),
                scale_factor: window.resolution.scale_factor(),
            }),
        NormalizedRenderTarget::Image(image_target) => images
            .and_then(|images| images.get(&image_target.handle))
            .map(|image| RenderTargetInfo {
                physical_size: image.size(),
                scale_factor: image_target.scale_factor,
            }),
        NormalizedRenderTarget::TextureView(id) => {
            manual_texture_views.get(id).map(|view| RenderTargetInfo {
                physical_size: view.size,
                scale_factor: 1.0,
            })
        }
        NormalizedRenderTarget::None { width, height } => Some(RenderTargetInfo {
            physical_size: UVec2::new(*width, *height),
            scale_factor: 1.0,
        }),
    }
}

fn image_texture_view_format(image: &Image) -> TextureFormat {
    image
        .texture_view_descriptor
        .as_ref()
        .and_then(|descriptor| descriptor.format)
        .unwrap_or(image.texture_descriptor.format)
}

#[cfg(test)]
mod tests {
    use crate::{
        ExtractedCameras, ExtractedViews, ManualTextureView, ManualTextureViews, RenderApp,
        RenderDevice, RenderPlugin,
    };
    use bevy_app::App;
    use bevy_asset::{Assets, RenderAssetUsages};
    use bevy_camera::{
        Camera, ManualTextureViewHandle, NormalizedRenderTarget, Projection, RenderTarget,
        RenderTargetInfo, Viewport,
    };
    use bevy_image::Image;
    use bevy_math::{Mat4, UVec2, UVec4};
    use bevy_transform::components::GlobalTransform;
    use wgpu::{Extent3d, TextureDimension, TextureFormat, TextureUsages};

    fn make_none_target_camera(target_size: UVec2) -> Camera {
        let mut camera = Camera::default();
        camera.computed.target_info = Some(RenderTargetInfo {
            physical_size: target_size,
            scale_factor: 1.0,
        });
        camera.computed.clip_from_view = Mat4::IDENTITY;
        camera
    }

    fn insert_test_image(app: &mut App, size: UVec2, format: TextureFormat) -> RenderTarget {
        if app.world().get_resource::<Assets<Image>>().is_none() {
            app.world_mut().insert_resource(Assets::<Image>::default());
        }

        let image = Image::new_fill(
            Extent3d {
                width: size.x,
                height: size.y,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            &[255, 255, 255, 255],
            format,
            RenderAssetUsages::MAIN_WORLD,
        );

        let handle = app.world_mut().resource_mut::<Assets<Image>>().add(image);
        RenderTarget::from(handle)
    }

    fn insert_manual_texture_view(
        app: &mut App,
        handle: ManualTextureViewHandle,
        size: UVec2,
        format: TextureFormat,
    ) -> RenderTarget {
        let texture_view = {
            let device = app.world().resource::<RenderDevice>();
            let texture = device.0.create_texture(&wgpu::TextureDescriptor {
                label: Some("leet_camera_manual_texture_view_test"),
                size: Extent3d {
                    width: size.x,
                    height: size.y,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: TextureDimension::D2,
                format,
                usage: TextureUsages::TEXTURE_BINDING | TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            });
            texture.create_view(&wgpu::TextureViewDescriptor::default())
        };

        app.world_mut().resource_mut::<ManualTextureViews>().insert(
            handle,
            ManualTextureView {
                texture_view,
                size,
                view_format: format,
            },
        );

        RenderTarget::TextureView(handle)
    }

    #[test]
    fn extracts_none_target_camera_into_render_world() {
        let mut app = App::new();
        app.add_plugins(RenderPlugin);

        let camera_entity = app
            .world_mut()
            .spawn((
                make_none_target_camera(UVec2::new(128, 64)),
                Projection::default(),
                RenderTarget::None {
                    size: UVec2::new(128, 64),
                },
                GlobalTransform::IDENTITY,
            ))
            .id();

        app.update();

        let main_world_clip_from_view = app
            .world()
            .entity(camera_entity)
            .get::<Camera>()
            .expect("main-world camera missing after update")
            .clip_from_view();

        let render_app = app
            .get_sub_app(RenderApp)
            .expect("LEET render sub-app missing");
        let extracted_cameras = render_app.world().resource::<ExtractedCameras>();
        let extracted_views = render_app.world().resource::<ExtractedViews>();

        let extracted_camera = extracted_cameras
            .get(&camera_entity)
            .expect("camera should have been extracted");
        assert_eq!(
            extracted_camera.target,
            Some(NormalizedRenderTarget::None {
                width: 128,
                height: 64,
            })
        );
        assert_eq!(
            extracted_camera.physical_target_size,
            Some(UVec2::new(128, 64))
        );
        assert_eq!(
            extracted_camera.physical_viewport_size,
            Some(UVec2::new(128, 64))
        );
        assert_eq!(extracted_camera.sorted_camera_index_for_target, 0);

        let extracted_view = extracted_views
            .get(&camera_entity)
            .expect("camera view should have been extracted");
        assert_eq!(extracted_view.clip_from_view, main_world_clip_from_view);
        assert_eq!(extracted_view.target_format, TextureFormat::Rgba8UnormSrgb);
        assert_eq!(extracted_view.viewport, UVec4::new(0, 0, 128, 64));
    }

    #[test]
    fn sorts_multiple_cameras_on_same_target_and_assigns_indices() {
        let mut app = App::new();
        app.add_plugins(RenderPlugin);

        let low_order = app
            .world_mut()
            .spawn((
                make_none_target_camera(UVec2::new(128, 64)),
                Projection::default(),
                RenderTarget::None {
                    size: UVec2::new(128, 64),
                },
                GlobalTransform::IDENTITY,
            ))
            .id();
        let high_order = app
            .world_mut()
            .spawn((
                Camera {
                    order: 1,
                    ..make_none_target_camera(UVec2::new(128, 64))
                },
                Projection::default(),
                RenderTarget::None {
                    size: UVec2::new(128, 64),
                },
                GlobalTransform::IDENTITY,
            ))
            .id();

        app.update();

        let render_app = app
            .get_sub_app(RenderApp)
            .expect("LEET render sub-app missing");
        let extracted_cameras = render_app.world().resource::<ExtractedCameras>();

        assert_eq!(
            extracted_cameras
                .get(&low_order)
                .expect("low-order camera missing")
                .sorted_camera_index_for_target,
            0
        );
        assert_eq!(
            extracted_cameras
                .get(&high_order)
                .expect("high-order camera missing")
                .sorted_camera_index_for_target,
            1
        );
    }

    #[test]
    fn removes_inactive_camera_and_respects_custom_viewport() {
        let mut app = App::new();
        app.add_plugins(RenderPlugin);

        let camera_entity = app
            .world_mut()
            .spawn((
                Camera {
                    viewport: Some(Viewport {
                        physical_position: UVec2::new(3, 5),
                        physical_size: UVec2::new(64, 32),
                        ..Default::default()
                    }),
                    ..make_none_target_camera(UVec2::new(128, 64))
                },
                Projection::default(),
                RenderTarget::None {
                    size: UVec2::new(128, 64),
                },
                GlobalTransform::IDENTITY,
            ))
            .id();

        app.update();

        {
            let render_app = app
                .get_sub_app(RenderApp)
                .expect("LEET render sub-app missing");
            let extracted_views = render_app.world().resource::<ExtractedViews>();
            let extracted_view = extracted_views
                .get(&camera_entity)
                .expect("viewport camera should have been extracted");
            assert_eq!(extracted_view.viewport, UVec4::new(3, 5, 64, 32));
        }

        app.world_mut()
            .entity_mut(camera_entity)
            .get_mut::<Camera>()
            .expect("main-world camera missing")
            .is_active = false;

        app.update();

        let render_app = app
            .get_sub_app(RenderApp)
            .expect("LEET render sub-app missing");
        let extracted_cameras = render_app.world().resource::<ExtractedCameras>();
        let extracted_views = render_app.world().resource::<ExtractedViews>();
        assert!(!extracted_cameras.contains_key(&camera_entity));
        assert!(!extracted_views.contains_key(&camera_entity));
    }

    #[test]
    fn updates_and_extracts_image_target_camera() {
        let mut app = App::new();
        app.add_plugins(RenderPlugin);

        let render_target =
            insert_test_image(&mut app, UVec2::new(96, 48), TextureFormat::Bgra8UnormSrgb);
        let expected_target = render_target.normalize(None);
        let camera_entity = app
            .world_mut()
            .spawn((
                Camera::default(),
                Projection::default(),
                render_target,
                GlobalTransform::IDENTITY,
            ))
            .id();

        app.update();

        let main_world_camera = app
            .world()
            .entity(camera_entity)
            .get::<Camera>()
            .expect("image-target camera missing from main world");
        let main_world_target_info = main_world_camera
            .computed
            .target_info
            .as_ref()
            .expect("image-target camera should have computed target info");
        assert_eq!(main_world_target_info.physical_size, UVec2::new(96, 48));
        assert_eq!(main_world_target_info.scale_factor, 1.0);

        let render_app = app
            .get_sub_app(RenderApp)
            .expect("LEET render sub-app missing");
        let extracted_cameras = render_app.world().resource::<ExtractedCameras>();
        let extracted_views = render_app.world().resource::<ExtractedViews>();

        let extracted_camera = extracted_cameras
            .get(&camera_entity)
            .expect("image-target camera should have been extracted");
        assert_eq!(extracted_camera.target, expected_target);
        assert_eq!(
            extracted_camera.physical_target_size,
            Some(UVec2::new(96, 48))
        );
        assert_eq!(
            extracted_camera.physical_viewport_size,
            Some(UVec2::new(96, 48))
        );

        let extracted_view = extracted_views
            .get(&camera_entity)
            .expect("image-target view should have been extracted");
        assert_eq!(extracted_view.target_format, TextureFormat::Rgba8UnormSrgb);
        assert_eq!(extracted_view.viewport, UVec4::new(0, 0, 96, 48));
    }

    #[test]
    fn updates_and_extracts_manual_texture_view_target_camera() {
        let mut app = App::new();
        app.add_plugins(RenderPlugin);

        let handle = ManualTextureViewHandle(7);
        let render_target = insert_manual_texture_view(
            &mut app,
            handle,
            UVec2::new(80, 40),
            TextureFormat::Rgba16Float,
        );
        let expected_target = render_target.normalize(None);
        let camera_entity = app
            .world_mut()
            .spawn((
                Camera::default(),
                Projection::default(),
                render_target,
                GlobalTransform::IDENTITY,
            ))
            .id();

        app.update();

        let main_world_camera = app
            .world()
            .entity(camera_entity)
            .get::<Camera>()
            .expect("manual-texture-view camera missing from main world");
        let main_world_target_info = main_world_camera
            .computed
            .target_info
            .as_ref()
            .expect("manual-texture-view camera should have computed target info");
        assert_eq!(main_world_target_info.physical_size, UVec2::new(80, 40));
        assert_eq!(main_world_target_info.scale_factor, 1.0);

        let render_app = app
            .get_sub_app(RenderApp)
            .expect("LEET render sub-app missing");
        let extracted_manual_texture_views = render_app.world().resource::<ManualTextureViews>();
        assert!(
            extracted_manual_texture_views.contains_key(&handle),
            "manual texture views should be extracted into the render world"
        );

        let extracted_cameras = render_app.world().resource::<ExtractedCameras>();
        let extracted_views = render_app.world().resource::<ExtractedViews>();
        let extracted_camera = extracted_cameras
            .get(&camera_entity)
            .expect("manual-texture-view camera should have been extracted");
        assert_eq!(extracted_camera.target, expected_target);
        assert_eq!(
            extracted_camera.physical_target_size,
            Some(UVec2::new(80, 40))
        );
        assert_eq!(
            extracted_camera.physical_viewport_size,
            Some(UVec2::new(80, 40))
        );

        let extracted_view = extracted_views
            .get(&camera_entity)
            .expect("manual-texture-view view should have been extracted");
        assert_eq!(extracted_view.target_format, TextureFormat::Rgba16Float);
        assert_eq!(extracted_view.viewport, UVec4::new(0, 0, 80, 40));
    }
}
