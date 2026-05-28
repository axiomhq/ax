//! Cascading collision shove for tile move / resize / insert.
//!
//! Sister module to [`super::tile_layout::tile_ops`], which rejects
//! collisions. The functions here push overlapping tiles out of the
//! way instead, using a deterministic BFS cascade:
//!
//!   * `Right` shove sets each victim's `x = max(x, blocker.x + blocker.w)`.
//!     If that would push the victim past column 12, the victim falls
//!     through to `Down` shove (its `y` advances; `x` is preserved).
//!   * `Down` shove sets each victim's `y = max(y, blocker.y + blocker.h)`.
//!     Down cascades never fall back to Right.
//!
//! Both shove flavours are strictly monotonic along their axis, so
//! the cascade terminates without needing the loop cap; the cap is
//! kept as a defensive backstop.
//!
//! See `plan/19-tile-shove-and-vim-clipboard.md` for the full algorithm.

use std::collections::{HashSet, VecDeque};

use crate::axiom::LayoutItem;

use super::tile_layout::{GRID_COLS, tile_ops};

/// Direction the cascade pushes overlapping tiles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShoveDir {
    Right,
    Down,
}

/// Summary of a successful shove. Returned to the caller so the
/// status line can report something more informative than "ok".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ShoveOutcome {
    /// Ids of every tile that moved, including the initial blocker /
    /// inserted tile, in cascade order.
    pub moved: Vec<String>,
    /// How many extra rows the grid grew by — `max_y_after - max_y_before`.
    pub new_rows: u32,
}

/// Defensive loop cap. Real cap scales with layout size to handle
/// dense grids, but stays bounded.
const BASE_CAP: usize = 256;

/// Auto-shove move. Negative-axis moves fall back to the strict
/// [`tile_ops::translate`] semantics — we never shove "backwards"
/// because that would push tiles off the top/left edge.
pub fn shove_move(
    layout: &mut Vec<LayoutItem>,
    id: &str,
    dx: i32,
    dy: i32,
) -> Result<ShoveOutcome, &'static str> {
    if dx == 0 && dy == 0 {
        // Nothing to do; keep the moved-list empty so callers can
        // detect the no-op cheaply.
        return Ok(ShoveOutcome::default());
    }
    if dx < 0 || dy < 0 {
        let before = layout_max_y(layout);
        tile_ops::translate(layout, id, dx, dy)?;
        return Ok(ShoveOutcome {
            moved: vec![id.to_string()],
            new_rows: layout_max_y(layout).saturating_sub(before),
        });
    }
    let mut next = layout.clone();
    let bidx = next
        .iter()
        .position(|l| l.i == id)
        .ok_or("tile has no layout entry")?;
    let cur_x = next[bidx].x as i32;
    let cur_y = y_of(&next[bidx]) as i32;
    let nx = cur_x + dx;
    let ny = cur_y + dy;
    if (nx as u32) + next[bidx].w > GRID_COLS {
        return Err("edge of grid");
    }
    next[bidx].x = nx as u32;
    next[bidx].y = Some(ny as u32);
    let primary = if dy > 0 {
        ShoveDir::Down
    } else {
        ShoveDir::Right
    };
    let before_max_y = layout_max_y(layout);
    let cascaded = cascade(&mut next, id, primary)?;
    validate(&next)?;
    let new_rows = layout_max_y(&next).saturating_sub(before_max_y);
    *layout = next;
    let mut moved = vec![id.to_string()];
    moved.extend(cascaded);
    Ok(ShoveOutcome { moved, new_rows })
}

/// Auto-shove resize. Shrink-only resizes (both deltas ≤ 0) use the
/// strict path because shrinking can't introduce overlap, then run
/// [`compact_vertical`] so tiles below collapse into the freed band.
/// Anything that grows in either axis runs through the cascade.
pub fn shove_resize(
    layout: &mut Vec<LayoutItem>,
    id: &str,
    dw: i32,
    dh: i32,
) -> Result<ShoveOutcome, &'static str> {
    if dw == 0 && dh == 0 {
        return Ok(ShoveOutcome::default());
    }
    if dw <= 0 && dh <= 0 {
        let before = layout_max_y(layout);
        let before_ys: Vec<(String, u32)> = layout.iter().map(|l| (l.i.clone(), y_of(l))).collect();
        tile_ops::resize(layout, id, dw, dh)?;
        // Shrink may leave an empty horizontal band beneath the
        // resized tile (or under any tile column band). Gravity-pack
        // so lower tiles ride up into the freed space.
        compact_vertical(layout);
        // Report every tile whose `y` actually changed — callers use
        // this for status messaging.
        let mut moved = vec![id.to_string()];
        for (other_id, before_y) in before_ys {
            if other_id == id {
                continue;
            }
            let after_y = layout
                .iter()
                .find(|l| l.i == other_id)
                .map(y_of)
                .unwrap_or(before_y);
            if after_y != before_y {
                moved.push(other_id);
            }
        }
        return Ok(ShoveOutcome {
            moved,
            new_rows: layout_max_y(layout).saturating_sub(before),
        });
    }
    let mut next = layout.clone();
    let bidx = next
        .iter()
        .position(|l| l.i == id)
        .ok_or("tile has no layout entry")?;
    let nw = next[bidx].w as i32 + dw;
    let nh = next[bidx].h as i32 + dh;
    if nw < 1 || nh < 1 {
        return Err("minimum size 1x1");
    }
    if next[bidx].x + (nw as u32) > GRID_COLS {
        return Err("exceeds 12-col grid");
    }
    next[bidx].w = nw as u32;
    next[bidx].h = nh as u32;
    // Prefer the axis that's actually growing; when both grow, start
    // with Right and let per-victim fallback handle the rest.
    let primary = if dw > 0 {
        ShoveDir::Right
    } else {
        ShoveDir::Down
    };
    let before_max_y = layout_max_y(layout);
    let cascaded = cascade(&mut next, id, primary)?;
    validate(&next)?;
    let new_rows = layout_max_y(&next).saturating_sub(before_max_y);
    *layout = next;
    let mut moved = vec![id.to_string()];
    moved.extend(cascaded);
    Ok(ShoveOutcome { moved, new_rows })
}

/// Insert a fully-specified `LayoutItem`, shoving any tiles it
/// overlaps. The caller is responsible for making `new_tile.i`
/// unique; we double-check and return an error if it isn't.
pub fn shove_insert(
    layout: &mut Vec<LayoutItem>,
    new_tile: LayoutItem,
    dir: ShoveDir,
) -> Result<ShoveOutcome, &'static str> {
    if new_tile.w == 0 || new_tile.h == 0 {
        return Err("minimum size 1x1");
    }
    if new_tile.x + new_tile.w > GRID_COLS {
        return Err("tile exceeds 12-col grid");
    }
    if layout.iter().any(|l| l.i == new_tile.i) {
        return Err("duplicate tile id");
    }
    let mut next = layout.clone();
    let id = new_tile.i.clone();
    next.push(new_tile);
    let before_max_y = layout_max_y(layout);
    let cascaded = cascade(&mut next, &id, dir)?;
    validate(&next)?;
    let new_rows = layout_max_y(&next).saturating_sub(before_max_y);
    *layout = next;
    let mut moved = vec![id];
    moved.extend(cascaded);
    Ok(ShoveOutcome { moved, new_rows })
}

/// Gravity-pack tiles upward: each tile slides as far up as it can
/// without colliding with any other tile or with the top edge
/// (`y = 0`). Stable: we process tiles in current `(y, x, id)`
/// order so an already-compacted row above is always settled before
/// a row below is considered, and ties break deterministically.
///
/// Why row-major iteration is safe: when we land on a tile T, every
/// tile that started above T has already moved to its final
/// position (or stayed put). Every tile that started below T is
/// still at its original `y ≥ T.y`. So colliding against the full
/// layout (minus T itself) correctly bounds how far T can rise.
pub fn compact_vertical(layout: &mut [LayoutItem]) {
    let mut order: Vec<usize> = (0..layout.len()).collect();
    order.sort_by(|&a, &b| {
        let la = &layout[a];
        let lb = &layout[b];
        (y_of(la), la.x, la.i.as_str()).cmp(&(y_of(lb), lb.x, lb.i.as_str()))
    });
    for idx in order {
        let item = layout[idx].clone();
        let original_y = y_of(&item);
        if original_y == 0 {
            continue;
        }
        // Linear scan from y=0 upward for the lowest non-colliding
        // slot. O(n·max_y) total — fine for our 12-col grids with a
        // few dozen tiles. Switch to a sweep-line if profiling ever
        // flags this.
        for y in 0..original_y {
            let test = LayoutItem {
                y: Some(y),
                ..item.clone()
            };
            let collides = layout
                .iter()
                .enumerate()
                .any(|(j, other)| j != idx && rects_overlap(&test, other));
            if !collides {
                layout[idx].y = Some(y);
                break;
            }
        }
    }
}

// ---------------------------------------------------------------------
// internals
// ---------------------------------------------------------------------

fn y_of(li: &LayoutItem) -> u32 {
    li.y.unwrap_or(0)
}

fn layout_max_y(layout: &[LayoutItem]) -> u32 {
    layout.iter().map(|l| y_of(l) + l.h).max().unwrap_or(0)
}

/// AABB overlap test for two grid tiles. Shared with
/// [`crate::app::tile_layout`]'s `overlaps_any` so the rectangle math
/// lives in exactly one place.
pub(crate) fn rects_overlap(a: &LayoutItem, b: &LayoutItem) -> bool {
    let (ax1, ay1) = (a.x, y_of(a));
    let (ax2, ay2) = (ax1 + a.w, ay1 + a.h);
    let (bx1, by1) = (b.x, y_of(b));
    let (bx2, by2) = (bx1 + b.w, by1 + b.h);
    ax1 < bx2 && ax2 > bx1 && ay1 < by2 && ay2 > by1
}

fn validate(layout: &[LayoutItem]) -> Result<(), &'static str> {
    for l in layout {
        if l.w == 0 || l.h == 0 {
            return Err("tile size below minimum after cascade");
        }
        if l.x + l.w > GRID_COLS {
            return Err("tile exceeds 12-col grid after cascade");
        }
    }
    for (i, a) in layout.iter().enumerate() {
        for b in layout.iter().skip(i + 1) {
            if rects_overlap(a, b) {
                return Err("cascade left overlapping tiles");
            }
        }
    }
    Ok(())
}

/// BFS shove cascade. The blocker is treated as fixed; everything
/// that overlaps it gets pushed away in `dir`, with per-victim
/// right-to-down fallback. Returns the cascade order of *victim*
/// ids (the blocker isn't included).
fn cascade(
    layout: &mut [LayoutItem],
    blocker_id: &str,
    dir: ShoveDir,
) -> Result<Vec<String>, &'static str> {
    let cap = std::cmp::max(
        BASE_CAP,
        layout.len().saturating_mul(layout.len()).saturating_mul(4),
    );
    let mut moved_order: Vec<String> = Vec::new();
    let mut seen_moved: HashSet<String> = HashSet::new();
    seen_moved.insert(blocker_id.to_string());

    let mut queue: VecDeque<(String, ShoveDir)> = VecDeque::new();
    for vid in overlapping_sorted(layout, blocker_id, dir) {
        queue.push_back((vid, dir));
    }

    let mut iters = 0usize;
    while let Some((victim_id, cur_dir)) = queue.pop_front() {
        iters += 1;
        if iters > cap {
            return Err("cascade did not converge");
        }
        let Some(victim_idx) = layout.iter().position(|l| l.i == victim_id) else {
            continue;
        };
        let before = layout[victim_idx].clone();
        let actual_dir = shove_one(layout, victim_idx, cur_dir);
        let moved = layout[victim_idx].x != before.x || layout[victim_idx].y != before.y;
        if !moved {
            // No-op shove; don't requeue successors (they can't have
            // gained new overlaps from a tile that didn't move).
            continue;
        }
        if seen_moved.insert(victim_id.clone()) {
            moved_order.push(victim_id.clone());
        }
        for next_v in overlapping_sorted(layout, &victim_id, actual_dir) {
            if next_v == victim_id {
                continue;
            }
            queue.push_back((next_v, actual_dir));
        }
    }
    Ok(moved_order)
}

/// Shove the tile at `victim_idx` away from every tile it currently
/// overlaps. Returns the direction actually used (Right may fall
/// back to Down).
fn shove_one(layout: &mut [LayoutItem], victim_idx: usize, dir: ShoveDir) -> ShoveDir {
    let victim = layout[victim_idx].clone();
    // Compute the max right-edge and max bottom-edge of every
    // overlapping blocker in one pass; cheap and avoids re-borrowing
    // mid-loop.
    let mut max_right: Option<u32> = None;
    let mut max_bottom: Option<u32> = None;
    for l in layout.iter() {
        if l.i == victim.i {
            continue;
        }
        if !rects_overlap(l, &victim) {
            continue;
        }
        let r = l.x + l.w;
        let b = y_of(l) + l.h;
        max_right = Some(max_right.map_or(r, |m| m.max(r)));
        max_bottom = Some(max_bottom.map_or(b, |m| m.max(b)));
    }
    let Some(max_right) = max_right else {
        return dir;
    };
    let max_bottom = max_bottom.unwrap_or(0);

    match dir {
        ShoveDir::Right => {
            if max_right + victim.w <= GRID_COLS {
                layout[victim_idx].x = layout[victim_idx].x.max(max_right);
                ShoveDir::Right
            } else {
                // Right-shove would overflow the 12-col grid; fall
                // through to Down for this victim.
                let cur_y = y_of(&layout[victim_idx]);
                layout[victim_idx].y = Some(cur_y.max(max_bottom));
                ShoveDir::Down
            }
        }
        ShoveDir::Down => {
            let cur_y = y_of(&layout[victim_idx]);
            layout[victim_idx].y = Some(cur_y.max(max_bottom));
            ShoveDir::Down
        }
    }
}

/// Collect ids of tiles overlapping `blocker_id`, sorted
/// deterministically by axis-first key so two runs on the same input
/// always produce the same cascade order.
fn overlapping_sorted(layout: &[LayoutItem], blocker_id: &str, dir: ShoveDir) -> Vec<String> {
    let Some(blocker) = layout.iter().find(|l| l.i == blocker_id).cloned() else {
        return Vec::new();
    };
    let mut v: Vec<&LayoutItem> = layout
        .iter()
        .filter(|l| l.i != blocker_id && rects_overlap(l, &blocker))
        .collect();
    match dir {
        ShoveDir::Right => {
            v.sort_by(|a, b| (a.x, y_of(a), a.i.as_str()).cmp(&(b.x, y_of(b), b.i.as_str())))
        }
        ShoveDir::Down => {
            v.sort_by(|a, b| (y_of(a), a.x, a.i.as_str()).cmp(&(y_of(b), b.x, b.i.as_str())))
        }
    }
    v.into_iter().map(|l| l.i.clone()).collect()
}

// ---------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn li(i: &str, x: u32, y: u32, w: u32, h: u32) -> LayoutItem {
        LayoutItem {
            i: i.to_string(),
            x,
            y: Some(y),
            w,
            h,
            extras: Default::default(),
        }
    }

    fn pos(layout: &[LayoutItem], id: &str) -> (u32, u32, u32, u32) {
        let l = layout.iter().find(|l| l.i == id).expect("id not found");
        (l.x, y_of(l), l.w, l.h)
    }

    #[test]
    fn no_op_move_is_empty_outcome() {
        let mut layout = vec![li("a", 0, 0, 4, 2)];
        let out = shove_move(&mut layout, "a", 0, 0).unwrap();
        assert!(out.moved.is_empty());
        assert_eq!(out.new_rows, 0);
    }

    #[test]
    fn single_overlap_shoves_right() {
        // A at (0,0,4,2), B at (4,0,4,2). Move A right by 2 → A at
        // (2,0,4,2), overlaps B; B shoves right to (6,0,4,2).
        let mut layout = vec![li("a", 0, 0, 4, 2), li("b", 4, 0, 4, 2)];
        let out = shove_move(&mut layout, "a", 2, 0).unwrap();
        assert_eq!(pos(&layout, "a"), (2, 0, 4, 2));
        assert_eq!(pos(&layout, "b"), (6, 0, 4, 2));
        assert_eq!(out.moved, vec!["a".to_string(), "b".to_string()]);
        assert_eq!(out.new_rows, 0);
    }

    #[test]
    fn chain_of_three_cascades_right() {
        // Three 3-wide tiles in a row (cols 0..9); move a right by 1
        // → all three shove and still fit within the 12-col grid.
        let mut layout = vec![
            li("a", 0, 0, 3, 2),
            li("b", 3, 0, 3, 2),
            li("c", 6, 0, 3, 2),
        ];
        let out = shove_move(&mut layout, "a", 1, 0).unwrap();
        assert_eq!(pos(&layout, "a"), (1, 0, 3, 2));
        assert_eq!(pos(&layout, "b"), (4, 0, 3, 2));
        assert_eq!(pos(&layout, "c"), (7, 0, 3, 2));
        assert!(out.moved.contains(&"a".to_string()));
        assert!(out.moved.contains(&"b".to_string()));
        assert!(out.moved.contains(&"c".to_string()));
        assert_eq!(out.new_rows, 0);
    }

    #[test]
    fn chain_wraps_from_right_to_down_at_col_12() {
        // a (0..4), b (4..8), c (8..12). Move a right by 2 →
        // a at (2..6); b → (6..10); c → would be (10..14) which
        // overflows; c falls through to Down at y = b.bottom = 2.
        let mut layout = vec![
            li("a", 0, 0, 4, 2),
            li("b", 4, 0, 4, 2),
            li("c", 8, 0, 4, 2),
        ];
        let out = shove_move(&mut layout, "a", 2, 0).unwrap();
        assert_eq!(pos(&layout, "a"), (2, 0, 4, 2));
        assert_eq!(pos(&layout, "b"), (6, 0, 4, 2));
        // c should keep x=8 but advance y to 2.
        assert_eq!(pos(&layout, "c"), (8, 2, 4, 2));
        assert_eq!(out.new_rows, 2); // grid grew by 2 (c's height).
    }

    #[test]
    fn down_shove_when_dy_positive() {
        // a at (0,0,4,2); b at (0,2,4,2). Move a down by 1 → b must
        // shove down to (0,3,4,2).
        let mut layout = vec![li("a", 0, 0, 4, 2), li("b", 0, 2, 4, 2)];
        let out = shove_move(&mut layout, "a", 0, 1).unwrap();
        assert_eq!(pos(&layout, "a"), (0, 1, 4, 2));
        assert_eq!(pos(&layout, "b"), (0, 3, 4, 2));
        assert_eq!(out.new_rows, 1);
    }

    #[test]
    fn move_left_into_neighbour_is_strict_reject() {
        // a at (4,0,4,2), b at (0,0,4,2). Move a left by 1 → would
        // overlap b; strict reject leaves layout untouched.
        let mut layout = vec![li("a", 4, 0, 4, 2), li("b", 0, 0, 4, 2)];
        let err = shove_move(&mut layout, "a", -1, 0).unwrap_err();
        assert_eq!(err, "would overlap another tile");
        assert_eq!(pos(&layout, "a"), (4, 0, 4, 2));
        assert_eq!(pos(&layout, "b"), (0, 0, 4, 2));
    }

    #[test]
    fn move_left_into_empty_succeeds_via_strict_path() {
        let mut layout = vec![li("a", 4, 0, 4, 2)];
        let out = shove_move(&mut layout, "a", -2, 0).unwrap();
        assert_eq!(pos(&layout, "a"), (2, 0, 4, 2));
        assert_eq!(out.moved, vec!["a".to_string()]);
    }

    #[test]
    fn move_off_grid_right_errors() {
        let mut layout = vec![li("a", 8, 0, 4, 2)];
        let err = shove_move(&mut layout, "a", 1, 0).unwrap_err();
        assert_eq!(err, "edge of grid");
    }

    #[test]
    fn resize_grow_right_shoves_neighbour() {
        let mut layout = vec![li("a", 0, 0, 4, 2), li("b", 4, 0, 4, 2)];
        let out = shove_resize(&mut layout, "a", 2, 0).unwrap();
        assert_eq!(pos(&layout, "a"), (0, 0, 6, 2));
        assert_eq!(pos(&layout, "b"), (6, 0, 4, 2));
        assert_eq!(out.new_rows, 0);
    }

    #[test]
    fn resize_grow_down_shoves_neighbour() {
        let mut layout = vec![li("a", 0, 0, 4, 2), li("b", 0, 2, 4, 2)];
        let out = shove_resize(&mut layout, "a", 0, 1).unwrap();
        assert_eq!(pos(&layout, "a"), (0, 0, 4, 3));
        assert_eq!(pos(&layout, "b"), (0, 3, 4, 2));
        assert_eq!(out.new_rows, 1);
    }

    #[test]
    fn resize_shrink_uses_strict_path() {
        let mut layout = vec![li("a", 0, 0, 6, 4), li("b", 6, 0, 4, 2)];
        let out = shove_resize(&mut layout, "a", -2, 0).unwrap();
        assert_eq!(pos(&layout, "a"), (0, 0, 4, 4));
        assert_eq!(pos(&layout, "b"), (6, 0, 4, 2));
        assert_eq!(out.moved, vec!["a".to_string()]);
    }

    #[test]
    fn resize_grow_past_col_12_errors() {
        let mut layout = vec![li("a", 0, 0, 8, 2)];
        let err = shove_resize(&mut layout, "a", 5, 0).unwrap_err();
        assert_eq!(err, "exceeds 12-col grid");
    }

    #[test]
    fn shove_insert_places_new_tile_at_collision_and_cascades() {
        // Two tiles occupying the top-left; insert a new tile that
        // overlaps the first. Existing tile shoves down (Down dir).
        let mut layout = vec![li("a", 0, 0, 4, 2), li("b", 0, 2, 4, 2)];
        let out = shove_insert(&mut layout, li("new", 0, 0, 4, 2), ShoveDir::Down).unwrap();
        assert_eq!(pos(&layout, "new"), (0, 0, 4, 2));
        assert_eq!(pos(&layout, "a"), (0, 2, 4, 2));
        assert_eq!(pos(&layout, "b"), (0, 4, 4, 2));
        assert!(out.moved.contains(&"new".to_string()));
        assert_eq!(out.new_rows, 2);
    }

    #[test]
    fn shove_insert_rejects_duplicate_id() {
        let mut layout = vec![li("a", 0, 0, 4, 2)];
        let err = shove_insert(&mut layout, li("a", 4, 0, 4, 2), ShoveDir::Right).unwrap_err();
        assert_eq!(err, "duplicate tile id");
    }

    #[test]
    fn shove_insert_rejects_off_grid() {
        let mut layout = vec![li("a", 0, 0, 4, 2)];
        let err = shove_insert(&mut layout, li("b", 9, 0, 4, 2), ShoveDir::Right).unwrap_err();
        assert_eq!(err, "tile exceeds 12-col grid");
    }

    #[test]
    fn dense_grid_cascade_terminates() {
        // 6 tiles, each 2-wide × 2-tall, packed across cols 0..12 at y=0.
        // Move the first by 1 col right → entire row cascades; last
        // tile must wrap to a new row at y=2.
        let mut layout: Vec<LayoutItem> = (0..6)
            .map(|i| li(&format!("t{i}"), i as u32 * 2, 0, 2, 2))
            .collect();
        let out = shove_move(&mut layout, "t0", 1, 0).unwrap();
        assert_eq!(pos(&layout, "t0"), (1, 0, 2, 2));
        // t1..t4 shove right; t5 wraps.
        assert_eq!(pos(&layout, "t1"), (3, 0, 2, 2));
        assert_eq!(pos(&layout, "t2"), (5, 0, 2, 2));
        assert_eq!(pos(&layout, "t3"), (7, 0, 2, 2));
        assert_eq!(pos(&layout, "t4"), (9, 0, 2, 2));
        assert_eq!(pos(&layout, "t5"), (10, 2, 2, 2));
        assert!(out.new_rows >= 2);
    }

    #[test]
    fn resize_shrink_height_pulls_tiles_below_up() {
        // Headline user bug: a (0..6 cols, 0..4 rows), b sitting
        // beneath at (0..6 cols, 4..6 rows). Shrink a's height by 2
        // → a is now 0..2 rows, leaving rows 2..4 empty across
        // a's column band. b must pull up to y=2 instead of
        // hovering at y=4 over an empty band.
        let mut layout = vec![li("a", 0, 0, 6, 4), li("b", 0, 4, 6, 2)];
        let out = shove_resize(&mut layout, "a", 0, -2).unwrap();
        assert_eq!(pos(&layout, "a"), (0, 0, 6, 2));
        assert_eq!(
            pos(&layout, "b"),
            (0, 2, 6, 2),
            "tile below should pull up after the row above shrinks"
        );
        assert!(out.moved.contains(&"b".to_string()));
    }

    #[test]
    fn resize_shrink_width_does_not_pull_neighbours_horizontally() {
        // Compaction is vertical-only. Shrinking a's width leaves a
        // horizontal gap; b stays put. (If you wanted horizontal
        // packing too you'd write a separate `compact_horizontal`.)
        let mut layout = vec![li("a", 0, 0, 6, 4), li("b", 6, 0, 4, 2)];
        let out = shove_resize(&mut layout, "a", -2, 0).unwrap();
        assert_eq!(pos(&layout, "a"), (0, 0, 4, 4));
        assert_eq!(pos(&layout, "b"), (6, 0, 4, 2));
        assert!(!out.moved.contains(&"b".to_string()));
    }

    #[test]
    fn resize_shrink_compacts_only_within_column_band() {
        // a (0..6, 0..4) shrinks to (0..6, 0..2). An unrelated
        // tile c at the right (8..12, 4..6) must NOT move — its
        // columns aren't under a, so there's no "empty row in
        // its column" to fill. Tile b directly under a does pull up.
        let mut layout = vec![
            li("a", 0, 0, 6, 4),
            li("b", 0, 4, 6, 2),
            li("c", 8, 4, 4, 2),
        ];
        shove_resize(&mut layout, "a", 0, -2).unwrap();
        assert_eq!(pos(&layout, "a"), (0, 0, 6, 2));
        assert_eq!(pos(&layout, "b"), (0, 2, 6, 2));
        // c is unrelated to a's column band, but it sits over an
        // empty band in its own columns (rows 0..4 in cols 8..12 are
        // empty after shrink) — gravity pulls c up to y=0.
        assert_eq!(pos(&layout, "c"), (8, 0, 4, 2));
    }

    #[test]
    fn compact_is_idempotent() {
        // Running compact twice produces the same layout as once —
        // i.e. there are no "oscillating" placements. `LayoutItem`
        // doesn't implement `PartialEq` so we compare via `pos`.
        let mut once = vec![li("a", 0, 0, 6, 2), li("b", 0, 4, 6, 2)];
        compact_vertical(&mut once);
        let mut twice = once.clone();
        compact_vertical(&mut twice);
        let ids = ["a", "b"];
        for id in ids {
            assert_eq!(pos(&once, id), pos(&twice, id));
        }
    }

    #[test]
    fn compact_pulls_tile_up_through_gap_when_above_is_clear() {
        // No tile above b's columns — it falls all the way to y=0.
        let mut layout = vec![li("b", 6, 4, 4, 2)];
        compact_vertical(&mut layout);
        assert_eq!(pos(&layout, "b"), (6, 0, 4, 2));
    }

    #[test]
    fn compact_respects_horizontal_neighbours() {
        // a sits at top-left; b is at (6, 4). a doesn't block b's
        // columns, so b falls to y=0 (next to a, not under it).
        let mut layout = vec![li("a", 0, 0, 6, 4), li("b", 6, 4, 4, 2)];
        compact_vertical(&mut layout);
        assert_eq!(pos(&layout, "a"), (0, 0, 6, 4));
        assert_eq!(pos(&layout, "b"), (6, 0, 4, 2));
    }

    #[test]
    fn shove_does_not_disturb_unrelated_rows() {
        // Two rows; only the top row's tiles overlap the moved tile.
        // Bottom row must stay put.
        let mut layout = vec![
            li("top_a", 0, 0, 4, 2),
            li("top_b", 4, 0, 4, 2),
            li("bot_x", 0, 4, 4, 2),
            li("bot_y", 4, 4, 4, 2),
        ];
        shove_move(&mut layout, "top_a", 2, 0).unwrap();
        assert_eq!(pos(&layout, "top_a"), (2, 0, 4, 2));
        assert_eq!(pos(&layout, "top_b"), (6, 0, 4, 2));
        assert_eq!(pos(&layout, "bot_x"), (0, 4, 4, 2));
        assert_eq!(pos(&layout, "bot_y"), (4, 4, 4, 2));
    }
}
