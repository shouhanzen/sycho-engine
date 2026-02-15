use std::time::{Duration, Instant};

use engine::app::InputFrame;
use winit::event::VirtualKeyCode;

use crate::playtest::InputAction;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HorizontalDir {
    Left,
    Right,
}

#[derive(Debug, Default)]
pub struct HorizontalRepeat {
    pub left_down: bool,
    pub right_down: bool,
    pub active: Option<HorizontalDir>,
    pub next_repeat_at: Option<Instant>,
}

impl HorizontalRepeat {
    // Roughly "DAS/ARR"-ish defaults, but the key property is that repeating is driven by our
    // own timer, not the OS key-repeat (so it won't get interrupted by other keypresses).
    pub const REPEAT_DELAY: Duration = Duration::from_millis(170);
    pub const REPEAT_INTERVAL: Duration = Duration::from_millis(50);

    pub fn clear(&mut self) {
        self.left_down = false;
        self.right_down = false;
        self.active = None;
        self.next_repeat_at = None;
    }

    pub fn on_press(&mut self, dir: HorizontalDir, now: Instant) -> bool {
        let was_down = match dir {
            HorizontalDir::Left => self.left_down,
            HorizontalDir::Right => self.right_down,
        };
        if was_down {
            // Ignore OS key-repeat "Pressed" events; repeating is handled by `next_repeat_action`.
            return false;
        }

        match dir {
            HorizontalDir::Left => self.left_down = true,
            HorizontalDir::Right => self.right_down = true,
        }

        self.active = Some(dir);
        self.next_repeat_at = Some(now + Self::REPEAT_DELAY);
        true
    }

    pub fn on_release(&mut self, dir: HorizontalDir, now: Instant) {
        match dir {
            HorizontalDir::Left => self.left_down = false,
            HorizontalDir::Right => self.right_down = false,
        }

        if self.active != Some(dir) {
            return;
        }

        // If the active direction was released, fall back to the other one if still held.
        let new_active = match dir {
            HorizontalDir::Left if self.right_down => Some(HorizontalDir::Right),
            HorizontalDir::Right if self.left_down => Some(HorizontalDir::Left),
            _ => None,
        };

        self.active = new_active;
        self.next_repeat_at = new_active.map(|_| now + Self::REPEAT_DELAY);
    }

    pub fn next_repeat_action(&mut self, now: Instant) -> Option<InputAction> {
        let dir = self.active?;
        let next_at = self.next_repeat_at?;
        if now < next_at {
            return None;
        }

        self.next_repeat_at = Some(now + Self::REPEAT_INTERVAL);
        Some(match dir {
            HorizontalDir::Left => InputAction::MoveLeft,
            HorizontalDir::Right => InputAction::MoveRight,
        })
    }
}

pub fn sync_horizontal_repeat_from_frame<F>(
    input: &InputFrame,
    repeat: &mut HorizontalRepeat,
    now: Instant,
    mut on_initial_action: F,
) where
    F: FnMut(InputAction),
{
    let left_down_now = input.keys_down.contains(&VirtualKeyCode::Left);
    let right_down_now = input.keys_down.contains(&VirtualKeyCode::Right)
        || input.keys_down.contains(&VirtualKeyCode::D);
    let right_released = input.keys_released.contains(&VirtualKeyCode::Right)
        || input.keys_released.contains(&VirtualKeyCode::D);

    if input.keys_released.contains(&VirtualKeyCode::Left) && !left_down_now && repeat.left_down {
        repeat.on_release(HorizontalDir::Left, now);
    }
    if right_released && !right_down_now && repeat.right_down {
        repeat.on_release(HorizontalDir::Right, now);
    }

    if left_down_now && !repeat.left_down {
        if repeat.on_press(HorizontalDir::Left, now) {
            on_initial_action(InputAction::MoveLeft);
        }
    } else if !left_down_now && repeat.left_down {
        repeat.on_release(HorizontalDir::Left, now);
    }

    if right_down_now && !repeat.right_down {
        if repeat.on_press(HorizontalDir::Right, now) {
            on_initial_action(InputAction::MoveRight);
        }
    } else if !right_down_now && repeat.right_down {
        repeat.on_release(HorizontalDir::Right, now);
    }
}

pub fn map_key_to_action(key: VirtualKeyCode) -> Option<InputAction> {
    match key {
        VirtualKeyCode::Left => Some(InputAction::MoveLeft),
        VirtualKeyCode::Right | VirtualKeyCode::D => Some(InputAction::MoveRight),
        VirtualKeyCode::Down | VirtualKeyCode::S => Some(InputAction::SoftDrop),
        VirtualKeyCode::Up | VirtualKeyCode::W => Some(InputAction::RotateCw),
        VirtualKeyCode::Z => Some(InputAction::RotateCcw),
        VirtualKeyCode::X => Some(InputAction::RotateCw),
        VirtualKeyCode::A => Some(InputAction::Rotate180),
        VirtualKeyCode::Space => Some(InputAction::HardDrop),
        VirtualKeyCode::C => Some(InputAction::Hold),
        _ => None,
    }
}

pub fn should_play_action_sfx(action: InputAction) -> bool {
    // Gameplay actions happen very frequently; only hard drop gets a click SFX.
    matches!(action, InputAction::HardDrop)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input_frame_for_keys(
        pressed: &[VirtualKeyCode],
        down: &[VirtualKeyCode],
        released: &[VirtualKeyCode],
    ) -> InputFrame {
        let mut input = InputFrame::default();
        for &key in down {
            input.keys_down.insert(key);
        }
        for &key in pressed {
            input.keys_pressed.insert(key);
        }
        for &key in released {
            input.keys_released.insert(key);
        }
        input
    }

    #[test]
    fn map_key_a_to_rotate_180() {
        assert_eq!(
            map_key_to_action(VirtualKeyCode::A),
            Some(InputAction::Rotate180)
        );
    }

    #[test]
    fn hard_drop_is_the_only_gameplay_sfx_trigger() {
        for action in [
            InputAction::Noop,
            InputAction::MoveLeft,
            InputAction::MoveRight,
            InputAction::SoftDrop,
            InputAction::RotateCw,
            InputAction::RotateCcw,
            InputAction::Rotate180,
            InputAction::Hold,
        ] {
            assert!(!should_play_action_sfx(action));
        }
        assert!(should_play_action_sfx(InputAction::HardDrop));
    }

    #[test]
    fn sync_horizontal_repeat_consumes_frame_sets() {
        let mut repeat = HorizontalRepeat::default();
        let now = Instant::now();
        let mut immediate = Vec::new();

        sync_horizontal_repeat_from_frame(
            &input_frame_for_keys(&[VirtualKeyCode::Left], &[VirtualKeyCode::Left], &[]),
            &mut repeat,
            now,
            |action| immediate.push(action),
        );
        assert_eq!(immediate, vec![InputAction::MoveLeft]);
        assert_eq!(repeat.active, Some(HorizontalDir::Left));

        sync_horizontal_repeat_from_frame(
            &input_frame_for_keys(&[], &[], &[VirtualKeyCode::Left]),
            &mut repeat,
            now + Duration::from_millis(10),
            |_| {},
        );
        assert!(!repeat.left_down);
        assert_eq!(repeat.active, None);
    }
}
