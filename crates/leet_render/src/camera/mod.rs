mod camera;
mod camera_storage;
mod plugin;

pub use camera::{RenderCamera, RenderCameraFeatures};
pub use camera_storage::{
    sync_render_camera_storage, CameraDependencyFlags, CameraManagement, CameraPrepareContext,
    CameraRenderPolicy, PreparedCameraDependency, PreparedCameraHistory, PreparedFrameCamera,
    PreparedFrameCameraSharedData, PreparedFrameViews, RenderCameraRegistration,
    RenderCameraRegistrationRef, RenderCameraStorage, MAX_CAMERA_DEPENDENCIES,
    MAX_CAMERA_DEPENDENCY_DEPTH,
};
pub use plugin::CameraPlugin;
