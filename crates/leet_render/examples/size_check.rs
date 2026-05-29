enum RenderViewportOutputForSize {
    WindowSurface {
        surface_texture: wgpu::SurfaceTexture,
        view: wgpu::TextureView,
    },
    TextureView {
        view: wgpu::TextureView,
    },
}

fn main() {
    println!(
        "SurfaceTexture {}",
        std::mem::size_of::<wgpu::SurfaceTexture>()
    );
    println!("TextureView {}", std::mem::size_of::<wgpu::TextureView>());
    println!(
        "RenderViewportOutputForSize {}",
        std::mem::size_of::<RenderViewportOutputForSize>()
    );
}
