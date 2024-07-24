use std::time::Duration;

use crate::iced::elements::{CosmicMappedInternal, PopupMappedInternal};

use super::{
    corner_element::{RoundedRectangleShader, RoundedRectangleShaderElement},
    layout::OverflowSection,
    PanelSpace,
};
use cctk::wayland_client::{Proxy, QueueHandle};
use itertools::Itertools;

use crate::xdg_shell_wrapper::shared_state::GlobalState;
use sctk::shell::WaylandSurface;
use smithay::{
    backend::renderer::{
        damage::OutputDamageTracker,
        element::{
            memory::MemoryRenderBufferRenderElement,
            surface::{render_elements_from_surface_tree, WaylandSurfaceRenderElement},
            utils::CropRenderElement,
            AsRenderElements, RenderElement, UnderlyingStorage,
        },
        gles::{GlesError, GlesFrame, GlesRenderer},
        Bind, Frame, Renderer, Unbind,
    },
    utils::{Buffer, Physical, Rectangle},
};
pub(crate) enum PanelRenderElement {
    Wayland(WaylandSurfaceRenderElement<GlesRenderer>),
    Crop(CropRenderElement<WaylandSurfaceRenderElement<GlesRenderer>>),
    RoundedRectangle(RoundedRectangleShaderElement),
    Iced(MemoryRenderBufferRenderElement<GlesRenderer>),
}

impl smithay::backend::renderer::element::Element for PanelRenderElement {
    fn id(&self) -> &smithay::backend::renderer::element::Id {
        match self {
            Self::Wayland(e, ..) => e.id(),
            Self::Crop(e) => e.id(),
            Self::RoundedRectangle(e) => e.id(),
            Self::Iced(e) => e.id(),
        }
    }

    fn current_commit(&self) -> smithay::backend::renderer::utils::CommitCounter {
        match self {
            Self::Wayland(e, ..) => e.current_commit(),
            Self::Crop(e) => e.current_commit(),
            Self::RoundedRectangle(e) => e.current_commit(),
            Self::Iced(e) => e.current_commit(),
        }
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        match self {
            Self::Wayland(e) => e.src(),
            Self::Crop(e) => e.src(),
            Self::RoundedRectangle(e) => e.src(),
            Self::Iced(e) => e.src(),
        }
    }

    fn geometry(&self, scale: smithay::utils::Scale<f64>) -> Rectangle<i32, Physical> {
        match self {
            Self::Wayland(e) => e.geometry(scale),
            Self::Crop(e) => e.geometry(scale),
            Self::RoundedRectangle(e) => e.geometry(scale),
            Self::Iced(e) => e.geometry(scale),
        }
    }
}

impl RenderElement<GlesRenderer> for PanelRenderElement {
    fn draw(
        &self,
        frame: &mut GlesFrame<'_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), GlesError> {
        match self {
            Self::Wayland(e, ..) => e.draw(frame, src, dst, damage, opaque_regions),
            Self::Crop(e) => e.draw(frame, src, dst, damage, opaque_regions),
            Self::RoundedRectangle(e) => e.draw(frame, src, dst, damage, opaque_regions),
            Self::Iced(e) => e.draw(frame, src, dst, damage, opaque_regions),
        }
    }

    fn underlying_storage(&self, renderer: &mut GlesRenderer) -> Option<UnderlyingStorage> {
        match self {
            PanelRenderElement::Wayland(e, ..) => e.underlying_storage(renderer),
            PanelRenderElement::Crop(e) => e.underlying_storage(renderer),
            PanelRenderElement::RoundedRectangle(e) => e.underlying_storage(renderer),
            PanelRenderElement::Iced(e) => e.underlying_storage(renderer),
        }
    }
}

impl PanelSpace {
    pub(crate) fn render(
        &mut self,
        renderer: &mut GlesRenderer,
        time: u32,
        qh: &QueueHandle<GlobalState>,
    ) -> anyhow::Result<()> {
        if self.space_event.get().is_some()
            && (self.actual_size.w <= 20 || self.actual_size.h <= 20)
        {
            return Ok(());
        }

        let clear_color = [0., 0., 0., 0.];

        if self.is_dirty && self.has_frame {
            tracing::trace!("Rendering space");
            let my_renderer = match self.damage_tracked_renderer.as_mut() {
                Some(r) => r,
                None => return Ok(()),
            };
            renderer.unbind()?;
            renderer.bind(self.egl_surface.as_ref().unwrap().clone())?;
            // if not visible, just clear and exit early
            let not_visible = self.config.autohide.is_some()
                && matches!(self.visibility, crate::xdg_shell_wrapper::space::Visibility::Hidden);
            let dim = self.dimensions.to_f64().to_physical(self.scale).to_i32_round();
            // TODO check to make sure this is not going to cause damage issues
            if not_visible {
                if let Ok(mut frame) = renderer.render(dim, smithay::utils::Transform::Normal) {
                    _ = frame
                        .clear([0.0, 0.0, 0.0, 0.0], &[Rectangle::from_loc_and_size((0, 0), dim)]);
                    if let Ok(sync_point) = frame.finish() {
                        if let Err(err) = sync_point.wait() {
                            tracing::error!("Error waiting for sync point: {:?}", err);
                        }
                        self.egl_surface.as_ref().unwrap().swap_buffers(None)?;
                    }
                    let wl_surface = self.layer.as_ref().unwrap().wl_surface();
                    wl_surface.frame(qh, wl_surface.clone());
                    wl_surface.commit();
                    // reset the damage tracker
                    *my_renderer =
                        OutputDamageTracker::new(dim, 1.0, smithay::utils::Transform::Flipped180);
                }

                renderer.unbind()?;
                self.is_dirty = false;
                self.has_frame = false;
                return Ok(());
            }

            if let Some((o, _info)) = &self.output.as_ref().map(|(_, o, info)| (o, info)) {
                let mut elements: Vec<PanelRenderElement> = (self.config.anchor_gap
                    || self.config.border_radius > 0)
                    .then(|| {
                        PanelRenderElement::RoundedRectangle(RoundedRectangleShader::element(
                            renderer,
                            Rectangle::from_loc_and_size((0, 0), dim.to_logical(1)),
                            self.panel_rect_settings,
                        ))
                    })
                    .into_iter()
                    .chain(
                        self.space
                            .elements()
                            .filter_map(|w| {
                                let loc = self
                                    .space
                                    .element_location(w)
                                    .unwrap_or_default()
                                    .to_f64()
                                    .to_physical(self.scale)
                                    .to_i32_round();

                                if let CosmicMappedInternal::OverflowButton(b) = w {
                                    return Some(
                                        b.render_elements(renderer, loc, self.scale.into(), 1.0)
                                            .into_iter()
                                            .map(PanelRenderElement::Iced)
                                            .collect::<Vec<_>>(),
                                    );
                                }
                                w.toplevel().map(|t| {
                                    let configured_size = t.current_state().size.map(|s| {
                                        let mut r = Rectangle::from_loc_and_size(
                                            self.space
                                                .element_location(w)
                                                .unwrap_or_default()
                                                .to_f64()
                                                .to_physical_precise_round(self.scale),
                                            s.to_f64().to_physical_precise_round(self.scale),
                                        );
                                        if r.size.w == 0 {
                                            r.size.w = i32::MAX;
                                        }
                                        if r.size.h == 0 {
                                            r.size.h = i32::MAX;
                                        }
                                        r
                                    });

                                    render_elements_from_surface_tree(
                                        renderer,
                                        t.wl_surface(),
                                        loc,
                                        1.0,
                                        1.0,
                                        smithay::backend::renderer::element::Kind::Unspecified,
                                    )
                                    .into_iter()
                                    .filter_map(|r: WaylandSurfaceRenderElement<GlesRenderer>| {
                                        if let Some(configured_size) = configured_size {
                                            return CropRenderElement::from_element(
                                                r,
                                                1.,
                                                configured_size,
                                            )
                                            .map(PanelRenderElement::Crop);
                                        }

                                        Some(PanelRenderElement::Wayland(r))
                                    })
                                    .collect::<Vec<_>>()
                                })
                            })
                            .flatten(),
                    )
                    .collect_vec();

                if let Some(bg) = self.background_element.as_ref().map(|e| {
                    let pos = e.with_program(|p| p.logical_pos);
                    e.render_elements(renderer, pos.into(), self.scale.into(), 1.0)
                        .into_iter()
                        .map(PanelRenderElement::Iced)
                }) {
                    elements.extend(bg);
                };

                _ = my_renderer.render_output(
                    renderer,
                    self.egl_surface.as_ref().unwrap().buffer_age().unwrap_or_default() as usize,
                    &elements,
                    clear_color,
                );

                self.egl_surface.as_ref().unwrap().swap_buffers(None)?;

                for window in self.space.elements().filter_map(|w| {
                    if let CosmicMappedInternal::Window(w) = w {
                        Some(w)
                    } else {
                        None
                    }
                }) {
                    let output = *o;
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
            p.popup.dirty
                && p.popup.egl_surface.is_some()
                && p.popup.state.is_none()
                && p.s_surface.alive()
                && p.popup.c_popup.wl_surface().is_alive()
                && p.popup.has_frame
        }) {
            renderer.unbind()?;
            renderer.bind(p.popup.egl_surface.as_ref().unwrap().clone())?;

            let elements: Vec<WaylandSurfaceRenderElement<_>> = render_elements_from_surface_tree(
                renderer,
                p.s_surface.wl_surface(),
                (0, 0),
                1.0,
                1.0,
                smithay::backend::renderer::element::Kind::Unspecified,
            );
            p.popup.damage_tracked_renderer.render_output(
                renderer,
                p.popup.egl_surface.as_ref().unwrap().buffer_age().unwrap_or_default() as usize,
                &elements,
                clear_color,
            )?;

            p.popup.egl_surface.as_ref().unwrap().swap_buffers(None)?;

            let wl_surface = p.popup.c_popup.wl_surface().clone();
            wl_surface.frame(qh, wl_surface.clone());
            wl_surface.commit();
            p.popup.dirty = false;
            p.popup.has_frame = false;
        }

        // render to overflow_popup
        if let Some((ref mut p, section)) = self.overflow_popup.as_mut().filter(|(p, _)| {
            p.dirty
                && p.egl_surface.is_some()
                && p.state.is_none()
                && p.c_popup.wl_surface().is_alive()
        }) {
            renderer.unbind()?;
            renderer.bind(p.egl_surface.as_ref().unwrap().clone())?;
            let space = match section {
                OverflowSection::Center => &self.overflow_center,
                OverflowSection::Left => &self.overflow_left,
                OverflowSection::Right => &self.overflow_right,
            };
            let mut bg_render_element = None;
            let mut elements: Vec<PanelRenderElement> = space
                .elements()
                .filter_map(|e| match e {
                    crate::iced::elements::PopupMappedInternal::Popup(e) => {
                        // move to bg_render_element
                        bg_render_element = Some(
                            e.render_elements(renderer, (0, 0).into(), self.scale.into(), 1.0)
                                .into_iter()
                                .map(PanelRenderElement::Iced)
                                .collect::<Vec<_>>(),
                        );
                        None
                    },
                    crate::iced::elements::PopupMappedInternal::Window(w) => {
                        let Some(t) = w.toplevel() else {
                            return None;
                        };

                        let loc = space
                            .element_location(&PopupMappedInternal::Window(w.clone()))
                            .unwrap_or_default()
                            .to_f64()
                            .to_physical(self.scale)
                            .to_i32_round();

                        let configured_size = t.current_state().size.map(|s| {
                            let mut r = Rectangle::from_loc_and_size(
                                loc,
                                s.to_f64().to_physical_precise_round(self.scale),
                            );
                            if r.size.w == 0 {
                                r.size.w = i32::MAX;
                            }
                            if r.size.h == 0 {
                                r.size.h = i32::MAX;
                            }
                            r
                        });
                        Some(
                            render_elements_from_surface_tree(
                                renderer,
                                t.wl_surface(),
                                loc,
                                1.0,
                                1.0,
                                smithay::backend::renderer::element::Kind::Unspecified,
                            )
                            .into_iter()
                            .filter_map(|r: WaylandSurfaceRenderElement<GlesRenderer>| {
                                if let Some(configured_size) = configured_size {
                                    return CropRenderElement::from_element(r, 1., configured_size)
                                        .map(PanelRenderElement::Crop);
                                }

                                Some(PanelRenderElement::Wayland(r))
                            })
                            .collect::<Vec<_>>(),
                        )
                    },
                    crate::iced::elements::PopupMappedInternal::_GenericCatcher(_) => None,
                })
                .flatten()
                .collect();

            elements.extend(bg_render_element.unwrap_or_default());

            _ = p.damage_tracked_renderer.render_output(
                renderer,
                p.egl_surface.as_ref().unwrap().buffer_age().unwrap_or_default() as usize,
                &elements,
                clear_color,
            );

            p.egl_surface.as_ref().unwrap().swap_buffers(None)?;
            let wl_surface = p.c_popup.wl_surface();
            wl_surface.frame(qh, wl_surface.clone());
            wl_surface.commit();
        }

        renderer.unbind()?;

        Ok(())
    }
}
