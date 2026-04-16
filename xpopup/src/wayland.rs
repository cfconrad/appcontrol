use eframe::egui;
use egui_wgpu::wgpu;
use raw_window_handle::{WaylandDisplayHandle, WaylandWindowHandle};
use std::ptr::NonNull;
use wayland_client::{
    protocol::{
        wl_compositor::WlCompositor,
        wl_output::WlOutput,
        wl_pointer::WlPointer,
        wl_registry::WlRegistry,
        wl_seat::WlSeat,
        wl_surface::WlSurface,
    },
    Connection, Dispatch, Proxy, QueueHandle, WEnum,
};
use wayland_protocols::ext::session_lock::v1::client::{
    ext_session_lock_manager_v1::ExtSessionLockManagerV1,
    ext_session_lock_surface_v1::ExtSessionLockSurfaceV1,
    ext_session_lock_v1::ExtSessionLockV1,
};

use crate::ui::{popup_ui, PopupState};

pub(crate) struct SurfaceState {
    pub wl_surface: WlSurface,
    pub lock_surface: ExtSessionLockSurfaceV1,
    pub width: u32,
    pub height: u32,
    pub configured: bool,
}

pub(crate) struct LockState {
    // Globals
    pub compositor: Option<WlCompositor>,
    pub lock_manager: Option<ExtSessionLockManagerV1>,
    pub outputs: Vec<WlOutput>,
    pub seat: Option<WlSeat>,
    pub pointer: Option<WlPointer>,

    // Session lock
    pub is_locked: bool,
    pub lock_failed: bool,

    // Per-output lock surfaces
    pub surfaces: Vec<SurfaceState>,

    // Input
    pub active_surface: Option<usize>,
    pub pointer_pos: egui::Pos2,
    pub events: Vec<egui::Event>,

    // Control
    pub needs_render: bool,
    pub exit_code: Option<i32>,
}

impl LockState {
    pub fn new() -> Self {
        Self {
            compositor: None,
            lock_manager: None,
            outputs: Vec::new(),
            seat: None,
            pointer: None,
            is_locked: false,
            lock_failed: false,
            surfaces: Vec::new(),
            active_surface: None,
            pointer_pos: egui::Pos2::ZERO,
            events: Vec::new(),
            needs_render: false,
            exit_code: None,
        }
    }
}

// --- Dispatch implementations ---

impl Dispatch<WlRegistry, ()> for LockState {
    fn event(
        state: &mut Self,
        registry: &WlRegistry,
        event: wayland_client::protocol::wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        use wayland_client::protocol::wl_registry::Event;
        if let Event::Global { name, interface, version } = event {
            match interface.as_str() {
                "wl_compositor" => {
                    state.compositor =
                        Some(registry.bind::<WlCompositor, _, _>(name, version.min(4), qh, ()));
                }
                "ext_session_lock_manager_v1" => {
                    state.lock_manager = Some(
                        registry.bind::<ExtSessionLockManagerV1, _, _>(name, 1, qh, ()),
                    );
                }
                "wl_output" => {
                    state.outputs.push(
                        registry.bind::<WlOutput, _, _>(name, version.min(4), qh, ()),
                    );
                }
                "wl_seat" => {
                    state.seat =
                        Some(registry.bind::<WlSeat, _, _>(name, version.min(8), qh, ()));
                }
                _ => {}
            }
        }
    }
}

impl Dispatch<WlCompositor, ()> for LockState {
    fn event(
        _: &mut Self,
        _: &WlCompositor,
        event: <WlCompositor as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let _ = event; // WlCompositor has no events.
    }
}

impl Dispatch<WlOutput, ()> for LockState {
    fn event(
        _: &mut Self,
        _: &WlOutput,
        _: <WlOutput as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlSeat, ()> for LockState {
    fn event(
        state: &mut Self,
        seat: &WlSeat,
        event: wayland_client::protocol::wl_seat::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        use wayland_client::protocol::wl_seat::{Capability, Event};
        if let Event::Capabilities { capabilities: WEnum::Value(caps) } = event {
            if caps.contains(Capability::Pointer) && state.pointer.is_none() {
                state.pointer = Some(seat.get_pointer(qh, ()));
            }
        }
    }
}

impl Dispatch<WlPointer, ()> for LockState {
    fn event(
        state: &mut Self,
        _: &WlPointer,
        event: wayland_client::protocol::wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use wayland_client::protocol::wl_pointer::{ButtonState, Event};
        match event {
            Event::Enter { surface, surface_x, surface_y, .. } => {
                state.active_surface = state
                    .surfaces
                    .iter()
                    .position(|s| s.wl_surface.id() == surface.id());
                let pos = egui::pos2(surface_x as f32, surface_y as f32);
                state.pointer_pos = pos;
                state.events.push(egui::Event::PointerMoved(pos));
                state.needs_render = true;
            }
            Event::Leave { .. } => {
                state.active_surface = None;
                state.events.push(egui::Event::PointerGone);
                state.needs_render = true;
            }
            Event::Motion { surface_x, surface_y, .. } => {
                let pos = egui::pos2(surface_x as f32, surface_y as f32);
                state.pointer_pos = pos;
                state.events.push(egui::Event::PointerMoved(pos));
                state.needs_render = true;
            }
            Event::Button { button, state: WEnum::Value(btn_state), .. } => {
                if button == 0x110 {
                    // BTN_LEFT
                    let pressed = btn_state == ButtonState::Pressed;
                    state.events.push(egui::Event::PointerButton {
                        pos: state.pointer_pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::default(),
                    });
                    state.needs_render = true;
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<WlSurface, ()> for LockState {
    fn event(
        _: &mut Self,
        _: &WlSurface,
        _: <WlSurface as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ExtSessionLockManagerV1, ()> for LockState {
    fn event(
        _: &mut Self,
        _: &ExtSessionLockManagerV1,
        event: <ExtSessionLockManagerV1 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let _ = event;
    }
}

impl Dispatch<ExtSessionLockV1, ()> for LockState {
    fn event(
        state: &mut Self,
        _: &ExtSessionLockV1,
        event: wayland_protocols::ext::session_lock::v1::client::ext_session_lock_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use wayland_protocols::ext::session_lock::v1::client::ext_session_lock_v1::Event;
        match event {
            Event::Locked => state.is_locked = true,
            Event::Finished => state.lock_failed = true,
            _ => {}
        }
    }
}

/// User data for `ExtSessionLockSurfaceV1` dispatch is the index into
/// `LockState::surfaces` so the configure event knows which surface to update.
impl Dispatch<ExtSessionLockSurfaceV1, usize> for LockState {
    fn event(
        state: &mut Self,
        _: &ExtSessionLockSurfaceV1,
        event: wayland_protocols::ext::session_lock::v1::client::ext_session_lock_surface_v1::Event,
        &idx: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use wayland_protocols::ext::session_lock::v1::client::ext_session_lock_surface_v1::Event;
        if let Event::Configure { serial, width, height } = event {
            if let Some(surf) = state.surfaces.get_mut(idx) {
                surf.width = width;
                surf.height = height;
                surf.configured = true;
                surf.lock_surface.ack_configure(serial);
                surf.wl_surface.commit();
            }
            state.needs_render = true;
        }
    }
}

pub(crate) fn run_wayland_locked(message: String, bg_image_path: Option<String>, warn_only: bool) {
    // --- Connect and scan globals ---
    let conn = Connection::connect_to_env().expect("xpopup: cannot connect to Wayland");
    let mut queue = conn.new_event_queue::<LockState>();
    let qh = queue.handle();
    conn.display().get_registry(&qh, ());

    let mut state = LockState::new();
    queue.roundtrip(&mut state).expect("xpopup: Wayland roundtrip failed");

    if state.outputs.is_empty() {
        eprintln!("xpopup: no Wayland outputs found");
        std::process::exit(1);
    }

    // Activate pointer if seat is available.
    if let Some(ref seat) = state.seat.clone() {
        // get_pointer triggers a Capabilities event; issue a roundtrip first.
        queue.roundtrip(&mut state).unwrap();
        // If capability was already received, pointer is set; otherwise try directly.
        if state.pointer.is_none() {
            state.pointer = Some(seat.get_pointer(&qh, ()));
        }
    }

    // --- Initiate session lock ---
    let manager = state
        .lock_manager
        .take()
        .expect("xpopup: compositor does not support ext-session-lock-v1");
    let lock = manager.lock(&qh, ());
    queue.roundtrip(&mut state).expect("xpopup: lock roundtrip failed");

    if state.lock_failed {
        eprintln!("xpopup: compositor refused session lock");
        std::process::exit(1);
    }
    if !state.is_locked {
        eprintln!("xpopup: did not receive Locked event");
        std::process::exit(1);
    }

    // --- Create one lock surface per output ---
    let compositor = state.compositor.clone().expect("xpopup: no wl_compositor");
    let outputs: Vec<WlOutput> = state.outputs.clone();
    for (idx, output) in outputs.iter().enumerate() {
        let wl_surface = compositor.create_surface(&qh, ());
        let lock_surface = lock.get_lock_surface(&wl_surface, output, &qh, idx);
        state.surfaces.push(SurfaceState {
            wl_surface,
            lock_surface,
            width: 0,
            height: 0,
            configured: false,
        });
    }

    // Wait for Configure events on all surfaces.
    queue.roundtrip(&mut state).unwrap();
    queue.roundtrip(&mut state).unwrap(); // second roundtrip for any late events
    conn.flush().unwrap();

    let configured: Vec<&SurfaceState> =
        state.surfaces.iter().filter(|s| s.configured).collect();
    if configured.is_empty() {
        eprintln!("xpopup: no lock surfaces were configured");
        std::process::exit(1);
    }

    // --- Set up wgpu ---
    let display_ptr: *mut std::ffi::c_void =
        conn.backend().display_ptr() as *mut std::ffi::c_void;

    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::all(),
        ..Default::default()
    });

    struct RenderTarget {
        surface: wgpu::Surface<'static>,
        width: u32,
        height: u32,
    }

    let mut render_targets: Vec<RenderTarget> = Vec::new();
    let mut device_queue: Option<(wgpu::Device, wgpu::Queue)> = None;
    let mut surface_format = wgpu::TextureFormat::Bgra8UnormSrgb;

    for surf in state.surfaces.iter().filter(|s| s.configured) {
        let surface_ptr: *mut std::ffi::c_void = surf.wl_surface.id().as_ptr() as *mut _;

        // Safety: display_ptr and surface_ptr are valid Wayland handles that
        // remain valid for the duration of this function (and thus the wgpu Surface).
        let wgpu_surface: wgpu::Surface<'static> = unsafe {
            instance
                .create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
                    raw_display_handle: raw_window_handle::RawDisplayHandle::Wayland(
                        WaylandDisplayHandle::new(NonNull::new(display_ptr).unwrap()),
                    ),
                    raw_window_handle: raw_window_handle::RawWindowHandle::Wayland(
                        WaylandWindowHandle::new(NonNull::new(surface_ptr).unwrap()),
                    ),
                })
                .expect("xpopup: failed to create wgpu surface")
        };

        if device_queue.is_none() {
            let adapter = pollster::block_on(instance.request_adapter(
                &wgpu::RequestAdapterOptions {
                    compatible_surface: Some(&wgpu_surface),
                    power_preference: wgpu::PowerPreference::None,
                    force_fallback_adapter: false,
                },
            ))
            .expect("xpopup: no wgpu adapter available");

            let (device, queue) = pollster::block_on(adapter.request_device(
                &wgpu::DeviceDescriptor {
                    label: None,
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::default(),
                    memory_hints: wgpu::MemoryHints::default(),
                },
                None,
            ))
            .expect("xpopup: failed to create wgpu device");

            let caps = wgpu_surface.get_capabilities(&adapter);
            surface_format = caps
                .formats
                .iter()
                .find(|f| f.is_srgb())
                .copied()
                .unwrap_or(caps.formats[0]);

            device_queue = Some((device, queue));
        }

        let (device, _) = device_queue.as_ref().unwrap();
        wgpu_surface.configure(
            device,
            &wgpu::SurfaceConfiguration {
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                format: surface_format,
                width: surf.width,
                height: surf.height,
                present_mode: wgpu::PresentMode::Fifo,
                desired_maximum_frame_latency: 2,
                alpha_mode: wgpu::CompositeAlphaMode::Auto,
                view_formats: vec![],
            },
        );

        render_targets.push(RenderTarget {
            surface: wgpu_surface,
            width: surf.width,
            height: surf.height,
        });
    }

    let (device, gpu_queue) = device_queue.expect("xpopup: no render targets");
    let mut renderer = egui_wgpu::Renderer::new(&device, surface_format, None, 1, false);

    // --- Set up egui ---
    let egui_ctx = egui::Context::default();
    egui_ctx.set_visuals(egui::Visuals::dark());

    let mut popup_state = PopupState::new(message, bg_image_path, warn_only);

    // Load background texture into the egui context.
    popup_state.load_texture(&egui_ctx);
    // Prime egui so any initial texture uploads are available on the first frame.
    let prime = egui_ctx.run(egui::RawInput::default(), |_| {});
    for (id, delta) in &prime.textures_delta.set {
        renderer.update_texture(&device, &gpu_queue, *id, delta);
    }

    // --- Event + render loop ---
    state.needs_render = true;

    loop {
        if state.needs_render {
            let (width, height) = render_targets
                .first()
                .map(|t| (t.width, t.height))
                .unwrap_or((1920, 1080));

            let raw_input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(width as f32, height as f32),
                )),
                events: std::mem::take(&mut state.events),
                ..Default::default()
            };

            let full_output = egui_ctx.run(raw_input, |ctx| {
                if let Some(code) = popup_ui(ctx, &mut popup_state) {
                    state.exit_code = Some(code);
                }
            });

            let clipped = egui_ctx.tessellate(full_output.shapes, full_output.pixels_per_point);

            for (id, delta) in &full_output.textures_delta.set {
                renderer.update_texture(&device, &gpu_queue, *id, delta);
            }

            for target in &render_targets {
                let screen_desc = egui_wgpu::ScreenDescriptor {
                    size_in_pixels: [target.width, target.height],
                    pixels_per_point: full_output.pixels_per_point,
                };
                let frame = match target.surface.get_current_texture() {
                    Ok(f) => f,
                    Err(e) => {
                        eprintln!("xpopup: get_current_texture: {e}");
                        continue;
                    }
                };
                let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
                let mut encoder =
                    device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());

                let extra_cmds =
                    renderer.update_buffers(&device, &gpu_queue, &mut encoder, &clipped, &screen_desc);

                {
                    let mut rp = encoder
                        .begin_render_pass(&wgpu::RenderPassDescriptor {
                            label: None,
                            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                view: &view,
                                resolve_target: None,
                                ops: wgpu::Operations {
                                    load: wgpu::LoadOp::Clear(wgpu::Color {
                                        r: 0.2,
                                        g: 0.2,
                                        b: 0.2,
                                        a: 1.0,
                                    }),
                                    store: wgpu::StoreOp::Store,
                                },
                            })],
                            depth_stencil_attachment: None,
                            timestamp_writes: None,
                            occlusion_query_set: None,
                        })
                        .forget_lifetime();
                    renderer.render(&mut rp, &clipped, &screen_desc);
                }

                gpu_queue
                    .submit(extra_cmds.into_iter().chain(std::iter::once(encoder.finish())));
                frame.present();
            }

            for id in &full_output.textures_delta.free {
                renderer.free_texture(id);
            }

            state.needs_render = false;
        }

        conn.flush().unwrap();

        if let Some(code) = state.exit_code {
            lock.unlock_and_destroy();
            conn.flush().unwrap();
            std::process::exit(code);
        }

        // Block until the next Wayland event arrives, then re-render.
        queue.blocking_dispatch(&mut state).expect("xpopup: Wayland dispatch failed");
        state.needs_render = true;
    }
}
