use std::collections::HashMap;

use crate::ui::Rect;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct UiId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct UiAction(pub u32);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UiState {
    pub hovered: Option<UiId>,
    pub pressed: Option<UiId>,
}

impl UiState {
    pub fn is_hovered(&self, id: UiId) -> bool {
        self.hovered == Some(id)
    }

    pub fn is_pressed(&self, id: UiId) -> bool {
        self.pressed == Some(id)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UiInput {
    pub mouse_pos: Option<(u32, u32)>,
    pub mouse_down: bool,
    pub mouse_up: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiEvent {
    Click { id: UiId, action: Option<UiAction> },
    Hover { id: UiId, entered: bool },
}

#[derive(Debug, Clone)]
pub struct UiTree {
    nodes: HashMap<UiId, UiNode>,
    roots: Vec<UiId>,
    state: UiState,
}

#[derive(Debug, Clone)]
struct UiNode {
    id: UiId,
    kind: UiNodeKind,
    rect: Rect,
    children: Vec<UiId>,
    visible: bool,
    enabled: bool,
}

#[derive(Debug, Clone)]
enum UiNodeKind {
    Canvas,
    Container,
    Button { action: Option<UiAction> },
}

impl UiTree {
    pub fn new() -> Self {
        Self {
            nodes: HashMap::new(),
            roots: Vec::new(),
            state: UiState::default(),
        }
    }

    pub fn begin_frame(&mut self) {
        self.roots.clear();
        for node in self.nodes.values_mut() {
            node.children.clear();
        }
    }

    pub fn state(&self) -> UiState {
        self.state
    }

    pub fn is_hovered(&self, id: UiId) -> bool {
        self.state.is_hovered(id)
    }

    pub fn is_pressed(&self, id: UiId) -> bool {
        self.state.is_pressed(id)
    }

    pub fn ensure_canvas(&mut self, id: UiId, rect: Rect) {
        self.ensure_node(id, UiNodeKind::Canvas, rect);
    }

    pub fn ensure_container(&mut self, id: UiId, rect: Rect) {
        self.ensure_node(id, UiNodeKind::Container, rect);
    }

    pub fn ensure_button(&mut self, id: UiId, rect: Rect, action: Option<UiAction>) {
        self.ensure_node(id, UiNodeKind::Button { action }, rect);
    }

    pub fn add_root(&mut self, id: UiId) {
        self.roots.push(id);
    }

    pub fn add_child(&mut self, parent: UiId, child: UiId) {
        if let Some(node) = self.nodes.get_mut(&parent) {
            node.children.push(child);
        }
    }

    pub fn set_visible(&mut self, id: UiId, visible: bool) {
        if let Some(node) = self.nodes.get_mut(&id) {
            node.visible = visible;
        }
    }

    pub fn set_enabled(&mut self, id: UiId, enabled: bool) {
        if let Some(node) = self.nodes.get_mut(&id) {
            node.enabled = enabled;
        }
    }

    pub fn process_input(&mut self, input: UiInput) -> Vec<UiEvent> {
        let mut events = Vec::new();
        if let Some(pos) = input.mouse_pos {
            let hovered = self.hit_test(pos);
            if hovered != self.state.hovered {
                if let Some(prev) = self.state.hovered {
                    events.push(UiEvent::Hover {
                        id: prev,
                        entered: false,
                    });
                }
                if let Some(next) = hovered {
                    events.push(UiEvent::Hover {
                        id: next,
                        entered: true,
                    });
                }
                self.state.hovered = hovered;
            }
        }

        if input.mouse_down {
            self.state.pressed = self.state.hovered;
        }

        if input.mouse_up {
            let pressed = self.state.pressed;
            if let (Some(pressed_id), Some(hovered_id)) = (pressed, self.state.hovered) {
                if pressed_id == hovered_id {
                    if let Some(node) = self.nodes.get(&pressed_id) {
                        if let UiNodeKind::Button { action } = node.kind {
                            if node.enabled {
                                events.push(UiEvent::Click {
                                    id: pressed_id,
                                    action,
                                });
                            }
                        }
                    }
                }
            }
            self.state.pressed = None;
        }

        events
    }

    fn ensure_node(&mut self, id: UiId, kind: UiNodeKind, rect: Rect) {
        let node = self.nodes.entry(id).or_insert_with(|| UiNode {
            id,
            kind: kind.clone(),
            rect,
            children: Vec::new(),
            visible: true,
            enabled: true,
        });
        node.kind = kind;
        node.rect = rect;
        node.visible = true;
    }

    fn hit_test(&self, pos: (u32, u32)) -> Option<UiId> {
        for root in self.roots.iter().rev() {
            if let Some(hit) = self.hit_test_node(*root, pos) {
                return Some(hit);
            }
        }
        None
    }

    fn hit_test_node(&self, id: UiId, pos: (u32, u32)) -> Option<UiId> {
        let node = self.nodes.get(&id)?;
        if !node.visible {
            return None;
        }
        if !node.rect.contains(pos.0, pos.1) {
            return None;
        }
        match node.kind {
            UiNodeKind::Button { .. } => {
                if node.enabled {
                    Some(id)
                } else {
                    None
                }
            }
            UiNodeKind::Canvas | UiNodeKind::Container => {
                for child in node.children.iter().rev() {
                    if let Some(hit) = self.hit_test_node(*child, pos) {
                        return Some(hit);
                    }
                }
                None
            }
        }
    }
}
