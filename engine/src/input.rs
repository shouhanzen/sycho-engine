use std::{collections::HashSet, hash::Hash};

use serde::{Deserialize, Serialize, Serializer};
use winit::event::{ElementState, KeyboardInput, MouseScrollDelta, WindowEvent};

pub use winit::event::{MouseButton, VirtualKeyCode};

use crate::embedded::EditorEvent;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InputFrame {
    #[serde(rename = "pointer")]
    pub mouse_pos: Option<(u32, u32)>,
    #[serde(rename = "buttons_down", serialize_with = "serialize_buttons")]
    pub mouse_buttons_down: HashSet<MouseButton>,
    #[serde(rename = "buttons_pressed", serialize_with = "serialize_buttons")]
    pub mouse_buttons_pressed: HashSet<MouseButton>,
    #[serde(rename = "buttons_released", serialize_with = "serialize_buttons")]
    pub mouse_buttons_released: HashSet<MouseButton>,
    #[serde(serialize_with = "serialize_keys")]
    pub keys_down: HashSet<VirtualKeyCode>,
    #[serde(serialize_with = "serialize_keys")]
    pub keys_pressed: HashSet<VirtualKeyCode>,
    #[serde(serialize_with = "serialize_keys")]
    pub keys_released: HashSet<VirtualKeyCode>,
    pub scroll: (f32, f32),
    #[serde(rename = "focused")]
    pub window_focused: bool,
}

// Canonical order keeps unchanged input sets from producing timeline deltas.
fn serialize_keys<S: Serializer>(
    keys: &HashSet<VirtualKeyCode>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    let mut keys: Vec<_> = keys.iter().copied().collect();
    keys.sort_unstable();
    keys.serialize(serializer)
}

fn serialize_buttons<S: Serializer>(
    buttons: &HashSet<MouseButton>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    let mut buttons: Vec<_> = buttons.iter().copied().collect();
    buttons.sort_unstable_by_key(|button| match button {
        MouseButton::Left => 0,
        MouseButton::Right => 1,
        MouseButton::Middle => 2,
        MouseButton::Other(id) => usize::from(*id) + 3,
    });
    buttons.serialize(serializer)
}

impl Default for InputFrame {
    fn default() -> Self {
        Self {
            mouse_pos: None,
            mouse_buttons_down: HashSet::new(),
            mouse_buttons_pressed: HashSet::new(),
            mouse_buttons_released: HashSet::new(),
            keys_down: HashSet::new(),
            keys_pressed: HashSet::new(),
            keys_released: HashSet::new(),
            scroll: (0.0, 0.0),
            // Avoid gating controls before the OS sends an initial focus event.
            window_focused: true,
        }
    }
}

impl InputFrame {
    fn apply_key_state(&mut self, key: VirtualKeyCode, state: ElementState) {
        apply_button_transition(
            &mut self.keys_down,
            &mut self.keys_pressed,
            &mut self.keys_released,
            key,
            state,
        );
    }

    fn apply_mouse_button_state(&mut self, button: MouseButton, state: ElementState) {
        apply_button_transition(
            &mut self.mouse_buttons_down,
            &mut self.mouse_buttons_pressed,
            &mut self.mouse_buttons_released,
            button,
            state,
        );
    }

    fn apply_scroll_delta(&mut self, delta: &MouseScrollDelta, scale_factor: f64) {
        match delta {
            MouseScrollDelta::LineDelta(x, y) => {
                self.scroll.0 += *x;
                self.scroll.1 += *y;
            }
            MouseScrollDelta::PixelDelta(pos) => {
                let logical = pos.to_logical::<f32>(scale_factor);
                self.scroll.0 += logical.x / 120.0;
                self.scroll.1 += logical.y / 120.0;
            }
        }
    }

    fn set_window_focus(&mut self, focused: bool) {
        self.window_focused = focused;
        if !focused {
            self.keys_down.clear();
            self.keys_pressed.clear();
            self.keys_released.clear();
            self.mouse_buttons_down.clear();
            self.mouse_buttons_pressed.clear();
            self.mouse_buttons_released.clear();
        }
    }

    pub(crate) fn apply_editor_event(&mut self, event: EditorEvent) {
        match event {
            EditorEvent::Key(key, state) => self.apply_key_state(key, state),
            EditorEvent::Button(button, state) => self.apply_mouse_button_state(button, state),
            EditorEvent::Pointer(position) => self.mouse_pos = position,
            EditorEvent::Scroll(x, y) => {
                self.scroll.0 += x;
                self.scroll.1 += y;
            }
            EditorEvent::Focus(focused) => self.set_window_focus(focused),
            EditorEvent::ReleaseInput => {
                let focused = self.window_focused;
                self.set_window_focus(false);
                self.window_focused = focused;
                self.mouse_pos = None;
            }
            EditorEvent::Resize(_)
            | EditorEvent::Profile(_)
            | EditorEvent::MasterVolume { .. }
            | EditorEvent::Timeline(_)
            | EditorEvent::ResetTimeline
            | EditorEvent::Bookmark(_)
            | EditorEvent::ReleaseSurface(_)
            | EditorEvent::UseRgba
            | EditorEvent::ReloadTiles
            | EditorEvent::ForkPreview { .. } => {}
        }
    }

    pub(crate) fn apply_window_event(&mut self, event: &WindowEvent, scroll_scale: f64) {
        match event {
            WindowEvent::CursorMoved { position, .. } => {
                let new_x = position.x.max(0.0) as u32;
                let new_y = position.y.max(0.0) as u32;
                self.mouse_pos = Some((new_x, new_y));
            }
            WindowEvent::CursorLeft { .. } => self.mouse_pos = None,
            WindowEvent::MouseInput {
                state: mouse_state,
                button,
                ..
            } => {
                self.apply_mouse_button_state(*button, *mouse_state);
            }
            WindowEvent::KeyboardInput {
                input:
                    KeyboardInput {
                        state: key_state,
                        virtual_keycode: Some(key),
                        ..
                    },
                ..
            } => {
                self.apply_key_state(*key, *key_state);
            }
            WindowEvent::MouseWheel { delta, .. } => {
                self.apply_scroll_delta(delta, scroll_scale);
            }
            WindowEvent::Focused(focused) => {
                self.set_window_focus(*focused);
            }
            _ => {}
        }
    }

    pub(crate) fn clear_frame_transients(&mut self) {
        self.mouse_buttons_pressed.clear();
        self.mouse_buttons_released.clear();
        self.keys_pressed.clear();
        self.keys_released.clear();
        self.scroll = (0.0, 0.0);
    }
}

fn apply_button_transition<T>(
    down: &mut HashSet<T>,
    pressed: &mut HashSet<T>,
    released: &mut HashSet<T>,
    button: T,
    state: ElementState,
) where
    T: Copy + Eq + Hash,
{
    match state {
        ElementState::Pressed => {
            if down.insert(button) {
                pressed.insert(button);
            }
        }
        ElementState::Released => {
            if down.remove(&button) {
                released.insert(button);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_frame_uses_the_recorded_input_schema_with_canonical_order() {
        let recorded = serde_json::json!({
            "pointer": [12, 34],
            "buttons_down": ["Left", "Middle", {"Other": 7}],
            "buttons_pressed": ["Middle"],
            "buttons_released": ["Right"],
            "keys_down": ["A", "Z"],
            "keys_pressed": ["Z"],
            "keys_released": ["B"],
            "scroll": [0.5, -1.25],
            "focused": true,
        });
        let frame: InputFrame = serde_json::from_value(recorded.clone()).unwrap();
        assert_eq!(frame.mouse_pos, Some((12, 34)));
        assert!(frame.mouse_buttons_down.contains(&MouseButton::Other(7)));
        assert_eq!(frame.scroll, (0.5, -1.25));
        assert_eq!(serde_json::to_value(&frame).unwrap(), recorded);

        let mut reordered = recorded.clone();
        reordered["buttons_down"].as_array_mut().unwrap().reverse();
        reordered["keys_down"].as_array_mut().unwrap().reverse();
        let reordered: InputFrame = serde_json::from_value(reordered).unwrap();
        assert_eq!(reordered, frame);
        assert_eq!(serde_json::to_value(reordered).unwrap(), recorded);
    }

    #[test]
    fn key_transitions_are_frame_based() {
        let mut input = InputFrame::default();

        input.apply_key_state(VirtualKeyCode::A, ElementState::Pressed);
        assert!(input.keys_down.contains(&VirtualKeyCode::A));
        assert!(input.keys_pressed.contains(&VirtualKeyCode::A));
        assert!(!input.keys_released.contains(&VirtualKeyCode::A));

        input.clear_frame_transients();
        assert!(input.keys_down.contains(&VirtualKeyCode::A));
        assert!(!input.keys_pressed.contains(&VirtualKeyCode::A));

        input.apply_key_state(VirtualKeyCode::A, ElementState::Released);
        assert!(!input.keys_down.contains(&VirtualKeyCode::A));
        assert!(input.keys_released.contains(&VirtualKeyCode::A));
    }

    #[test]
    fn mouse_button_edges_reset_between_ticks() {
        let mut input = InputFrame::default();

        input.apply_mouse_button_state(MouseButton::Left, ElementState::Pressed);
        assert!(input.mouse_buttons_down.contains(&MouseButton::Left));
        assert!(input.mouse_buttons_pressed.contains(&MouseButton::Left));

        input.clear_frame_transients();
        assert!(input.mouse_buttons_down.contains(&MouseButton::Left));
        assert!(input.mouse_buttons_pressed.is_empty());

        input.apply_mouse_button_state(MouseButton::Left, ElementState::Released);
        assert!(!input.mouse_buttons_down.contains(&MouseButton::Left));
        assert!(input.mouse_buttons_released.contains(&MouseButton::Left));
    }

    #[test]
    fn scroll_accumulates_then_resets_between_frames() {
        let mut input = InputFrame::default();

        input.apply_scroll_delta(&MouseScrollDelta::LineDelta(1.0, -2.0), 2.0);
        input.apply_scroll_delta(
            &MouseScrollDelta::PixelDelta(winit::dpi::PhysicalPosition::new(480.0, -240.0)),
            2.0,
        );
        assert!((input.scroll.0 - 3.0).abs() < 0.0001);
        assert!((input.scroll.1 - -3.0).abs() < 0.0001);

        input.clear_frame_transients();
        assert!((input.scroll.0 - 0.0).abs() < 0.0001);
        assert!((input.scroll.1 - 0.0).abs() < 0.0001);
    }

    #[test]
    fn focus_loss_clears_held_inputs() {
        let mut input = InputFrame::default();

        input.apply_key_state(VirtualKeyCode::Space, ElementState::Pressed);
        input.apply_mouse_button_state(MouseButton::Left, ElementState::Pressed);
        input.set_window_focus(false);

        assert!(!input.window_focused);
        assert!(input.keys_down.is_empty());
        assert!(input.mouse_buttons_down.is_empty());
    }
}
