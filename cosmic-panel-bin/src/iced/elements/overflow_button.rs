use std::{
    borrow::Cow,
    hash::Hash,
    sync::{
        atomic::{self, AtomicBool},
        Arc,
    },
};

// element for rendering a button that toggles the overflow popup when clicked
use cosmic::{iced::Padding, iced_core::id, theme, Element};
use smithay::{
    desktop::space::SpaceElement,
    utils::{IsAlive, Logical, Point, Rectangle, Size},
};

use crate::iced::Program;

#[derive(Debug, Clone, Copy)]
pub enum Message {
    TogglePopup,
}

#[derive(Debug, Clone)]
pub struct OverflowButton {
    id: id::Id,
    pos: Point<i32, Logical>,
    icon_size: u16,
    button_padding: Padding,
    /// Selected if the popup is open
    selected: Arc<AtomicBool>,
    icon: Cow<'static, str>,
}

impl PartialEq for OverflowButton {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for OverflowButton {}

impl Hash for OverflowButton {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

impl Program for OverflowButton {
    type Message = Message;

    fn update(
        &mut self,
        message: Self::Message,
        loop_handle: &calloop::LoopHandle<
            'static,
            xdg_shell_wrapper::shared_state::GlobalState<crate::space_container::SpaceContainer>,
        >,
    ) -> cosmic::Command<Self::Message> {
        match message {
            Message::TogglePopup => {
                let id = self.id.clone();
                loop_handle.insert_idle(move |state| {
                    state.space.toggle_overflow_popup(id);
                });
            }
        }
        cosmic::Command::none()
    }

    fn view(&self) -> crate::iced::Element<'_, Self::Message> {
        Element::from(
            cosmic::widget::button::icon(
                cosmic::widget::icon::from_name(self.icon.clone())
                    .symbolic(true)
                    .size(self.icon_size),
            )
            .style(theme::Button::AppletIcon)
            .padding(self.button_padding)
            .on_press(Message::TogglePopup)
            .selected(self.selected.load(atomic::Ordering::SeqCst)),
        )
    }
}

impl IsAlive for OverflowButton {
    fn alive(&self) -> bool {
        true
    }
}

impl SpaceElement for OverflowButton {
    fn bbox(&self) -> smithay::utils::Rectangle<i32, smithay::utils::Logical> {
        Rectangle {
            loc: self.pos,
            size: Size::from((self.icon_size as i32, self.icon_size as i32)),
        }
    }

    fn is_in_input_region(
        &self,
        point: &smithay::utils::Point<f64, smithay::utils::Logical>,
    ) -> bool {
        self.bbox().to_f64().contains(*point)
    }

    fn set_activate(&self, _activated: bool) {}

    fn output_enter(
        &self,
        _output: &smithay::output::Output,
        _overlap: smithay::utils::Rectangle<i32, smithay::utils::Logical>,
    ) {
    }

    fn output_leave(&self, _output: &smithay::output::Output) {}
}
