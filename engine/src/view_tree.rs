use serde::{Deserialize, Serialize};

use crate::ui::Rect;
use crate::ui_tree::UiInput;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewTree<A> {
    pub nodes: Vec<ViewNode<A>>,
}

impl<A> ViewTree<A> {
    pub fn new() -> Self {
        Self { nodes: Vec::new() }
    }

    pub fn push(&mut self, node: ViewNode<A>) {
        self.nodes.push(node);
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ViewNode<A> {
    Button(ButtonNode<A>),
    Text(TextNode),
    Rect(RectNode),
    Line(LineNode),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ButtonNode<A> {
    pub id: u32,
    pub rect: Rect,
    pub label: String,
    pub action: A,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextNode {
    pub pos: (u32, u32),
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RectNode {
    pub rect: Rect,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LineNode {
    pub start: (i32, i32),
    pub end: (i32, i32),
    pub thickness: u32,
}

pub fn hit_test_actions<A: Clone>(view: &ViewTree<A>, input: UiInput) -> Vec<A> {
    if !input.mouse_up {
        return Vec::new();
    }
    let Some((mx, my)) = input.mouse_pos else {
        return Vec::new();
    };
    let mut actions = Vec::new();
    for node in view.nodes.iter().rev() {
        if let ViewNode::Button(button) = node {
            if button.enabled && button.rect.contains(mx, my) {
                actions.push(button.action.clone());
            }
        }
    }
    actions
}
