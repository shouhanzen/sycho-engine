use std::time::Duration;

use crate::skilltree::{
    SkillTreeRuntime, Vec2f, clamp_camera_min_to_bounds, skilltree_world_bounds,
};
use crate::tetris_core::Vec2i;
use crate::tetris_ui::{Rect, SkillTreeLayout};

pub const SKILLTREE_CAMERA_MIN_CELL_PX: f32 = 8.0;
pub const SKILLTREE_CAMERA_MAX_CELL_PX: f32 = 64.0;
pub const SKILLTREE_EDGE_PAN_MARGIN_PX: f32 = 28.0;
pub const SKILLTREE_EDGE_PAN_MAX_SPEED_PX_PER_S: f32 = 900.0;
pub const SKILLTREE_DRAG_THRESHOLD_PX: f32 = 4.0;
pub const SKILLTREE_CAMERA_BOUNDS_PAD_CELLS: f32 = 6.0;

#[derive(Debug, Default, Clone, Copy)]
pub struct SkillTreeCameraInput {
    pub left_down: bool,
    pub drag_started: bool,
    pub drag_started_in_view: bool,
    pub down_x: u32,
    pub down_y: u32,
    pub last_x: u32,
    pub last_y: u32,
}

pub fn skilltree_grid_viewport(layout: SkillTreeLayout) -> Option<Rect> {
    if layout.grid_cell == 0 || layout.grid_cols == 0 || layout.grid_rows == 0 {
        return None;
    }
    let w = layout.grid_cols.saturating_mul(layout.grid_cell);
    let h = layout.grid_rows.saturating_mul(layout.grid_cell);
    if w == 0 || h == 0 {
        return None;
    }
    Some(Rect::new(layout.grid_origin_x, layout.grid_origin_y, w, h))
}

pub fn skilltree_world_cell_at_screen(
    skilltree: &SkillTreeRuntime,
    layout: SkillTreeLayout,
    sx: u32,
    sy: u32,
) -> Option<Vec2i> {
    let view = skilltree_grid_viewport(layout)?;
    if !view.contains(sx, sy) {
        return None;
    }

    let cell = layout.grid_cell as f32;
    if cell <= 0.0 {
        return None;
    }

    // Camera min in world cells (float).
    let default_cam_min_x = -(layout.grid_cols as i32) / 2;
    let cam_min_x = default_cam_min_x as f32 + skilltree.camera.pan.x;
    let cam_min_y = skilltree.camera.pan.y;

    // Pixel centers (avoid boundary edge-cases).
    let sx = sx as f32 + 0.5;
    let sy = sy as f32 + 0.5;

    let col_f = (sx - layout.grid_origin_x as f32) / cell;
    let row_from_top_f = (sy - layout.grid_origin_y as f32) / cell;

    let world_x = cam_min_x + col_f;
    let world_y = cam_min_y + (layout.grid_rows as f32) - row_from_top_f;

    Some(Vec2i::new(world_x.floor() as i32, world_y.floor() as i32))
}

pub fn skilltree_node_at_world<'a>(
    skilltree: &'a SkillTreeRuntime,
    world: Vec2i,
) -> Option<&'a str> {
    for node in &skilltree.def.nodes {
        for rel in &node.shape {
            let wx = node.pos.x + rel.x;
            let wy = node.pos.y + rel.y;
            if wx == world.x && wy == world.y {
                return Some(node.id.as_str());
            }
        }
    }
    None
}

pub fn clamp_skilltree_camera_to_bounds(
    skilltree: &mut SkillTreeRuntime,
    grid_cols: u32,
    grid_rows: u32,
) {
    if grid_cols == 0 || grid_rows == 0 {
        return;
    }
    let Some(bounds) = skilltree_world_bounds(&skilltree.def) else {
        return;
    };

    let default_cam_min_x = -(grid_cols as i32) / 2;
    let default_cam_min_y = 0i32;
    let view = Vec2f::new(grid_cols as f32, grid_rows as f32);

    let cam_min_target = Vec2f::new(
        default_cam_min_x as f32 + skilltree.camera.target_pan.x,
        default_cam_min_y as f32 + skilltree.camera.target_pan.y,
    );
    let cam_min_target = clamp_camera_min_to_bounds(
        cam_min_target,
        view,
        bounds,
        SKILLTREE_CAMERA_BOUNDS_PAD_CELLS,
    );
    skilltree.camera.target_pan.x = cam_min_target.x - default_cam_min_x as f32;
    skilltree.camera.target_pan.y = cam_min_target.y - default_cam_min_y as f32;

    let cam_min = Vec2f::new(
        default_cam_min_x as f32 + skilltree.camera.pan.x,
        default_cam_min_y as f32 + skilltree.camera.pan.y,
    );
    let cam_min =
        clamp_camera_min_to_bounds(cam_min, view, bounds, SKILLTREE_CAMERA_BOUNDS_PAD_CELLS);
    skilltree.camera.pan.x = cam_min.x - default_cam_min_x as f32;
    skilltree.camera.pan.y = cam_min.y - default_cam_min_y as f32;
}

pub fn update_drag_from_frame(
    skilltree: &mut SkillTreeRuntime,
    last_skilltree: SkillTreeLayout,
    cam_input: &mut SkillTreeCameraInput,
    mouse_x: u32,
    mouse_y: u32,
    left_mouse_down: bool,
    in_skilltree_view: bool,
) {
    if !in_skilltree_view
        || !left_mouse_down
        || !cam_input.left_down
        || !cam_input.drag_started_in_view
        || last_skilltree.grid_cell == 0
    {
        return;
    }

    let new_x = mouse_x;
    let new_y = mouse_y;
    let dx = new_x as i32 - cam_input.last_x as i32;
    let dy = new_y as i32 - cam_input.last_y as i32;

    if !cam_input.drag_started {
        let total_dx = new_x as f32 - cam_input.down_x as f32;
        let total_dy = new_y as f32 - cam_input.down_y as f32;
        if total_dx * total_dx + total_dy * total_dy
            >= SKILLTREE_DRAG_THRESHOLD_PX * SKILLTREE_DRAG_THRESHOLD_PX
        {
            cam_input.drag_started = true;
        }
    }

    if cam_input.drag_started {
        let mut cam_min = Vec2f::new(
            last_skilltree.grid_cam_min_x as f32,
            last_skilltree.grid_cam_min_y as f32,
        );
        cam_min.x -= dx as f32 / last_skilltree.grid_cell as f32;
        cam_min.y += dy as f32 / last_skilltree.grid_cell as f32;

        let view_size_cells = Vec2f::new(
            last_skilltree.grid_cols as f32,
            last_skilltree.grid_rows as f32,
        );
        if let Some(bounds) = skilltree_world_bounds(&skilltree.def) {
            cam_min = clamp_camera_min_to_bounds(
                cam_min,
                view_size_cells,
                bounds,
                SKILLTREE_CAMERA_BOUNDS_PAD_CELLS,
            );
        }

        let default_cam_min_x = -(last_skilltree.grid_cols as i32) / 2;
        let default_cam_min_y = 0i32;
        skilltree.camera.pan.x = cam_min.x - default_cam_min_x as f32;
        skilltree.camera.pan.y = cam_min.y - default_cam_min_y as f32;
        skilltree.camera.target_pan = skilltree.camera.pan;
    }

    cam_input.last_x = new_x;
    cam_input.last_y = new_y;
}

pub fn apply_wheel_zoom(
    skilltree: &mut SkillTreeRuntime,
    last_skilltree: SkillTreeLayout,
    mouse_x: u32,
    mouse_y: u32,
    scroll_y: f32,
) {
    if scroll_y == 0.0 {
        return;
    }

    let zoom_factor = 1.12f32.powf(scroll_y);
    skilltree.camera.target_cell_px = (skilltree.camera.target_cell_px * zoom_factor)
        .clamp(SKILLTREE_CAMERA_MIN_CELL_PX, SKILLTREE_CAMERA_MAX_CELL_PX);

    let Some(viewport) = skilltree_grid_viewport(last_skilltree) else {
        return;
    };
    if !viewport.contains(mouse_x, mouse_y) || last_skilltree.grid_cell == 0 {
        return;
    }

    let old_cell = last_skilltree.grid_cell.max(1) as f32;
    let sx = mouse_x as f32 + 0.5;
    let sy = mouse_y as f32 + 0.5;

    let default_cam_min_x_old = -(last_skilltree.grid_cols as i32) / 2;
    let cam_min_x_old = default_cam_min_x_old as f32 + skilltree.camera.pan.x;
    let cam_min_y_old = skilltree.camera.pan.y;

    let col_f = (sx - last_skilltree.grid_origin_x as f32) / old_cell;
    let row_from_top_f = (sy - last_skilltree.grid_origin_y as f32) / old_cell;
    let world_x = cam_min_x_old + col_f;
    let world_y = cam_min_y_old + (last_skilltree.grid_rows as f32) - row_from_top_f;

    let grid_cell_new = skilltree
        .camera
        .target_cell_px
        .round()
        .clamp(SKILLTREE_CAMERA_MIN_CELL_PX, SKILLTREE_CAMERA_MAX_CELL_PX)
        as u32;
    if grid_cell_new == 0 {
        return;
    }

    let grid = last_skilltree.grid;
    let grid_cols_new = grid.w / grid_cell_new;
    let grid_rows_new = grid.h / grid_cell_new;
    if grid_cols_new == 0 || grid_rows_new == 0 {
        return;
    }

    let grid_pixel_w_new = grid_cols_new.saturating_mul(grid_cell_new);
    let grid_pixel_h_new = grid_rows_new.saturating_mul(grid_cell_new);
    let grid_origin_x_new = grid
        .x
        .saturating_add(grid.w.saturating_sub(grid_pixel_w_new) / 2);
    let grid_origin_y_new = grid
        .y
        .saturating_add(grid.h.saturating_sub(grid_pixel_h_new) / 2);

    let new_cell = grid_cell_new as f32;
    let default_cam_min_x_new = -(grid_cols_new as i32) / 2;

    let cam_min_x_new = world_x - (sx - grid_origin_x_new as f32) / new_cell;
    let cam_min_y_new =
        world_y - (grid_rows_new as f32) + (sy - grid_origin_y_new as f32) / new_cell;

    skilltree.camera.target_pan.x = cam_min_x_new - default_cam_min_x_new as f32;
    skilltree.camera.target_pan.y = cam_min_y_new;

    clamp_skilltree_camera_to_bounds(skilltree, grid_cols_new, grid_rows_new);
}

pub fn apply_edge_pan(
    skilltree: &mut SkillTreeRuntime,
    last_skilltree: SkillTreeLayout,
    mouse_x: u32,
    mouse_y: u32,
    dt: Duration,
    drag_active: bool,
) {
    if drag_active {
        return;
    }

    let Some(viewport) = skilltree_grid_viewport(last_skilltree) else {
        return;
    };
    if !viewport.contains(mouse_x, mouse_y) || last_skilltree.grid_cell == 0 {
        return;
    }

    let mx = mouse_x as f32;
    let my = mouse_y as f32;
    let x0 = viewport.x as f32;
    let y0 = viewport.y as f32;
    let x1 = (viewport.x.saturating_add(viewport.w)) as f32;
    let y1 = (viewport.y.saturating_add(viewport.h)) as f32;

    let margin = SKILLTREE_EDGE_PAN_MARGIN_PX.max(1.0);
    let left = (mx - x0).max(0.0);
    let right = (x1 - mx).max(0.0);
    let top = (my - y0).max(0.0);
    let bottom = (y1 - my).max(0.0);

    let mut vx = 0.0f32;
    let mut vy = 0.0f32;
    if left < margin {
        let t = 1.0 - left / margin;
        vx -= t * t;
    }
    if right < margin {
        let t = 1.0 - right / margin;
        vx += t * t;
    }
    if top < margin {
        let t = 1.0 - top / margin;
        vy += t * t;
    }
    if bottom < margin {
        let t = 1.0 - bottom / margin;
        vy -= t * t;
    }

    let dt_s = dt.as_secs_f32();
    if (vx == 0.0 && vy == 0.0) || dt_s <= 0.0 {
        return;
    }

    let cell_px = (last_skilltree.grid_cell as f32).max(1.0);
    let dx_cells = (vx * SKILLTREE_EDGE_PAN_MAX_SPEED_PX_PER_S * dt_s) / cell_px;
    let dy_cells = (vy * SKILLTREE_EDGE_PAN_MAX_SPEED_PX_PER_S * dt_s) / cell_px;
    skilltree.camera.target_pan.x += dx_cells;
    skilltree.camera.target_pan.y += dy_cells;
    clamp_skilltree_camera_to_bounds(
        skilltree,
        last_skilltree.grid_cols,
        last_skilltree.grid_rows,
    );
}

pub fn finalize_camera(skilltree: &mut SkillTreeRuntime, last_skilltree: SkillTreeLayout) {
    skilltree.camera.target_cell_px = skilltree
        .camera
        .target_cell_px
        .clamp(SKILLTREE_CAMERA_MIN_CELL_PX, SKILLTREE_CAMERA_MAX_CELL_PX);
    skilltree.camera.pan.x = skilltree.camera.target_pan.x;
    skilltree.camera.pan.y = skilltree.camera.target_pan.y;
    skilltree.camera.cell_px = skilltree
        .camera
        .target_cell_px
        .clamp(SKILLTREE_CAMERA_MIN_CELL_PX, SKILLTREE_CAMERA_MAX_CELL_PX);
    clamp_skilltree_camera_to_bounds(
        skilltree,
        last_skilltree.grid_cols,
        last_skilltree.grid_rows,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_viewport_requires_non_zero_grid() {
        assert!(skilltree_grid_viewport(SkillTreeLayout::default()).is_none());
        assert!(
            skilltree_grid_viewport(SkillTreeLayout {
                grid_cell: 16,
                grid_cols: 8,
                grid_rows: 6,
                ..SkillTreeLayout::default()
            })
            .is_some()
        );
    }
}
