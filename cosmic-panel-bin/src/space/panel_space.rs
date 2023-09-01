use std::{
    cell::{Cell, RefCell},
    os::{fd::RawFd, unix::net::UnixStream},
    rc::Rc,
    time::{Duration, Instant},
};

use cctk::cosmic_protocols::toplevel_info::v1::client::zcosmic_toplevel_handle_v1::ZcosmicToplevelHandleV1;
use cosmic_config::{Config, CosmicConfigEntry};
use image::RgbaImage;
use itertools::{chain, Itertools};
use launch_pad::process::Process;
use sctk::{
    compositor::Region,
    output::OutputInfo,
    reexports::client::{
        backend::ObjectId,
        protocol::{wl_display::WlDisplay, wl_output as c_wl_output},
        Proxy, QueueHandle,
    },
    shell::{
        wlr_layer::{LayerSurface, LayerSurfaceConfigure},
        xdg::{popup, XdgPositioner},
        WaylandSurface,
    },
};
use smithay::{
    backend::{
        allocator::Fourcc,
        egl::{
            context::{GlAttributes, PixelFormatRequirements},
            ffi::egl::SwapInterval,
            EGLContext,
        },
        renderer::{
            damage::{OutputDamageTracker, RenderOutputResult},
            element::{
                memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
                surface::{render_elements_from_surface_tree, WaylandSurfaceRenderElement},
            },
            Bind, ImportAll, ImportMem, Unbind,
        },
    },
    output::Output,
    reexports::{
        wayland_protocols::xdg::shell::client::xdg_positioner::{Anchor, Gravity},
        wayland_server::{backend::ClientId, DisplayHandle},
    },
    render_elements,
    utils::Transform,
    wayland::{
        seat::WaylandFocus,
        shell::xdg::{PopupSurface, PositionerState},
    },
};
use smithay::{
    backend::{
        egl::{display::EGLDisplay, surface::EGLSurface},
        renderer::gles::GlesRenderer,
    },
    desktop::{PopupKind, PopupManager, Space, Window},
    reexports::wayland_server::{Client, Resource},
    utils::{Logical, Point, Rectangle, Size},
};
use smithay::utils::IsAlive;
use tokio::sync::{mpsc, oneshot};
use tracing::{error, info};
use wayland_egl::WlEglSurface;
use wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_v1::WpFractionalScaleV1;
use wayland_protocols::wp::viewporter::client::wp_viewport::WpViewport;
use xdg_shell_wrapper::{
    client_state::{ClientFocus, FocusStatus},
    server_state::{ServerFocus, ServerPtrFocus},
    shared_state::GlobalState,
    space::{
        ClientEglDisplay, ClientEglSurface, SpaceEvent, Visibility, WrapperPopup,
        WrapperPopupState, WrapperSpace,
    },
    util::smootherstep,
};

use cosmic_panel_config::{CosmicPanelBackground, CosmicPanelConfig, PanelAnchor};

use crate::space::Alignment;

pub enum AppletMsg {
    NewProcess(ObjectId, Process),
    NewNotificationsProcess(ObjectId, Process, Vec<(String, String)>),
    NeedNewNotificationFd(oneshot::Sender<RawFd>),
    ClientSocketPair(String, ClientId, Client, UnixStream),
    Cleanup(ObjectId),
}

render_elements! {
    MyRenderElements<R> where R: ImportMem + ImportAll;
    Memory=MemoryRenderBufferRenderElement<R>,
    WaylandSurface=WaylandSurfaceRenderElement<R>
}

/// space for the cosmic panel
#[derive(Debug)]
pub(crate) struct PanelSpace {
    // XXX implicitly drops egl_surface first to avoid segfault
    pub(crate) egl_surface: Option<Rc<EGLSurface>>,
    pub(crate) c_display: Option<WlDisplay>,
    pub config: CosmicPanelConfig,
    pub(crate) space: Space<Window>,
    pub(crate) damage_tracked_renderer: Option<OutputDamageTracker>,
    pub(crate) clients_left: Vec<(String, Client, UnixStream)>,
    pub(crate) clients_center: Vec<(String, Client, UnixStream)>,
    pub(crate) clients_right: Vec<(String, Client, UnixStream)>,
    pub(crate) last_dirty: Option<Instant>,
    pub(crate) pending_dimensions: Option<Size<i32, Logical>>,
    pub(crate) suggested_length: Option<u32>,
    pub(crate) actual_size: Size<i32, Logical>,
    pub(crate) is_dirty: bool,
    pub(crate) space_event: Rc<Cell<Option<SpaceEvent>>>,
    pub(crate) dimensions: Size<i32, Logical>,
    pub(crate) c_focused_surface: Rc<RefCell<ClientFocus>>,
    pub(crate) c_hovered_surface: Rc<RefCell<ClientFocus>>,
    pub(crate) s_focused_surface: ServerFocus,
    pub(crate) s_hovered_surface: ServerPtrFocus,
    pub(crate) visibility: Visibility,
    pub(crate) output: Option<(c_wl_output::WlOutput, Output, OutputInfo)>,
    pub(crate) s_display: Option<DisplayHandle>,
    pub(crate) layer: Option<LayerSurface>,
    pub(crate) layer_fractional_scale: Option<WpFractionalScaleV1>,
    pub(crate) layer_viewport: Option<WpViewport>,
    pub(crate) popups: Vec<WrapperPopup>,
    pub(crate) start_instant: Instant,
    pub(crate) bg_color: [f32; 4],
    pub applet_tx: mpsc::Sender<AppletMsg>,
    pub(crate) input_region: Option<Region>,
    old_buff: Option<MemoryRenderBuffer>,
    buffer: Option<MemoryRenderBuffer>,
    buffer_changed: bool,
    pub(crate) has_frame: bool,
    pub(crate) scale: f64,
}

impl PanelSpace {
    /// create a new space for the cosmic panel
    pub fn new(
        config: CosmicPanelConfig,
        c_focused_surface: Rc<RefCell<ClientFocus>>,
        c_hovered_surface: Rc<RefCell<ClientFocus>>,
        applet_tx: mpsc::Sender<AppletMsg>,
    ) -> Self {
        let bg_color = match config.background {
            CosmicPanelBackground::ThemeDefault => {
                let t = Config::new("com.system76.CosmicTheme", 1)
                    .map(|helper| match cosmic_theme::Theme::get_entry(&helper) {
                        Ok(c) => c,
                        Err((err, c)) => {
                            for e in err {
                                error!("Error loading cosmic theme for {} {:?}", &config.name, e);
                            }
                            c
                        }
                    })
                    .unwrap_or(cosmic_theme::Theme::dark_default());
                let c = [
                    t.bg_color().red,
                    t.bg_color().green,
                    t.bg_color().blue,
                    config.opacity,
                ];
                c
            }
            CosmicPanelBackground::Dark => {
                let t = cosmic_theme::Theme::dark_default();
                let c = [
                    t.bg_color().red,
                    t.bg_color().green,
                    t.bg_color().blue,
                    config.opacity,
                ];
                c
            }
            CosmicPanelBackground::Light => {
                let t = cosmic_theme::Theme::light_default();
                let c = [
                    t.bg_color().red,
                    t.bg_color().green,
                    t.bg_color().blue,
                    config.opacity,
                ];
                c
            }
            CosmicPanelBackground::Color(c) => [c[0], c[1], c[2], config.opacity],
        };

        let visibility = if config.autohide.is_none() {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };

        Self {
            config,
            space: Space::default(),
            clients_left: Default::default(),
            clients_center: Default::default(),
            clients_right: Default::default(),
            last_dirty: Default::default(),
            pending_dimensions: Default::default(),
            space_event: Default::default(),
            dimensions: Default::default(),
            suggested_length: None,
            output: Default::default(),
            s_display: Default::default(),
            c_display: Default::default(),
            layer: Default::default(),
            layer_fractional_scale: Default::default(),
            layer_viewport: Default::default(),
            egl_surface: Default::default(),
            popups: Default::default(),
            visibility,
            start_instant: Instant::now(),
            c_focused_surface,
            c_hovered_surface,
            s_focused_surface: Default::default(),
            s_hovered_surface: Default::default(),
            bg_color,
            applet_tx,
            actual_size: (0, 0).into(),
            input_region: None,
            damage_tracked_renderer: Default::default(),
            is_dirty: false,
            old_buff: Default::default(),
            buffer: Default::default(),
            buffer_changed: false,
            has_frame: true,
            scale: 1.0,
        }
    }

    pub(crate) fn close_popups(&mut self) {
        for w in &mut self.space.elements() {
            for (PopupKind::Xdg(p), _) in
                PopupManager::popups_for_surface(w.toplevel().wl_surface())
            {
                if !self
                    .s_hovered_surface
                    .iter()
                    .any(|hs| &hs.surface == w.toplevel().wl_surface())
                {
                    p.send_popup_done();
                }
            }
        }
    }

    pub(crate) fn handle_focus(&mut self) {
        let (layer_surface, layer_shell_wl_surface) =
            if let Some(layer_surface) = self.layer.as_ref() {
                (layer_surface, layer_surface.wl_surface())
            } else {
                return;
            };
        let cur_focus = {
            let c_focused_surface = self.c_focused_surface.borrow();
            let c_hovered_surface = self.c_hovered_surface.borrow();
            // no transition if not configured for autohide
            if self.config.autohide().is_none() {
                if c_focused_surface
                    .iter()
                    .all(|f| matches!(f.2, FocusStatus::LastFocused(_)))
                    && c_hovered_surface
                        .iter()
                        .all(|f| matches!(f.2, FocusStatus::LastFocused(_)))
                {
                    self.visibility = Visibility::Hidden;
                } else {
                    self.visibility = Visibility::Visible;
                }
                return;
            }

            c_focused_surface
                .iter()
                .chain(c_hovered_surface.iter())
                .fold(
                    FocusStatus::LastFocused(self.start_instant),
                    |acc, (surface, _, f)| {
                        if self
                            .layer
                            .as_ref()
                            .map(|s| *s.wl_surface() == *surface)
                            .unwrap_or(false)
                            || self.popups.iter().any(|p| {
                                &p.c_popup.wl_surface() == &surface
                                    || self
                                        .popups
                                        .iter()
                                        .any(|p| p.c_popup.wl_surface() == surface)
                            })
                        {
                            match (&acc, &f) {
                                (
                                    FocusStatus::LastFocused(t_acc),
                                    FocusStatus::LastFocused(t_cur),
                                ) => {
                                    if t_cur > t_acc {
                                        *f
                                    } else {
                                        acc
                                    }
                                }
                                (FocusStatus::LastFocused(_), FocusStatus::Focused) => *f,
                                _ => acc,
                            }
                        } else {
                            acc
                        }
                    },
                )
        };
        match self.visibility {
            Visibility::Hidden => {
                if let FocusStatus::Focused = cur_focus {
                    // start transition to visible
                    let margin = match self.config.anchor() {
                        PanelAnchor::Left | PanelAnchor::Right => -(self.dimensions.w),
                        PanelAnchor::Top | PanelAnchor::Bottom => -(self.dimensions.h),
                    } + self.config.get_hide_handle().unwrap() as i32;
                    self.visibility = Visibility::TransitionToVisible {
                        last_instant: Instant::now(),
                        progress: Duration::new(0, 0),
                        prev_margin: margin,
                    }
                }
            }
            Visibility::Visible => {
                if let FocusStatus::LastFocused(t) = cur_focus {
                    // start transition to hidden
                    let duration_since_last_focus = match Instant::now().checked_duration_since(t) {
                        Some(d) => d,
                        None => return,
                    };
                    if duration_since_last_focus > self.config.get_hide_wait().unwrap() {
                        self.visibility = Visibility::TransitionToHidden {
                            last_instant: Instant::now(),
                            progress: Duration::new(0, 0),
                            prev_margin: 0,
                        }
                    }
                }
            }
            Visibility::TransitionToHidden {
                last_instant,
                progress,
                prev_margin,
            } => {
                let now = Instant::now();
                let total_t = self.config.get_hide_transition().unwrap();
                let delta_t = match now.checked_duration_since(last_instant) {
                    Some(d) => d,
                    None => return,
                };
                let prev_progress = progress;
                let progress = match prev_progress.checked_add(delta_t) {
                    Some(d) => d,
                    None => return,
                };
                let progress_norm =
                    smootherstep(progress.as_millis() as f32 / total_t.as_millis() as f32);
                let handle = self.config.get_hide_handle().unwrap() as i32;

                if let FocusStatus::Focused = cur_focus {
                    // start transition to visible
                    self.visibility = Visibility::TransitionToVisible {
                        last_instant: now,
                        progress: total_t.checked_sub(progress).unwrap_or_default(),
                        prev_margin,
                    }
                } else {
                    let panel_size = match self.config.anchor() {
                        PanelAnchor::Left | PanelAnchor::Right => {
                            self.dimensions.w + self.config.get_effective_anchor_gap() as i32
                        }
                        PanelAnchor::Top | PanelAnchor::Bottom => {
                            self.dimensions.h + self.config.get_effective_anchor_gap() as i32
                        }
                    };
                    let target = -panel_size + handle;

                    let cur_pix = (progress_norm * target as f32) as i32;
                    let margin = self.config.get_margin() as i32;

                    if progress > total_t {
                        if self.config.exclusive_zone() {
                            layer_surface.set_exclusive_zone(panel_size + handle);
                        }
                        Self::set_margin(self.config.anchor, margin, target, layer_surface);
                        layer_shell_wl_surface.commit();
                        self.visibility = Visibility::Hidden;
                    } else {
                        if prev_margin != cur_pix {
                            if self.config.exclusive_zone() {
                                layer_surface.set_exclusive_zone(panel_size - cur_pix);
                            }
                            Self::set_margin(self.config.anchor, margin, cur_pix, layer_surface);

                            layer_shell_wl_surface.commit();
                        }
                        self.close_popups();
                        self.visibility = Visibility::TransitionToHidden {
                            last_instant: now,
                            progress,
                            prev_margin: cur_pix,
                        };
                    }
                }
            }
            Visibility::TransitionToVisible {
                last_instant,
                progress,
                prev_margin,
            } => {
                let now = Instant::now();
                let total_t = self.config.get_hide_transition().unwrap();
                let delta_t = match now.checked_duration_since(last_instant) {
                    Some(d) => d,
                    None => return,
                };
                let prev_progress = progress;
                let progress = match prev_progress.checked_add(delta_t) {
                    Some(d) => d,
                    None => return,
                };
                let progress_norm =
                    smootherstep(progress.as_millis() as f32 / total_t.as_millis() as f32);
                let handle = self.config.get_hide_handle().unwrap() as i32;

                if let FocusStatus::LastFocused(_) = cur_focus {
                    // start transition to visible
                    self.close_popups();
                    self.visibility = Visibility::TransitionToHidden {
                        last_instant: now,
                        progress: total_t.checked_sub(progress).unwrap_or_default(),
                        prev_margin,
                    }
                } else {
                    let panel_size = match self.config.anchor() {
                        PanelAnchor::Left | PanelAnchor::Right => {
                            self.dimensions.w + self.config.get_effective_anchor_gap() as i32
                        }
                        PanelAnchor::Top | PanelAnchor::Bottom => {
                            self.dimensions.h + self.config.get_effective_anchor_gap() as i32
                        }
                    };
                    let start = -panel_size + handle;

                    let cur_pix = ((1.0 - progress_norm) * start as f32) as i32;

                    if progress > total_t {
                        if self.config.exclusive_zone() {
                            layer_surface.set_exclusive_zone(panel_size);
                        }
                        Self::set_margin(
                            self.config.anchor,
                            self.config.get_margin() as i32,
                            0,
                            layer_surface,
                        );
                        layer_shell_wl_surface.commit();
                        self.visibility = Visibility::Visible;
                    } else {
                        if prev_margin != cur_pix {
                            if self.config.exclusive_zone() {
                                layer_surface.set_exclusive_zone(panel_size - cur_pix);
                            }
                            let margin = self.config.get_margin() as i32;
                            Self::set_margin(self.config.anchor, margin, cur_pix, layer_surface);

                            layer_shell_wl_surface.commit();
                        }
                        self.visibility = Visibility::TransitionToVisible {
                            last_instant: now,
                            progress,
                            prev_margin: cur_pix,
                        };
                    }
                }
            }
        }
    }

    fn set_margin(anchor: PanelAnchor, margin: i32, target: i32, layer_surface: &LayerSurface) {
        match anchor {
            PanelAnchor::Left => layer_surface.set_margin(margin, 0, margin, target),
            PanelAnchor::Right => layer_surface.set_margin(margin, target, margin, 0),
            PanelAnchor::Top => layer_surface.set_margin(target, margin, 0, margin),
            PanelAnchor::Bottom => layer_surface.set_margin(0, margin, target, margin),
        };
    }

    pub(crate) fn constrain_dim(&self, size: Size<i32, Logical>) -> Size<i32, Logical> {
        let mut w = size.w.try_into().unwrap();
        let mut h = size.h.try_into().unwrap();

        let output_dims = self
            .output
            .as_ref()
            .and_then(|(_, _, info)| {
                info.modes
                    .iter()
                    .find_map(|m| if m.current { Some(m.dimensions) } else { None })
            })
            .map(|(w, h)| (w as u32, h as u32));

        if let (Some(w_range), _) = self
            .config
            .get_dimensions(output_dims, self.suggested_length)
        {
            if w < w_range.start {
                w = w_range.start;
            } else if w >= w_range.end {
                w = w_range.end - 1;
            }
        }
        if let (_, Some(h_range)) = self
            .config
            .get_dimensions(output_dims, self.suggested_length)
        {
            if h < h_range.start {
                h = h_range.start;
            } else if h >= h_range.end {
                h = h_range.end - 1;
            }
        }

        (w as i32, h as i32).into()
    }

    pub(crate) fn render<W: WrapperSpace>(
        &mut self,
        renderer: &mut GlesRenderer,
        time: u32,
        qh: &QueueHandle<GlobalState<W>>,
    ) -> anyhow::Result<()> {
        if self.space_event.get() != None {
            return Ok(());
        }

        if self.is_dirty && self.has_frame {
            let my_renderer = match self.damage_tracked_renderer.as_mut() {
                Some(r) => r,
                None => return Ok(()),
            };
            renderer.unbind()?;
            renderer.bind(self.egl_surface.as_ref().unwrap().clone())?;
            let is_dock = !self.config.expand_to_edges();
            let clear_color = if self.buffer.is_none() {
                &self.bg_color
            } else {
                &[0.0, 0.0, 0.0, 0.0]
            };

            if let Some((o, _info)) = &self.output.as_ref().map(|(_, o, info)| (o, info)) {
                let mut elements: Vec<MyRenderElements<_>> = self
                    .space
                    .elements()
                    .map(|w| {
                        let loc = self
                            .space
                            .element_location(w)
                            .unwrap_or_default()
                            .to_f64()
                            .to_physical(self.scale)
                            .to_i32_round();
                        render_elements_from_surface_tree(
                            renderer,
                            w.toplevel().wl_surface(),
                            loc,
                            1.0,
                            1.0,
                        )
                        .into_iter()
                        .map(|r| MyRenderElements::WaylandSurface(r))
                    })
                    .flatten()
                    .collect_vec();
                if let Some(buff) = self.buffer.as_mut() {
                    let mut render_context = buff.render();
                    let margin_offset = match self.config.anchor {
                        PanelAnchor::Top | PanelAnchor::Left => {
                            self.config.get_effective_anchor_gap() as f64
                        }
                        PanelAnchor::Bottom | PanelAnchor::Right => 0.0,
                    };

                    let (panel_size, loc) = if is_dock {
                        let loc: Point<f64, Logical> = if self.config.is_horizontal() {
                            (
                                ((self.dimensions.w - self.actual_size.w) as f64 / 2.0).floor(),
                                margin_offset,
                            )
                        } else {
                            (
                                margin_offset,
                                ((self.dimensions.h - self.actual_size.h) as f64 / 2.0).floor(),
                            )
                        }
                        .into();

                        (self.actual_size, loc)
                    } else {
                        let loc: Point<f64, Logical> = if self.config.is_horizontal() {
                            (0.0, margin_offset)
                        } else {
                            (margin_offset, 0.0)
                        }
                        .into();

                        (self.dimensions, loc)
                    };
                    let scaled_panel_size =
                        panel_size.to_f64().to_physical(self.scale).to_i32_round();

                    let _ = render_context.draw(|_| {
                        if self.buffer_changed {
                            Result::<_, ()>::Ok(vec![Rectangle::from_loc_and_size(
                                Point::default(),
                                (scaled_panel_size.w, scaled_panel_size.h),
                            )])
                        } else {
                            Result::<_, ()>::Ok(Default::default())
                        }
                    });
                    self.buffer_changed = false;

                    drop(render_context);
                    if let Ok(render_element) = MemoryRenderBufferRenderElement::from_buffer(
                        renderer,
                        loc.to_physical(self.scale).to_i32_round(),
                        &buff,
                        None,
                        None,
                        None,
                    ) {
                        elements.push(MyRenderElements::Memory(render_element));
                    }
                }

                let mut res: RenderOutputResult = my_renderer
                    .render_output(
                        renderer,
                        self.egl_surface
                            .as_ref()
                            .unwrap()
                            .buffer_age()
                            .unwrap_or_default() as usize,
                        &elements,
                        *clear_color,
                    )
                    .unwrap();
                self.egl_surface
                    .as_ref()
                    .unwrap()
                    .swap_buffers(res.damage.as_deref_mut())?;

                for window in self.space.elements() {
                    let output = o.clone();
                    window.send_frame(o, Duration::from_millis(time as u64), None, move |_, _| {
                        Some(output.clone())
                    });
                }
                let wl_surface = self.layer.as_ref().unwrap().wl_surface().clone();
                wl_surface.frame(qh, wl_surface.clone());
                wl_surface.commit();

                self.is_dirty = false;
                self.has_frame = false;
            }
        }

        let clear_color = [0.0, 0.0, 0.0, 0.0];
        // TODO Popup rendering optimization
        for p in self.popups.iter_mut().filter(|p| {
            p.dirty
                && p.egl_surface.is_some()
                && p.state.is_none()
                && p.s_surface.alive()
                && p.c_popup.wl_surface().is_alive()
                && p.has_frame
        }) {
            renderer.unbind()?;
            renderer.bind(p.egl_surface.as_ref().unwrap().clone())?;

            let elements: Vec<WaylandSurfaceRenderElement<_>> = render_elements_from_surface_tree(
                renderer,
                p.s_surface.wl_surface(),
                (0, 0),
                1.0,
                1.0,
            );
            p.damage_tracked_renderer.render_output(
                renderer,
                p.egl_surface
                    .as_ref()
                    .unwrap()
                    .buffer_age()
                    .unwrap_or_default() as usize,
                &elements,
                clear_color,
            )?;

            p.egl_surface.as_ref().unwrap().swap_buffers(None)?;

            let wl_surface = p.c_popup.wl_surface().clone();
            wl_surface.frame(qh, wl_surface.clone());
            wl_surface.commit();
            p.dirty = false;
            p.has_frame = false;
        }
        renderer.unbind()?;

        Ok(())
    }

    pub(crate) fn update_window_locations(&mut self) -> anyhow::Result<()> {
        self.space.refresh();
        let padding = self.config.padding();
        let anchor = self.config.anchor();
        let spacing = self.config.spacing();
        // First try partitioning the panel evenly into N spaces.
        // If all windows fit into each space, then set their offsets and return.
        let (list_length, list_thickness, actual_length) = match anchor {
            PanelAnchor::Left | PanelAnchor::Right => {
                (self.dimensions.h, self.dimensions.w, self.actual_size.h)
            }
            PanelAnchor::Top | PanelAnchor::Bottom => {
                (self.dimensions.w, self.dimensions.h, self.actual_size.w)
            }
        };
        let is_dock = !self.config.expand_to_edges();

        let mut num_lists = 0;
        if !is_dock && self.config.plugins_wings.is_some() {
            num_lists += 2;
        }
        if self.config.plugins_center.is_some() {
            num_lists += 1;
        }

        let mut windows_right = self
            .space
            .elements()
            .cloned()
            .filter(|w| w.alive())
            .filter_map(|w| {
                self.clients_right
                    .iter()
                    .enumerate()
                    .find_map(|(i, (_, c, _))| {
                        if Some(c.id()) == w.toplevel().wl_surface().client().map(|c| c.id()) {
                            Some((i, w.clone()))
                        } else {
                            None
                        }
                    })
            })
            .collect_vec();
        windows_right.sort_by(|(a_i, _), (b_i, _)| a_i.cmp(b_i));

        let mut windows_center = self
            .space
            .elements()
            .cloned()
            .filter(|w| w.alive())
            .filter_map(|w| {
                self.clients_center
                    .iter()
                    .enumerate()
                    .find_map(|(i, (_, c, _))| {
                        if Some(c.id()) == w.toplevel().wl_surface().client().map(|c| c.id()) {
                            Some((i, w.clone()))
                        } else {
                            None
                        }
                    })
            })
            .collect_vec();
        windows_center.sort_by(|(a_i, _), (b_i, _)| a_i.cmp(b_i));

        let mut windows_left = self
            .space
            .elements()
            .cloned()
            .filter(|w| w.alive())
            .filter_map(|w| {
                self.clients_left
                    .iter()
                    .enumerate()
                    .find_map(|(i, (_, c, _))| {
                        if Some(c.id()) == w.toplevel().wl_surface().client().map(|c| c.id()) {
                            Some((i, w.clone()))
                        } else {
                            None
                        }
                    })
            })
            .collect_vec();
        windows_left.sort_by(|(a_i, _), (b_i, _)| a_i.cmp(b_i));

        fn map_fn(
            (i, w): &(usize, Window),
            anchor: PanelAnchor,
            alignment: Alignment,
            scale: f64,
        ) -> (Alignment, usize, i32, i32) {
            // XXX this is a bit of a hack, but it works for now, and I'm not sure how to do it better
            let bbox = w
                .bbox()
                .to_f64()
                .to_physical(1.0)
                .to_logical(scale)
                .to_i32_round();

            match anchor {
                PanelAnchor::Left | PanelAnchor::Right => (alignment, *i, bbox.size.h, bbox.size.w),
                PanelAnchor::Top | PanelAnchor::Bottom => (alignment, *i, bbox.size.w, bbox.size.h),
            }
        }

        let left = windows_left
            .iter()
            .map(|e| map_fn(e, anchor, Alignment::Left, self.scale));
        let left_sum = left.clone().map(|(_, _, length, _)| length).sum::<i32>()
            + spacing as i32 * (windows_left.len().max(1) as i32 - 1);

        let center = windows_center
            .iter()
            .map(|e| map_fn(e, anchor, Alignment::Center, self.scale));
        let center_sum = center.clone().map(|(_, _, length, _)| length).sum::<i32>()
            + spacing as i32 * (windows_center.len().max(1) as i32 - 1);

        let right = windows_right
            .iter()
            .map(|e| map_fn(e, anchor, Alignment::Right, self.scale));

        let right_sum = right.clone().map(|(_, _, length, _)| length).sum::<i32>()
            + spacing as i32 * (windows_right.len().max(1) as i32 - 1);

        let total_sum = left_sum + center_sum + right_sum;
        let new_list_length =
            total_sum + padding as i32 * 2 + spacing as i32 * (num_lists as i32 - 1);
        let new_list_thickness: i32 = 2 * padding as i32
            + chain!(left.clone(), center.clone(), right.clone())
                .map(|(_, _, _, thickness)| thickness)
                .max()
                .unwrap_or(0);
        let mut new_dim: Size<i32, Logical> = match anchor {
            PanelAnchor::Left | PanelAnchor::Right => (new_list_thickness, new_list_length),
            PanelAnchor::Top | PanelAnchor::Bottom => (new_list_length, new_list_thickness),
        }
        .into();

        // update input region of panel when list length changes
        if actual_length != new_list_length && is_dock {
            let (input_region, layer) = match (self.input_region.as_ref(), self.layer.as_ref()) {
                (Some(r), Some(layer)) => (r, layer),
                _ => anyhow::bail!("Missing input region or layer!"),
            };
            let margin = self.config.get_effective_anchor_gap() as i32;
            let (w, h) = if self.config.is_horizontal() {
                (new_dim.w, new_dim.h + margin)
            } else {
                (new_dim.w + margin, new_dim.h)
            };
            input_region.subtract(0, 0, self.dimensions.w.max(w), self.dimensions.h.max(h));

            let (layer_length, _) = if self.config.is_horizontal() {
                (self.dimensions.w, self.dimensions.h)
            } else {
                (self.dimensions.h, self.dimensions.w)
            };

            if new_list_length < layer_length {
                let side = (layer_length as u32 - new_list_length as u32) / 2;

                // clear center
                let loc = if self.config.is_horizontal() {
                    (side as i32, 0)
                } else {
                    (0, side as i32)
                };

                input_region.add(loc.0, loc.1, w, h);
            } else {
                input_region.add(0, 0, self.dimensions.w.max(w), self.dimensions.h.max(h));
            }
            layer
                .wl_surface()
                .set_input_region(Some(input_region.wl_region()));
            layer.wl_surface().commit();
        }

        self.actual_size = match anchor {
            PanelAnchor::Left | PanelAnchor::Right => (new_list_thickness, new_list_length),
            PanelAnchor::Top | PanelAnchor::Bottom => (new_list_length, new_list_thickness),
        }
        .into();

        new_dim = self.constrain_dim(new_dim);

        let (new_list_dim_length, new_list_thickness_dim) = match anchor {
            PanelAnchor::Left | PanelAnchor::Right => (new_dim.h, new_dim.w),
            PanelAnchor::Top | PanelAnchor::Bottom => (new_dim.w, new_dim.h),
        };

        if new_list_dim_length != list_length as i32 || new_list_thickness_dim != list_thickness {
            self.pending_dimensions = Some(new_dim);
            self.is_dirty = true;
            anyhow::bail!("resizing list");
        }

        fn center_in_bar(thickness: u32, dim: u32) -> i32 {
            (thickness as i32 - dim as i32) / 2
        }

        let requested_eq_length: i32 = list_length / num_lists;
        let (right_sum, center_offset) = if is_dock {
            (0, padding as i32 + (list_length - new_list_length) / 2)
        } else if num_lists == 1 {
            (0, (requested_eq_length - center_sum) / 2)
        } else if left_sum <= requested_eq_length
            && center_sum <= requested_eq_length
            && right_sum <= requested_eq_length
        {
            let center_padding = (requested_eq_length - center_sum) / 2;
            (
                right_sum,
                requested_eq_length + padding as i32 + center_padding,
            )
        } else {
            let center_padding = (list_length as i32 - total_sum) / 2;

            (right_sum, left_sum + padding as i32 + center_padding)
        };

        let mut prev: u32 = padding;

        // offset for centering
        let margin_offset = match anchor {
            PanelAnchor::Top | PanelAnchor::Left => self.config.get_effective_anchor_gap(),
            PanelAnchor::Bottom | PanelAnchor::Right => 0,
        } as i32;

        for (i, w) in &mut windows_left.iter_mut() {
            // XXX this is a bit of a hack, but it works for now, and I'm not sure how to do it better
            let bbox = w
                .bbox()
                .to_f64()
                .to_physical(1.0)
                .to_logical(self.scale)
                .to_i32_round();
            let size: Point<i32, Logical> = (bbox.size.w, bbox.size.h).into();
            let cur: u32 = prev + spacing * *i as u32;
            match anchor {
                PanelAnchor::Left | PanelAnchor::Right => {
                    let cur = (
                        margin_offset
                            + center_in_bar(list_thickness.try_into().unwrap(), size.x as u32),
                        cur,
                    );
                    prev += size.y as u32;
                    self.space
                        .map_element(w.clone(), (cur.0 as i32, cur.1 as i32), false);
                }
                PanelAnchor::Top | PanelAnchor::Bottom => {
                    let cur = (
                        cur,
                        margin_offset
                            + center_in_bar(list_thickness.try_into().unwrap(), size.y as u32),
                    );
                    prev += size.x as u32;
                    self.space
                        .map_element(w.clone(), (cur.0 as i32, cur.1 as i32), false);
                }
            };
        }

        let mut prev: u32 = center_offset as u32;
        for (i, w) in &mut windows_center.iter_mut() {
            // XXX this is a bit of a hack, but it works for now, and I'm not sure how to do it better
            let bbox = w
                .bbox()
                .to_f64()
                .to_physical(1.0)
                .to_logical(self.scale)
                .to_i32_round();
            let size: Point<i32, Logical> = (bbox.size.w, bbox.size.h).into();
            let cur = prev + spacing * *i as u32;
            match anchor {
                PanelAnchor::Left | PanelAnchor::Right => {
                    let cur = (
                        margin_offset
                            + center_in_bar(list_thickness.try_into().unwrap(), size.x as u32),
                        cur,
                    );
                    prev += size.y as u32;
                    self.space
                        .map_element(w.clone(), (cur.0 as i32, cur.1 as i32), false);
                }
                PanelAnchor::Top | PanelAnchor::Bottom => {
                    let cur = (
                        cur,
                        margin_offset
                            + center_in_bar(list_thickness.try_into().unwrap(), size.y as u32),
                    );
                    prev += size.x as u32;
                    self.space
                        .map_element(w.clone(), (cur.0 as i32, cur.1 as i32), false);
                }
            };
        }

        // twice padding is subtracted
        let mut prev: u32 = list_length as u32 - padding - right_sum as u32;

        for (i, w) in &mut windows_right.iter_mut() {
            // XXX this is a bit of a hack, but it works for now, and I'm not sure how to do it better
            let bbox = w
                .bbox()
                .to_f64()
                .to_physical(1.0)
                .to_logical(self.scale)
                .to_i32_round();
            let size: Point<i32, Logical> = (bbox.size.w, bbox.size.h).into();
            let cur = prev + spacing * *i as u32;
            match anchor {
                PanelAnchor::Left | PanelAnchor::Right => {
                    let cur = (
                        margin_offset
                            + center_in_bar(list_thickness.try_into().unwrap(), size.x as u32),
                        cur,
                    );
                    prev += size.y as u32;
                    self.space
                        .map_element(w.clone(), (cur.0 as i32, cur.1 as i32), false);
                }
                PanelAnchor::Top | PanelAnchor::Bottom => {
                    let cur = (
                        cur,
                        margin_offset
                            + center_in_bar(list_thickness.try_into().unwrap(), size.y as u32),
                    );
                    prev += size.x as u32;
                    self.space
                        .map_element(w.clone(), (cur.0 as i32, cur.1 as i32), false);
                }
            };
        }
        self.space.refresh();
        if self.actual_size.w > 0
            && self.actual_size.h > 0
            && actual_length > 0
            && (self.config.border_radius > 0 || self.config.get_effective_anchor_gap() > 0)
        {
            // corners calculation with border_radius
            let panel_size = if is_dock {
                self.actual_size
                    .to_f64()
                    .to_physical(self.scale)
                    .to_i32_round()
            } else {
                self.dimensions
                    .to_f64()
                    .to_physical(self.scale)
                    .to_i32_round()
            };

            let mut buff = MemoryRenderBuffer::new(
                Fourcc::Abgr8888,
                (panel_size.w, panel_size.h),
                1,
                Transform::Normal,
                None,
            );
            let mut render_context = buff.render();
            let bg_color = self
                .bg_color
                .iter()
                .map(|c| ((c * 255.0) as u8).clamp(0, 255))
                .collect_vec();
            let _ = render_context.draw(|buffer| {
                buffer.chunks_exact_mut(4).for_each(|chunk| {
                    chunk.copy_from_slice(&bg_color);
                });

                let radius = (self.config.border_radius as f64 * self.scale).round() as u32;
                let radius = radius
                    .min(panel_size.w as u32 / 2)
                    .min(panel_size.h as u32 / 2);

                // early return if no radius
                if radius == 0 {
                    return Result::<_, ()>::Ok(vec![Rectangle::from_loc_and_size(
                        Point::default(),
                        (panel_size.w, panel_size.h),
                    )]);
                }
                let drawn_radius = 128;
                let drawn_radius2 = drawn_radius as f64 * drawn_radius as f64;
                let grid = (0..((drawn_radius + 1) * (drawn_radius + 1)))
                    .into_iter()
                    .map(|i| {
                        let (x, y) = (i as u32 % (drawn_radius + 1), i as u32 / (drawn_radius + 1));
                        drawn_radius2 - (x as f64 * x as f64 + y as f64 * y as f64)
                    })
                    .collect_vec();

                let bg_color: [u8; 4] = self
                    .bg_color
                    .iter()
                    .map(|c| ((c * 255.0) as u8).clamp(0, 255))
                    .collect_vec()
                    .try_into()
                    .unwrap();
                let empty = [0, 0, 0, 0];

                let mut corner_image = RgbaImage::new(drawn_radius, drawn_radius);
                for i in 0..(drawn_radius * drawn_radius) {
                    let (x, y) = (i as u32 / drawn_radius, i as u32 % drawn_radius);
                    let bottom_left = grid[(y * (drawn_radius + 1) + x) as usize];
                    let bottom_right = grid[(y * (drawn_radius + 1) + x + 1) as usize];
                    let top_left = grid[((y + 1) * (drawn_radius + 1) + x) as usize];
                    let top_right = grid[((y + 1) * (drawn_radius + 1) + x + 1) as usize];
                    let color = if bottom_left >= 0.0
                        && bottom_right >= 0.0
                        && top_left >= 0.0
                        && top_right >= 0.0
                    {
                        bg_color.clone()
                    } else {
                        empty
                    };
                    corner_image.put_pixel(x, y, image::Rgba(color));
                }
                let corner_image = image::imageops::resize(
                    &corner_image,
                    radius as u32,
                    radius as u32,
                    image::imageops::FilterType::CatmullRom,
                );

                for (i, color) in corner_image.pixels().enumerate() {
                    let (x, y) = (i as u32 % radius, i as u32 / radius);
                    let top_left = (radius - 1 - x, radius - 1 - y);
                    let top_right = (panel_size.w as u32 - radius + x, radius - 1 - y);
                    let bottom_left = (radius - 1 - x, panel_size.h as u32 - radius + y);
                    let bottom_right = (
                        panel_size.w as u32 - radius + x,
                        panel_size.h as u32 - radius + y,
                    );
                    for (c_x, c_y) in match (self.config.anchor, self.config.anchor_gap) {
                        (PanelAnchor::Left, false) => vec![top_right, bottom_right],
                        (PanelAnchor::Right, false) => vec![top_left, bottom_left],
                        (PanelAnchor::Top, false) => vec![bottom_left, bottom_right],
                        (PanelAnchor::Bottom, false) => vec![top_left, top_right],
                        _ => vec![top_left, top_right, bottom_left, bottom_right],
                    } {
                        let b_i = (c_y * panel_size.w as u32 + c_x) as usize * 4;
                        let c = buffer.get_mut(b_i..b_i + 4).unwrap();
                        c.copy_from_slice(&color.0);
                    }
                }

                // Return the whole buffer as damage
                Result::<_, ()>::Ok(vec![Rectangle::from_loc_and_size(
                    Point::default(),
                    (panel_size.w, panel_size.h),
                )])
            });
            drop(render_context);
            let old = self.buffer.replace(buff);
            self.old_buff = old;
            self.buffer_changed = true;
        }

        Ok(())
    }

    pub(crate) fn handle_events<W: WrapperSpace>(
        &mut self,
        _dh: &DisplayHandle,
        popup_manager: &mut PopupManager,
        time: u32,
        mut renderer: Option<&mut GlesRenderer>,
        qh: &QueueHandle<GlobalState<W>>,
    ) -> Instant {
        self.space.refresh();
        popup_manager.cleanup();

        self.handle_focus();
        let mut should_render = false;
        match self.space_event.take() {
            Some(SpaceEvent::Quit) => {
                info!("root layer shell surface removed.");
            }
            Some(SpaceEvent::WaitConfigure {
                first,
                width,
                height,
            }) => {
                self.space_event.replace(Some(SpaceEvent::WaitConfigure {
                    first,
                    width,
                    height,
                }));
            }
            None => {
                if let (Some(size), Some(layer_surface)) =
                    (self.pending_dimensions.take(), self.layer.as_ref())
                {
                    let width: u32 = size.w.try_into().unwrap();
                    let height: u32 = size.h.try_into().unwrap();
                    let margin = self.config.get_effective_anchor_gap() as u32;
                    if self.config.is_horizontal() {
                        layer_surface.set_size(0, height + margin);
                    } else {
                        layer_surface.set_size(width + margin, 0);
                    }
                    let list_thickness = match self.config.anchor() {
                        PanelAnchor::Left | PanelAnchor::Right => width + margin,
                        PanelAnchor::Top | PanelAnchor::Bottom => height + margin,
                    };

                    if self.config().autohide.is_some() {
                        if self.config.exclusive_zone() {
                            self.layer.as_ref().unwrap().set_exclusive_zone(
                                list_thickness as i32
                                    + self.config.get_hide_handle().unwrap() as i32,
                            );
                        }

                        let target =
                            self.config.get_hide_handle().unwrap() as i32 - list_thickness as i32;
                        Self::set_margin(
                            self.config.anchor,
                            self.config.get_margin() as i32,
                            target,
                            layer_surface,
                        );
                    } else if self.config.exclusive_zone() {
                        self.layer
                            .as_ref()
                            .unwrap()
                            .set_exclusive_zone(list_thickness as i32);
                        if self.config.margin > 0 {
                            Self::set_margin(
                                self.config.anchor,
                                self.config.margin as i32,
                                0,
                                layer_surface,
                            );
                        }
                    }
                    layer_surface.wl_surface().commit();
                    self.space_event.replace(Some(SpaceEvent::WaitConfigure {
                        first: false,
                        width: size.w,
                        height: size.h,
                    }));
                } else if self.layer.is_some() {
                    should_render = if self.is_dirty {
                        let update_res = self.update_window_locations();
                        update_res.is_ok()
                    } else {
                        true
                    };
                }
            }
        }

        if let Some(renderer) = renderer.as_mut() {
            let prev = self.popups.len();
            self.popups
                .retain_mut(|p: &mut WrapperPopup| p.handle_events(popup_manager));

            if prev == self.popups.len() && should_render {
                if let Err(e) = self.render(renderer, time, qh) {
                    error!("Failed to render, error: {:?}", e);
                }
            }
        }

        self.last_dirty.unwrap_or_else(Instant::now)
    }

    pub fn configure_panel_layer(
        &mut self,
        layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        renderer: &mut Option<GlesRenderer>,
    ) {
        let (w, h) = configure.new_size;
        match self.space_event.take() {
            Some(e) => match e {
                SpaceEvent::WaitConfigure {
                    first,
                    mut width,
                    mut height,
                } => {
                    let _ = self.spawn_clients(self.s_display.clone().unwrap());
                    if w != 0 {
                        width = w as i32;
                        if self.config.is_horizontal() {
                            self.suggested_length.replace(w);
                        }
                    }
                    if h != 0 {
                        height = h as i32;
                        if !self.config.is_horizontal() {
                            self.suggested_length.replace(h);
                        }
                    }

                    if width <= 0 {
                        width = 1;
                    }
                    if height <= 0 {
                        height = 1;
                    }
                    let dim = self.constrain_dim((width as i32, height as i32).into());

                    let (panel_width, panel_height) = if self.config.is_horizontal() {
                        (
                            width,
                            height - self.config.get_effective_anchor_gap() as i32,
                        )
                    } else {
                        (
                            width - self.config.get_effective_anchor_gap() as i32,
                            height,
                        )
                    };

                    if first {
                        let client_egl_surface = unsafe {
                            ClientEglSurface::new(
                                WlEglSurface::new(
                                    self.layer.as_ref().unwrap().wl_surface().id(),
                                    dim.w,
                                    dim.h,
                                )
                                .unwrap(), // TODO remove unwrap
                                self.layer.as_ref().unwrap().wl_surface().clone(),
                            )
                        };
                        let new_egl_display = if let Some(renderer) = renderer.as_ref() {
                            renderer.egl_context().display().clone()
                        } else {
                            let client_egl_display = ClientEglDisplay {
                                display: self.c_display.as_ref().unwrap().clone(),
                            };
                            EGLDisplay::new(client_egl_display)
                                .expect("Failed to create EGL display")
                        };

                        let egl_context = EGLContext::new_with_config(
                            &new_egl_display,
                            GlAttributes {
                                version: (3, 0),
                                profile: None,
                                debug: cfg!(debug_assertions),
                                vsync: false,
                            },
                            PixelFormatRequirements::_8_bit(),
                        )
                        .unwrap_or_else(|_| {
                            EGLContext::new_with_config(
                                &new_egl_display,
                                GlAttributes {
                                    version: (2, 0),
                                    profile: None,
                                    debug: cfg!(debug_assertions),
                                    vsync: false,
                                },
                                PixelFormatRequirements::_8_bit(),
                            )
                            .expect("Failed to create EGL context")
                        });

                        let mut new_renderer = if let Some(renderer) = renderer.take() {
                            renderer
                        } else {
                            unsafe {
                                GlesRenderer::new(egl_context)
                                    .expect("Failed to create EGL Surface")
                            }
                        };

                        let egl_surface = Rc::new(unsafe {
                            EGLSurface::new(
                                &new_egl_display,
                                new_renderer
                                    .egl_context()
                                    .pixel_format()
                                    .expect("Failed to get pixel format from EGL context "),
                                new_renderer.egl_context().config_id(),
                                client_egl_surface,
                            )
                            .expect("Failed to create EGL Surface")
                        });

                        // bind before setting swap interval
                        let _ = new_renderer.unbind();
                        let _ = new_renderer.bind(egl_surface.clone());
                        let swap_success =
                            unsafe { SwapInterval(new_egl_display.get_display_handle().handle, 0) }
                                == 1;
                        if !swap_success {
                            error!("Failed to set swap interval");
                        }
                        let _ = new_renderer.unbind();

                        renderer.replace(new_renderer);
                        self.egl_surface.replace(egl_surface);
                    } else if self.dimensions != (panel_width, panel_height).into()
                        && self.pending_dimensions.is_none()
                    {
                        if let (Some(renderer), Some(egl_surface)) =
                            (renderer.as_mut(), self.egl_surface.as_ref())
                        {
                            let _ = renderer.unbind();
                            let scaled_size = dim.to_f64().to_physical(self.scale).to_i32_round();
                            let _ = renderer.bind(egl_surface.clone());
                            egl_surface.resize(scaled_size.w, scaled_size.h, 0, 0);
                            let _ = renderer.unbind();
                            if let Some(viewport) = self.layer_viewport.as_ref() {
                                viewport.set_destination(dim.w.max(1), dim.h.max(1));
                                layer.wl_surface().commit();
                            }
                        }
                    }

                    self.dimensions = (panel_width, panel_height).into();
                    self.damage_tracked_renderer = Some(OutputDamageTracker::new(
                        dim.to_f64().to_physical(self.scale).to_i32_round(),
                        1.0,
                        smithay::utils::Transform::Flipped180,
                    ));
                    self.layer.as_ref().unwrap().wl_surface().commit();
                }
                SpaceEvent::Quit => (),
            },
            None => {
                let mut width = self.dimensions.w;
                let mut height = self.dimensions.h;
                if w != 0 {
                    width = w as i32;
                    if self.config.is_horizontal() {
                        self.suggested_length.replace(w);
                    }
                }
                if h != 0 {
                    height = h as i32;
                    if !self.config.is_horizontal() {
                        self.suggested_length.replace(h);
                    }
                }
                if width == 0 {
                    width = 1;
                }
                if height == 0 {
                    height = 1;
                }
                let dim = self.constrain_dim((width as i32, height as i32).into());
                let (panel_width, panel_height) = if self.config.is_horizontal() {
                    (
                        width,
                        height - self.config.get_effective_anchor_gap() as i32,
                    )
                } else {
                    (
                        height - self.config.get_effective_anchor_gap() as i32,
                        width,
                    )
                };
                if let (Some(renderer), Some(egl_surface)) =
                    (renderer.as_mut(), self.egl_surface.as_ref())
                {
                    let _ = renderer.unbind();
                    let _ = renderer.bind(egl_surface.clone());
                    let scaled_size = dim.to_f64().to_physical(self.scale).to_i32_round();
                    egl_surface.resize(scaled_size.w, scaled_size.h, 0, 0);
                    let _ = renderer.unbind();
                    if let Some(viewport) = self.layer_viewport.as_ref() {
                        viewport.set_destination(dim.w.max(1), dim.h.max(1));
                        layer.wl_surface().commit();
                    }
                }
                self.dimensions = (panel_width, panel_height).into();
                self.damage_tracked_renderer = Some(OutputDamageTracker::new(
                    dim.to_f64().to_physical(self.scale).to_i32_round(),
                    1.0,
                    smithay::utils::Transform::Flipped180,
                ));
                self.layer.as_ref().unwrap().wl_surface().commit();
            }
        }
    }

    pub fn configure_panel_popup(
        &mut self,
        popup: &sctk::shell::xdg::popup::Popup,
        config: sctk::shell::xdg::popup::PopupConfigure,
        renderer: Option<&mut GlesRenderer>,
    ) {
        let Some(renderer)= renderer else {
            return;
        };

        if let Some(p) = self
            .popups
            .iter_mut()
            .find(|p| popup.wl_surface() == p.c_popup.wl_surface())
        {
            // use the size that we have already
            p.wrapper_rectangle =
                Rectangle::from_loc_and_size(config.position, (config.width, config.height));

            let (width, height) = (config.width, config.height);
            p.state = match p.state {
                None | Some(WrapperPopupState::WaitConfigure) => None,
                Some(r) => Some(r),
            };

            let _ = p.s_surface.send_configure();
            match config.kind {
                popup::ConfigureKind::Initial => {
                    let wl_egl_surface =
                        match WlEglSurface::new(p.c_popup.wl_surface().id(), width, height) {
                            Ok(s) => s,
                            Err(_) => return,
                        };
                    let client_egl_surface = unsafe {
                        ClientEglSurface::new(wl_egl_surface, p.c_popup.wl_surface().clone())
                    };
                    let egl_surface = Rc::new(unsafe {
                        EGLSurface::new(
                            renderer.egl_context().display(),
                            renderer
                                .egl_context()
                                .pixel_format()
                                .expect("Failed to get pixel format from EGL context "),
                            renderer.egl_context().config_id(),
                            client_egl_surface,
                        )
                        .expect("Failed to initialize EGL Surface")
                    });
                    p.egl_surface.replace(egl_surface);
                    p.dirty = true;
                }
                popup::ConfigureKind::Reactive => {}
                popup::ConfigureKind::Reposition { token: _token } => {}
                _ => {}
            };
        }
    }

    pub fn set_theme_window_color(&mut self, mut color: [f32; 4]) {
        if let CosmicPanelBackground::ThemeDefault = self.config.background {
            color[3] = self.config.opacity;
        }
        self.bg_color = color;
        self.clear();
    }

    /// clear the panel
    pub fn clear(&mut self) {
        self.is_dirty = true;
        self.popups.clear();
        self.damage_tracked_renderer = Some(OutputDamageTracker::new(
            self.dimensions
                .to_f64()
                .to_physical(self.scale)
                .to_i32_round(),
            1.0,
            smithay::utils::Transform::Flipped180,
        ));
    }

    pub fn apply_positioner_state(
        &self,
        positioner: &XdgPositioner,
        pos_state: PositionerState,
        s_surface: &PopupSurface,
    ) {
        let PositionerState {
            rect_size,
            anchor_rect,
            anchor_edges,
            gravity,
            constraint_adjustment,
            offset,
            reactive,
            parent_size,
            parent_configure: _,
        } = pos_state;
        let parent_window = if let Some(s) = self
            .space
            .elements()
            .find(|w| w.wl_surface() == s_surface.get_parent_surface().as_ref().cloned())
        {
            s
        } else {
            return;
        };

        let p_offset = self
            .space
            .element_location(parent_window)
            .unwrap_or_else(|| (0, 0).into());

        positioner.set_size(rect_size.w.max(1), rect_size.h.max(1));
        positioner.set_anchor_rect(
            anchor_rect.loc.x + p_offset.x,
            anchor_rect.loc.y + p_offset.y,
            anchor_rect.size.w,
            anchor_rect.size.h,
        );
        positioner.set_anchor(Anchor::try_from(anchor_edges as u32).unwrap_or(Anchor::None));
        positioner.set_gravity(Gravity::try_from(gravity as u32).unwrap_or(Gravity::None));

        positioner.set_constraint_adjustment(u32::from(constraint_adjustment));
        positioner.set_offset(offset.x, offset.y);
        if positioner.version() >= 3 {
            if reactive {
                positioner.set_reactive();
            }
            if let Some(parent_size) = parent_size {
                positioner.set_parent_size(parent_size.w, parent_size.h);
            }
        }
    }
}

impl Drop for PanelSpace {
    fn drop(&mut self) {
        // request processes to stop
        if let Some(id) = self.layer.as_ref().map(|l| l.wl_surface().id()) {
            let _ = self.applet_tx.try_send(AppletMsg::Cleanup(id));
        }
    }
}
