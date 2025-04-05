// Copyright 2024 the Vello Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Simple example.

use pathfinder_geometry::transform2d::Transform2F;
use pdf_render::vello_backend::{OutlineBuilder, VelloBackend};
use pdf_render::{render_page, Cache};
use std::num::NonZeroUsize;
use std::sync::Arc;
use vello::kurbo::{Affine, Circle, Ellipse, Line, RoundedRect, Stroke};
use vello::peniko::color::palette;
use vello::peniko::Color;
use vello::util::{RenderContext, RenderSurface};
use vello::{AaConfig, Renderer, RendererOptions, Scene};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::Window;

use vello::wgpu;
/// Simple struct to hold the state of the renderer
#[derive(Debug)]
pub struct ActiveRenderState<'s> {
    // The fields MUST be in this order, so that the surface is dropped before the window
    surface: RenderSurface<'s>,
    window: Arc<Window>,
}

enum RenderState<'s> {
    Active(ActiveRenderState<'s>),
    // Cache a window so that it can be reused when the app is resumed after being suspended
    Suspended(Option<Arc<Window>>),
}

pub struct App<'s> {
    // The vello RenderContext which is a global context that lasts for the
    // lifetime of the application
    context: RenderContext,

    // An array of renderers, one per wgpu device
    renderers: Vec<Option<Renderer>>,

    // State for our example where we store the winit Window and the wgpu Surface
    state: RenderState<'s>,

    // A vello Scene which is a data structure which allows one to build up a
    // description a scene to be drawn (with paths, fills, images, text, etc)
    // which is then passed to a renderer for rendering
    scene: Scene,

    view_ctx: ViewContext,
}

impl<'s> App<'s> {
    fn new(view_ctx: ViewContext) -> Self {
        Self {
            renderers: vec![],
            context: RenderContext::new(),
            state: RenderState::Suspended(None),
            // modifiers: ModifiersState::default(),
            // mouse_down: false,
            scene: Scene::new(),
            view_ctx,
            // prior_position: None,
            // transform: Transform2F::default(),
        }
    }

    pub fn run(view_ctx: ViewContext) {
        let event_loop = EventLoop::new().unwrap();

        event_loop.set_control_flow(ControlFlow::Wait);

        let mut app = App::new(view_ctx);

        let _ = event_loop.run_app(&mut app);
    }
}

impl ApplicationHandler for App<'_> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let RenderState::Suspended(cached_window) = &mut self.state else {
            return;
        };

        // Get the winit window cached in a previous Suspended event or else create a new window
        let window = cached_window
            .take()
            .unwrap_or_else(|| create_winit_window(event_loop));

        // Create a vello Surface
        let size = window.inner_size();
        let surface_future = self.context.create_surface(
            window.clone(),
            size.width,
            size.height,
            wgpu::PresentMode::AutoVsync,
        );
        let surface = pollster::block_on(surface_future).expect("Error creating surface");

        // Create a vello Renderer for the surface (using its device id)
        self.renderers
            .resize_with(self.context.devices.len(), || None);
        self.renderers[surface.dev_id]
            .get_or_insert_with(|| create_vello_renderer(&self.context, &surface));

        // Save the Window and Surface to a state variable
        self.state = RenderState::Active(ActiveRenderState { window, surface });
    }

    fn suspended(&mut self, _event_loop: &ActiveEventLoop) {
        if let RenderState::Active(state) = &self.state {
            self.state = RenderState::Suspended(Some(state.window.clone()));
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: winit::window::WindowId,
        event: WindowEvent,
    ) {
        // Ignore the event (return from the function) if
        //   - we have no render_state
        //   - OR the window id of the event doesn't match the window id of our render_state
        //
        // Else extract a mutable reference to the render state from its containing option for use below
        let render_state = match &mut self.state {
            RenderState::Active(state) if state.window.id() == window_id => state,
            _ => return,
        };

        match event {
            // Exit the event loop when a close is requested (e.g. window's close button is pressed)
            WindowEvent::CloseRequested => event_loop.exit(),

            // Resize the surface when the window is resized
            WindowEvent::Resized(size) => {
                self.context
                    .resize_surface(&mut render_state.surface, size.width, size.height);
            }

            // This is where all the rendering happens
            WindowEvent::RedrawRequested => {
                // Empty the scene of objects to draw. You could create a new Scene each time, but in this case
                // the same Scene is reused so that the underlying memory allocation can also be reused.
                self.scene.reset();

                // Re-add the objects to draw to the scene.
                // add_shapes_to_scene(&mut self.scene);

                let mut scene = Scene::new();

                if let Some(current) = self.view_ctx.get_current_mut() {
                    if let Some(s) = current.render(render_state.window.clone(), self.transform) {
                        self.scene.append(&s, None);
                    }
                }

                // Get the RenderSurface (surface + config)
                let surface = &render_state.surface;

                // Get the window size
                let width = surface.config.width;
                let height = surface.config.height;

                // Get a handle to the device
                let device_handle = &self.context.devices[surface.dev_id];

                // Get the surface's texture
                let surface_texture = surface
                    .surface
                    .get_current_texture()
                    .expect("failed to get surface texture");

                // Render to the surface's texture
                self.renderers[surface.dev_id]
                    .as_mut()
                    .unwrap()
                    .render_to_surface(
                        &device_handle.device,
                        &device_handle.queue,
                        &self.scene,
                        &surface_texture,
                        &vello::RenderParams {
                            base_color: palette::css::BLACK, // Background color
                            width,
                            height,
                            antialiasing_method: AaConfig::Msaa16,
                        },
                    )
                    .expect("failed to render to surface");

                // Queue the texture to be presented on the surface
                surface_texture.present();

                device_handle.device.poll(wgpu::Maintain::Poll);
            }
            _ => {}
        }
    }
}

/// Helper function that creates a Winit window and returns it (wrapped in an Arc for sharing between threads)
fn create_winit_window(event_loop: &ActiveEventLoop) -> Arc<Window> {
    let attr = Window::default_attributes()
        .with_inner_size(LogicalSize::new(1044, 800))
        .with_resizable(true)
        .with_title("Vello Shapes");
    Arc::new(event_loop.create_window(attr).unwrap())
}

/// Helper function that creates a vello `Renderer` for a given `RenderContext` and `RenderSurface`
fn create_vello_renderer(render_cx: &RenderContext, surface: &RenderSurface<'_>) -> Renderer {
    Renderer::new(
        &render_cx.devices[surface.dev_id].device,
        RendererOptions {
            surface_format: Some(surface.format),
            use_cpu: false,
            antialiasing_support: vello::AaSupport::all(),
            num_init_threads: NonZeroUsize::new(1),
        },
    )
    .expect("Couldn't create renderer")
}

/// Add shapes to a vello scene. This does not actually render the shapes, but adds them
/// to the Scene data structure which represents a set of objects to draw.
fn add_shapes_to_scene(scene: &mut Scene) {
    // Draw an outlined rectangle
    let stroke = Stroke::new(6.0);
    let rect = RoundedRect::new(10.0, 10.0, 240.0, 240.0, 20.0);
    let rect_stroke_color = Color::new([0.9804, 0.702, 0.5294, 1.]);
    scene.stroke(&stroke, Affine::IDENTITY, rect_stroke_color, None, &rect);

    // Draw a filled circle
    let circle = Circle::new((420.0, 200.0), 120.0);
    let circle_fill_color = Color::new([0.9529, 0.5451, 0.6588, 1.]);
    scene.fill(
        vello::peniko::Fill::NonZero,
        Affine::IDENTITY,
        circle_fill_color,
        None,
        &circle,
    );

    // Draw a filled ellipse
    let ellipse = Ellipse::new((250.0, 420.0), (100.0, 160.0), -90.0);
    let ellipse_fill_color = Color::new([0.7961, 0.651, 0.9686, 1.]);
    scene.fill(
        vello::peniko::Fill::NonZero,
        Affine::IDENTITY,
        ellipse_fill_color,
        None,
        &ellipse,
    );

    // Draw a straight line
    let line = Line::new((260.0, 20.0), (620.0, 100.0));
    let line_stroke_color = Color::new([0.5373, 0.7059, 0.9804, 1.]);
    scene.stroke(&stroke, Affine::IDENTITY, line_stroke_color, None, &line);
}

pub struct FileContext {
    page_nr: u32,
    file: pdf::file::CachedFile<Vec<u8>>,
    cache: Cache<OutlineBuilder>,
}

impl FileContext {
    pub fn new(file: pdf::file::CachedFile<Vec<u8>>) -> Self {
        Self {
            page_nr: 0,
            file,
            cache: Cache::new(OutlineBuilder::default()),
        }
    }

    fn render(&mut self, window: Arc<Window>, transform: Transform2F) -> Option<Scene> {
        let mut backend = VelloBackend::new(&mut self.cache);
        let resolver = self.file.resolver();

        let window_size: winit::dpi::PhysicalSize<u32> = window.inner_size();

        let page = self.file.get_page(self.page_nr).ok()?;

        render_page(&mut backend, &resolver, &page, transform).ok()?;

        Some(backend.finish())
    }
}

pub struct ViewContext {
    files: Vec<FileContext>,
    current_file: Option<usize>,
}

impl ViewContext {
    pub fn new(files: Vec<FileContext>) -> Self {
        let current_file = if files.is_empty() { None } else { Some(0) };

        Self {
            files,
            current_file,
        }
    }

    fn get_current(&self) -> Option<&FileContext> {
        if let Some(n) = self.current_file {
            self.files.get(n)
        } else {
            None
        }
    }
    fn get_current_mut(&mut self) -> Option<&mut FileContext> {
        if let Some(n) = self.current_file {
            self.files.get_mut(n)
        } else {
            None
        }
    }
    fn seek_forward(&mut self, n: u32) {
        if let Some(current) = self.get_current_mut() {
            current.page_nr = (current.page_nr + n).min(current.file.num_pages());
        }
    }
    fn seek_backwards(&mut self, n: u32) {
        if let Some(current) = self.get_current_mut() {
            current.page_nr = current.page_nr.saturating_sub(n);
        }
    }
}
