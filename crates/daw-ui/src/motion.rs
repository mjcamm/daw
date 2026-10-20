//! Rows that move: Arrange animates every change to which rows show and in
//! what order, so the tree's flow reads as motion.
//!
//! Each frame the page passes the rows it should show; [`RowMotion`]
//! compares them with the last ones. New rows grow open; rows that are gone
//! stay as ghosts while they close up (anchored at their bottom edge, so
//! folded children slide down into their folder); rows whose order changed
//! glide from where they were. Pure, so it is tested with made-up times.

use daw_engine::{NodeId, NodeSnapshot};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// How long every row animation takes.
pub const DURATION: Duration = Duration::from_millis(200);

#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    Appear,
    Disappear,
    /// Glides from `rows` row-heights away (positive: it was further down).
    Glide {
        rows: f32,
    },
}

#[derive(Debug, Clone)]
struct Anim {
    kind: Kind,
    start: Instant,
}

/// A row that is gone but still closing up.
#[derive(Debug, Clone)]
struct Ghost {
    node: Rc<NodeSnapshot>,
    /// The row it follows: the nearest row before it that still shows.
    after: Option<NodeId>,
}

/// One row as drawn this frame.
#[derive(Debug, Clone)]
pub struct RowLayout {
    pub id: NodeId,
    /// The node's last state, for a row that is gone; `None` for a live row
    /// (read it from the snapshot).
    pub ghost: Option<Rc<NodeSnapshot>>,
    /// Height drawn, from 0 to the full row height.
    pub height: f32,
    /// Drawn this far from its place (gliding).
    pub offset: f32,
    pub opacity: f32,
}

/// A size that eases toward its target instead of jumping, so a map that
/// gains or loses a level does not jolt everything beside it.
#[derive(Debug, Default)]
pub struct Eased(std::cell::Cell<Option<f32>>);

impl Eased {
    /// The size to draw this frame, a step closer to `target`.
    pub fn toward(&self, target: f32) -> f32 {
        let next = match self.0.get() {
            Some(current) if (target - current).abs() > 0.5 => current + (target - current) * 0.25,
            _ => target,
        };
        self.0.set(Some(next));
        next
    }
}

#[derive(Debug, Default)]
pub struct RowMotion {
    /// The rows last shown, in order.
    shown: Vec<NodeId>,
    started: bool,
    anims: HashMap<NodeId, Anim>,
    ghosts: Vec<Ghost>,
}

/// Fast at first, settling gently.
fn ease_out(t: f32) -> f32 {
    1. - (1. - t.clamp(0., 1.)).powi(3)
}

impl RowMotion {
    /// Takes the rows that should show now and starts animations for what
    /// changed. The first call only records them: a page opening does not
    /// animate.
    /// `last` finds a row's last state, for rows that go (from the current
    /// snapshot, or the one before it for a removed node).
    pub fn update(
        &mut self,
        rows: &[&NodeSnapshot],
        last: impl Fn(NodeId) -> Option<NodeSnapshot>,
        now: Instant,
    ) {
        self.finish(now);
        let ids: Vec<NodeId> = rows.iter().map(|n| n.id).collect();
        if !self.started {
            self.started = true;
            self.shown = ids;
            return;
        }
        if ids == self.shown {
            return;
        }
        let old = std::mem::replace(&mut self.shown, ids.clone());
        let is_new = |id: &NodeId| !old.contains(id);
        let is_gone = |id: &NodeId| !ids.contains(id);

        for id in ids.iter().filter(|id| is_new(id)) {
            self.ghosts.retain(|g| g.node.id != *id);
            self.anims.insert(
                *id,
                Anim {
                    kind: Kind::Appear,
                    start: now,
                },
            );
        }
        for (index, id) in old.iter().enumerate().filter(|(_, id)| is_gone(id)) {
            let after = old[..index]
                .iter()
                .rev()
                .find(|id| ids.contains(id))
                .copied();
            if let Some(node) = last(*id) {
                self.ghosts.push(Ghost {
                    node: Rc::new(node),
                    after,
                });
                self.anims.insert(
                    *id,
                    Anim {
                        kind: Kind::Disappear,
                        start: now,
                    },
                );
            }
        }
        // Rows that stayed but changed order glide from where they were.
        let kept_old: Vec<NodeId> = old.iter().filter(|id| ids.contains(id)).copied().collect();
        let kept_new: Vec<NodeId> = ids.iter().filter(|id| old.contains(id)).copied().collect();
        if kept_old != kept_new {
            for (new_index, id) in kept_new.iter().enumerate() {
                let old_index = kept_old.iter().position(|o| o == id).unwrap_or(new_index);
                let rows = old_index as f32 - new_index as f32;
                if rows != 0. {
                    self.anims.insert(
                        *id,
                        Anim {
                            kind: Kind::Glide { rows },
                            start: now,
                        },
                    );
                }
            }
        }
    }

    /// Drops animations that have run their course.
    fn finish(&mut self, now: Instant) {
        self.anims
            .retain(|_, anim| now.duration_since(anim.start) < DURATION);
        let anims = &self.anims;
        self.ghosts.retain(|g| anims.contains_key(&g.node.id));
    }

    /// Whether anything is moving.
    pub fn animating(&self) -> bool {
        !self.anims.is_empty()
    }

    /// The rows to draw now, ghosts included, each with its height, offset
    /// and opacity.
    pub fn layout(&self, rows: &[&NodeSnapshot], row_height: f32, now: Instant) -> Vec<RowLayout> {
        let progress = |anim: &Anim| {
            ease_out(now.duration_since(anim.start).as_secs_f32() / DURATION.as_secs_f32())
        };
        let row = |id: NodeId, ghost: Option<Rc<NodeSnapshot>>| {
            let (mut height, mut offset, mut opacity) = (row_height, 0., 1.);
            if let Some(anim) = self.anims.get(&id) {
                let t = progress(anim);
                match anim.kind {
                    Kind::Appear => {
                        height = row_height * t;
                        opacity = t;
                    }
                    Kind::Disappear => {
                        height = row_height * (1. - t);
                        opacity = 1. - t;
                    }
                    Kind::Glide { rows } => offset = rows * row_height * (1. - t),
                }
            }
            RowLayout {
                id,
                ghost,
                height,
                offset,
                opacity,
            }
        };
        let ghosts_after = |after: Option<NodeId>| {
            self.ghosts
                .iter()
                .filter(move |g| g.after == after)
                .map(|g| row(g.node.id, Some(g.node.clone())))
        };
        let mut out: Vec<RowLayout> = ghosts_after(None).collect();
        for node in rows {
            out.push(row(node.id, None));
            out.extend(ghosts_after(Some(node.id)));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: u64) -> NodeSnapshot {
        NodeSnapshot {
            id: NodeId(id),
            kind: daw_engine::NodeKind::Midi,
            parent: None,
            depth: 1,
            children: Vec::new(),
            name: format!("N{id}"),
            color: 0,
            muted: false,
            soloed: false,
            clips: Vec::new(),
            audio_clips: Vec::new(),
            devices: Vec::new(),
        }
    }

    fn step(
        motion: &mut RowMotion,
        before: &[&NodeSnapshot],
        after: &[&NodeSnapshot],
        at: Instant,
    ) {
        let last = |id: NodeId| before.iter().find(|n| n.id == id).map(|n| (*n).clone());
        motion.update(after, last, at);
    }

    fn heights(layout: &[RowLayout]) -> Vec<(u64, f32)> {
        layout.iter().map(|r| (r.id.0, r.height.round())).collect()
    }

    #[test]
    fn opening_the_page_does_not_animate() {
        let (a, b) = (node(1), node(2));
        let mut motion = RowMotion::default();
        let now = Instant::now();
        step(&mut motion, &[], &[&a, &b], now);
        assert!(!motion.animating());
        assert_eq!(
            heights(&motion.layout(&[&a, &b], 50., now)),
            [(1, 50.), (2, 50.)]
        );
    }

    #[test]
    fn new_rows_grow_open() {
        let (a, b) = (node(1), node(2));
        let mut motion = RowMotion::default();
        let now = Instant::now();
        step(&mut motion, &[], &[&a], now);
        step(&mut motion, &[&a], &[&a, &b], now);
        assert_eq!(
            heights(&motion.layout(&[&a, &b], 50., now)),
            [(1, 50.), (2, 0.)]
        );
        let later = now + DURATION;
        step(&mut motion, &[&a, &b], &[&a, &b], later);
        assert!(!motion.animating());
        assert_eq!(
            heights(&motion.layout(&[&a, &b], 50., later)),
            [(1, 50.), (2, 50.)]
        );
    }

    #[test]
    fn gone_rows_close_up_where_they_were() {
        // Folding: children 1 and 2 go into folder 3, after row 0.
        let (z, a, b, f) = (node(10), node(1), node(2), node(3));
        let mut motion = RowMotion::default();
        let now = Instant::now();
        step(&mut motion, &[], &[&z, &a, &b, &f], now);
        step(&mut motion, &[&z, &a, &b, &f], &[&z, &f], now);
        let layout = motion.layout(&[&z, &f], 50., now);
        assert_eq!(
            heights(&layout),
            [(10, 50.), (1, 50.), (2, 50.), (3, 50.)],
            "ghosts start full, in their old places"
        );
        assert!(layout[1].ghost.is_some() && layout[3].ghost.is_none());
        let halfway = now + DURATION / 2;
        let layout = motion.layout(&[&z, &f], 50., halfway);
        assert!(layout[1].height < 25., "closing fast at first");
        step(&mut motion, &[&z, &f], &[&z, &f], now + DURATION);
        assert_eq!(
            heights(&motion.layout(&[&z, &f], 50., now + DURATION)),
            [(10, 50.), (3, 50.)]
        );
    }

    #[test]
    fn reordered_rows_glide_from_where_they_were() {
        let (a, b, c) = (node(1), node(2), node(3));
        let mut motion = RowMotion::default();
        let now = Instant::now();
        step(&mut motion, &[], &[&a, &b, &c], now);
        // Row 3 moves to the top.
        step(&mut motion, &[&a, &b, &c], &[&c, &a, &b], now);
        let layout = motion.layout(&[&c, &a, &b], 50., now);
        let offsets: Vec<f32> = layout.iter().map(|r| r.offset).collect();
        assert_eq!(offsets, [100., -50., -50.]);
        let layout = motion.layout(&[&c, &a, &b], 50., now + DURATION);
        assert!(layout.iter().all(|r| r.offset == 0.));
    }

    #[test]
    fn sizes_ease_toward_their_target() {
        let size = Eased::default();
        assert_eq!(size.toward(100.), 100., "the first frame is exact");
        let next = size.toward(60.);
        assert!(next < 100. && next > 60.);
        for _ in 0..40 {
            size.toward(60.);
        }
        assert_eq!(size.toward(60.), 60.);
    }

    #[test]
    fn easing_is_fast_then_gentle() {
        assert_eq!(ease_out(0.), 0.);
        assert_eq!(ease_out(1.), 1.);
        assert!(ease_out(0.5) > 0.8);
    }
}
