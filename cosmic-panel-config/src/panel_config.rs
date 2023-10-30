//! Config for cosmic-panel

use std::{fmt::Display, ops::Range, str::FromStr, time::Duration};

use anyhow::bail;
use cosmic_config::{cosmic_config_derive::CosmicConfigEntry, Config, CosmicConfigEntry};
use serde::{Deserialize, Serialize};
#[cfg(feature = "wayland-rs")]
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};
#[cfg(feature = "wayland-rs")]
use xdg_shell_wrapper_config::{KeyboardInteractivity, Layer, WrapperConfig, WrapperOutput};

use crate::{NAME, VERSION};

/// Edge to which the panel is anchored
#[derive(Debug, Deserialize, Serialize, Copy, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub enum PanelAnchor {
    /// anchored to left edge
    Left,
    /// anchored to right edge
    Right,
    /// anchored to top edge
    Top,
    /// anchored to bottom edge
    Bottom,
}

impl Display for PanelAnchor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PanelAnchor::Left => write!(f, "Left"),
            PanelAnchor::Right => write!(f, "Right"),
            PanelAnchor::Top => write!(f, "Top"),
            PanelAnchor::Bottom => write!(f, "Bottom"),
        }
    }
}

impl FromStr for PanelAnchor {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Left" => Ok(Self::Left),
            "Right" => Ok(Self::Right),
            "Top" => Ok(Self::Top),
            "Bottom" => Ok(Self::Bottom),
            _ => Err(anyhow::anyhow!("Not a valid PanelAnchor")),
        }
    }
}

impl Default for PanelAnchor {
    fn default() -> Self {
        PanelAnchor::Top
    }
}

#[cfg(feature = "wayland-rs")]
impl TryFrom<zwlr_layer_surface_v1::Anchor> for PanelAnchor {
    type Error = anyhow::Error;
    fn try_from(align: zwlr_layer_surface_v1::Anchor) -> Result<Self, Self::Error> {
        if align.contains(zwlr_layer_surface_v1::Anchor::Left) {
            Ok(Self::Left)
        } else if align.contains(zwlr_layer_surface_v1::Anchor::Right) {
            Ok(Self::Right)
        } else if align.contains(zwlr_layer_surface_v1::Anchor::Top) {
            Ok(Self::Top)
        } else if align.contains(zwlr_layer_surface_v1::Anchor::Bottom) {
            Ok(Self::Bottom)
        } else {
            anyhow::bail!("Invalid Anchor")
        }
    }
}

#[cfg(feature = "wayland-rs")]
impl Into<zwlr_layer_surface_v1::Anchor> for PanelAnchor {
    fn into(self) -> zwlr_layer_surface_v1::Anchor {
        let mut anchor = zwlr_layer_surface_v1::Anchor::empty();
        match self {
            Self::Left => {
                anchor.insert(zwlr_layer_surface_v1::Anchor::Left);
            }
            Self::Right => {
                anchor.insert(zwlr_layer_surface_v1::Anchor::Right);
            }
            Self::Top => {
                anchor.insert(zwlr_layer_surface_v1::Anchor::Top);
            }
            Self::Bottom => {
                anchor.insert(zwlr_layer_surface_v1::Anchor::Bottom);
            }
        };
        anchor
    }
}

/// Configurable size for the cosmic panel
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub enum PanelSize {
    /// XS
    XS,
    /// S
    S,
    /// M
    M,
    /// L
    L,
    /// XL
    XL,
}

impl Display for PanelSize {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PanelSize::XS => write!(f, "XS"),
            PanelSize::S => write!(f, "S"),
            PanelSize::M => write!(f, "M"),
            PanelSize::L => write!(f, "L"),
            PanelSize::XL => write!(f, "XL"),
        }
    }
}

impl FromStr for PanelSize {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "XS" => Ok(Self::XS),
            "S" => Ok(Self::S),
            "M" => Ok(Self::M),
            "L" => Ok(Self::L),
            "XL" => Ok(Self::XL),
            _ => Err(anyhow::anyhow!("Not a valid PanelSize")),
        }
    }
}

/// configurable backgrounds for the cosmic panel
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub enum CosmicPanelBackground {
    /// theme default color with optional transparency
    ThemeDefault,
    /// theme default dark
    Dark,
    /// theme default light
    Light,
    /// RGBA
    Color([f32; 3]),
}

// TODO configurable interpolation type?
/// configurable autohide behavior
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AutoHide {
    /// time in milliseconds without pointer focus before hiding
    pub wait_time: u32,
    /// time in milliseconds that it should take to transition
    pub transition_time: u32,
    /// size of the handle in pixels
    /// should be > 0
    pub handle_size: u32,
}

impl Default for AutoHide {
    fn default() -> Self {
        Self {
            wait_time: 1000,
            transition_time: 200,
            handle_size: 4,
        }
    }
}

/// Configuration for the panel's ouput
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub enum CosmicPanelOuput {
    /// show panel on all outputs
    All,
    /// show panel on the active output
    Active,
    /// show panel on a specific output
    Name(String),
}

impl Display for CosmicPanelOuput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CosmicPanelOuput::All => write!(f, "All"),
            CosmicPanelOuput::Active => write!(f, "Active"),
            CosmicPanelOuput::Name(n) => write!(f, "Name({})", n),
        }
    }
}

impl FromStr for CosmicPanelOuput {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "All" => Ok(Self::All),
            "Active" => Ok(Self::Active),
            s if s.len() >= 6 && &s[..5] == "Name(" && s.ends_with(')') => {
                Ok(Self::Name(s[5..s.len() - 1].to_string()))
            }
            _ => bail!("Failed to parse output."),
        }
    }
}

#[cfg(feature = "wayland-rs")]
impl Into<WrapperOutput> for CosmicPanelOuput {
    fn into(self) -> WrapperOutput {
        match self {
            CosmicPanelOuput::All => WrapperOutput::All,
            CosmicPanelOuput::Active => WrapperOutput::Name(vec![]),
            CosmicPanelOuput::Name(n) => WrapperOutput::Name(vec![n]),
        }
    }
}

#[cfg(feature = "wayland-rs")]
// TODO refactor to have separate dock mode config & panel mode config
/// Config structure for the cosmic panel
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, CosmicConfigEntry)]
#[serde(deny_unknown_fields)]
pub struct CosmicPanelConfig {
    /// profile name for this config, should be unique
    pub name: String,
    /// edge which the panel is locked to
    pub anchor: PanelAnchor,
    /// gap between the panel and the edge of the ouput
    pub anchor_gap: bool,
    /// configured layer which the panel is on
    pub layer: Layer,
    /// configured interactivity level for the panel
    pub keyboard_interactivity: KeyboardInteractivity,
    /// configured size for the panel
    pub size: PanelSize,
    /// name of configured output (Intended for dock or panel), or None to place on active output (Intended for wrapping a single application)
    pub output: CosmicPanelOuput,
    /// customized background, or
    pub background: CosmicPanelBackground,
    /// list of plugins on the left / top and right / bottom of the panel
    pub plugins_wings: Option<(Vec<String>, Vec<String>)>,
    /// list of plugins in the center of the panel
    pub plugins_center: Option<Vec<String>>,
    /// whether the panel should stretch to the edges of output
    pub expand_to_edges: bool,
    /// padding around the panel
    pub padding: u32,
    /// space between panel plugins
    pub spacing: u32,
    pub border_radius: u32,
    // TODO autohide & exclusive zone should not be able to both be enabled at once
    /// exclusive zone
    pub exclusive_zone: bool,
    /// enable autohide feature with the transitions lasting the supplied wait time and duration in millis
    pub autohide: Option<AutoHide>,
    /// margin between the panel and the edge of the output
    pub margin: u16,
    /// opacity of the panel
    pub opacity: f32,
}

#[cfg(feature = "wayland-rs")]
impl Default for CosmicPanelConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            anchor: PanelAnchor::Top,
            anchor_gap: false,
            layer: Layer::Top,
            keyboard_interactivity: KeyboardInteractivity::None,
            size: PanelSize::M,
            output: CosmicPanelOuput::All,
            background: CosmicPanelBackground::ThemeDefault,
            plugins_wings: Default::default(),
            plugins_center: Default::default(),
            expand_to_edges: true,
            padding: 4,
            spacing: 4,
            exclusive_zone: true,
            autohide: None,
            border_radius: 8,
            margin: 4,
            opacity: 0.8,
        }
    }
}

#[cfg(feature = "wayland-rs")]
impl CosmicPanelConfig {
    /// get applet icon dimensions
    pub fn get_applet_icon_size(&self) -> u32 {
        match &self.size {
            PanelSize::XS => 18,
            PanelSize::S => 24,
            PanelSize::M => 36,
            PanelSize::L => 48,
            PanelSize::XL => 64,
        }
    }

    /// get the priority of the panel
    /// higher priority panels will be created first and given more space when competing for space
    pub fn get_priority(&self) -> u32 {
        let mut priority = if self.expand_to_edges() { 1000 } else { 0 };
        if self.margin == 0 {
            priority += 200;
        }
        if !self.anchor_gap {
            priority += 100;
        }
        if self.name.to_lowercase().contains("panel") {
            priority += 10;
        }
        priority
    }

    /// get margin between the panel and the edge of the output
    pub fn get_margin(&self) -> u16 {
        self.margin
    }

    /// get the effective anchor gap margin
    pub fn get_effective_anchor_gap(&self) -> u32 {
        if self.anchor_gap {
            self.margin as u32
        } else {
            0
        }
    }

    /// if autohide is configured, returns the duration of time which the panel should wait to hide when it has lost focus
    pub fn get_hide_wait(&self) -> Option<Duration> {
        self.autohide
            .as_ref()
            .map(|AutoHide { wait_time, .. }| Duration::from_millis((*wait_time).into()))
    }

    /// if autohide is configured, returns the duration of time which the panel hide / show transition should last
    pub fn get_hide_transition(&self) -> Option<Duration> {
        self.autohide.as_ref().map(
            |AutoHide {
                 transition_time, ..
             }| Duration::from_millis((*transition_time).into()),
        )
    }

    /// if autohide is configured, returns the size of the handle of the panel which should be exposed
    pub fn get_hide_handle(&self) -> Option<u32> {
        self.autohide
            .as_ref()
            .map(|AutoHide { handle_size, .. }| *handle_size)
    }

    pub fn background(&self) -> CosmicPanelBackground {
        self.background.clone()
    }

    pub fn spacing(&self) -> u32 {
        self.spacing
    }

    pub fn exclusive_zone(&self) -> bool {
        self.exclusive_zone
    }

    pub fn autohide(&self) -> Option<AutoHide> {
        self.autohide.clone()
    }

    /// get whether the panel should expand to cover the edges of the output
    pub fn expand_to_edges(&self) -> bool {
        self.expand_to_edges
    }

    pub fn plugins_left(&self) -> Option<Vec<String>> {
        if self.expand_to_edges {
            self.plugins_wings.as_ref().map(|w| w.0.clone())
        } else {
            None
        }
    }

    pub fn plugins_center(&self) -> Option<Vec<String>> {
        if self.expand_to_edges || self.plugins_wings.is_none() {
            self.plugins_center.clone()
        } else if let Some(plugins_center) = self.plugins_center.as_ref() {
            let (left, right) = self.plugins_wings.as_ref().unwrap();
            Some(
                left.clone()
                    .into_iter()
                    .chain(
                        plugins_center
                            .clone()
                            .into_iter()
                            .chain(right.clone().into_iter()),
                    )
                    .collect(),
            )
        } else {
            let (left, right) = self.plugins_wings.as_ref().unwrap();
            Some(
                left.clone()
                    .into_iter()
                    .chain(right.clone().into_iter())
                    .collect(),
            )
        }
    }

    pub fn plugins_right(&self) -> Option<Vec<String>> {
        if self.expand_to_edges {
            self.plugins_wings.as_ref().map(|w| w.1.clone())
        } else {
            None
        }
    }

    pub fn anchor(&self) -> PanelAnchor {
        self.anchor
    }

    pub fn padding(&self) -> u32 {
        self.padding
    }

    pub fn layer(&self) -> zwlr_layer_shell_v1::Layer {
        self.layer.into()
    }

    pub fn keyboard_interactivity(&self) -> zwlr_layer_surface_v1::KeyboardInteractivity {
        self.keyboard_interactivity.into()
    }

    pub fn is_horizontal(&self) -> bool {
        match self.anchor {
            PanelAnchor::Top | PanelAnchor::Bottom => true,
            _ => false,
        }
    }

    /// get constraints for the thickness of the panel bar
    pub fn get_dimensions(
        &self,
        output_dims: Option<(u32, u32)>,
        suggested_length: Option<u32>,
    ) -> (Option<Range<u32>>, Option<Range<u32>>) {
        let mut bar_thickness = match &self.size {
            PanelSize::XS => 8..61,
            PanelSize::S => 8..81,
            PanelSize::M => 8..101,
            PanelSize::L => 8..121,
            PanelSize::XL => 8..141,
        };
        assert!(2 * self.padding < bar_thickness.end);
        bar_thickness.end -= 2 * self.padding;
        let o_h = suggested_length.unwrap_or_else(|| output_dims.unwrap_or_default().1);
        let o_w = suggested_length.unwrap_or_else(|| output_dims.unwrap_or_default().0);

        match self.anchor {
            PanelAnchor::Left | PanelAnchor::Right => (Some(bar_thickness), Some(o_h..o_h + 1)),
            PanelAnchor::Top | PanelAnchor::Bottom => (Some(o_w..o_w + 1), Some(bar_thickness)),
        }
    }

    pub fn cosmic_config(name: &str) -> Result<Config, cosmic_config::Error> {
        let entry_name = format!("{NAME}.{}", name);
        Config::new(&entry_name, VERSION)
    }
}

#[cfg(feature = "wayland-rs")]
impl WrapperConfig for CosmicPanelConfig {
    fn outputs(&self) -> WrapperOutput {
        self.output.clone().into()
    }

    fn name(&self) -> &str {
        &self.name
    }
}
