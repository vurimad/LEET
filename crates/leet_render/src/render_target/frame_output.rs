use crate::PresentationIntent;

pub enum FrameOutput {
    WindowSurface(wgpu::SurfaceTexture),
    TextureView,
    Targetless,
}

impl FrameOutput {
    pub fn finish(self, presentation: PresentationIntent) {
        if let Self::WindowSurface(surface_texture) = self {
            if matches!(presentation, PresentationIntent::Present) {
                surface_texture.present();
            }
        }
    }
}
