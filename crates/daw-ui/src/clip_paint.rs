//! Clips on Arrange, painted (`docs/design/ui-speed.md`, step 4).
//!
//! A lane's clips are one drawing, not an element each: frame, label, notes
//! or waveform. Presses land on the lane, which finds the clip and the part
//! of it under the pointer (`clip_at`) and starts the same gestures the
//! clips had (`ProjectView::press_clip`). The look matches what the clip
//! elements drew: soft fill, outline (accent and thicker when selected),
//! the label in the detail colour, faint while the clip is being moved.
//!
//! A looped clip (`docs/design/clip-loops.md`) draws its repeats softer with
//! a line where each cycle ends, and shows its loop icon in the top-right
//! corner while hovered or selected, as blocks do.

use crate::block_edit::{LOOP_HANDLE, has_loop_handle, loop_band};
use crate::clip_edit::{AnyClip, Grab};
use crate::icons::Icon;
use crate::paint::Bar;
use crate::project::ProjectView;
use crate::theme::{ContentColor, Theme};
use crate::waveform::{self, Reading};
use daw_engine::{ClipId, Peaks};
use gpui::{
    Bounds, ContentMask, FontWeight, Hsla, Pixels, SharedString, Window, canvas, fill, point, px,
    size,
};
use std::sync::Arc;

/// Space above and below a clip in its lane.
pub const CLIP_INSET: f32 = 4.;
/// Height of a clip's label strip.
pub const CLIP_LABEL_HEIGHT: f32 = 16.;
/// Width of the resize zone at each end of a clip, inside it.
pub const EDGE: f32 = 6.;
/// How far past each end a press still takes that end (where no other clip
/// is): the outline is what the eye aims at.
pub const OUTSIDE: f32 = 4.;

/// What a clip shows under its label.
#[derive(Clone)]
pub enum Content {
    /// Notes, placed from the clip's top-left.
    Notes(Vec<Bar>),
    /// A waveform under the label: one run per stretch of the file that
    /// plays (one, or one per repeat of a loop), as pixels from the clip's
    /// left.
    Wave(Vec<(Arc<Peaks>, Reading, f32, f32)>, Hsla),
}

/// One clip as painted, placed from the lane's top-left.
#[derive(Clone)]
pub struct Painted {
    pub id: ClipId,
    pub left: f32,
    pub width: f32,
    pub height: f32,
    pub fill: Hsla,
    pub outline: Hsla,
    pub border: f32,
    pub label: SharedString,
    pub label_color: Hsla,
    pub content: Content,
    /// Where each cycle of a loop ends (pixels from the clip's left).
    pub seams: Vec<f32>,
    /// The softening over repeats, and the line at each cycle's end.
    pub repeat_wash: Hsla,
    pub seam_color: Hsla,
    /// Show the loop icon (hovered or selected).
    pub loop_icon: bool,
    /// Looped: its right end only drags repeats (`clip_at`).
    pub looped: bool,
}

/// The clip and the part of it at `x` pixels from the lane's left and `y`
/// from the clips' top (`height` tall). Later clips win, as they were drawn
/// on top. The ends resize when the clip is wide enough for both zones and
/// its middle; the top quarter of the right end, and the loop icon beside
/// it, drag out repeats (as on blocks). Just outside a clip, its nearest
/// end resizes (any clip, however narrow). Clips come as (id, left, width,
/// looped): a looped clip's right end only drags repeats, never resizes.
pub fn clip_at(
    clips: &[(ClipId, f32, f32, bool)],
    x: f32,
    y: f32,
    height: f32,
) -> Option<(ClipId, Grab)> {
    inside_clip(clips, x, y, height).or_else(|| beside_clip(clips, x))
}

/// The end of a clip just outside it, the nearest if two are close.
fn beside_clip(clips: &[(ClipId, f32, f32, bool)], x: f32) -> Option<(ClipId, Grab)> {
    clips
        .iter()
        .rev()
        .filter_map(|&(id, left, width, looped)| {
            let right = left + width;
            if (right..right + OUTSIDE).contains(&x) {
                let end = if looped { Grab::Loop } else { Grab::End };
                Some((x - right, (id, end)))
            } else if (left - OUTSIDE..left).contains(&x) {
                Some((left - x, (id, Grab::Start)))
            } else {
                None
            }
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, hit)| hit)
}

fn inside_clip(
    clips: &[(ClipId, f32, f32, bool)],
    x: f32,
    y: f32,
    height: f32,
) -> Option<(ClipId, Grab)> {
    clips
        .iter()
        .rev()
        .find(|(_, left, width, _)| x >= *left && x < left + width)
        .map(|&(id, left, width, looped)| {
            let edges = width >= 3. * EDGE;
            let right = left + width;
            let in_band = (0.0..loop_band(height)).contains(&y);
            let loop_from = if has_loop_handle(width) {
                right - EDGE - LOOP_HANDLE
            } else {
                right - EDGE
            };
            let grab = if edges && in_band && x >= loop_from {
                Grab::Loop
            } else if edges && x < left + EDGE {
                Grab::Start
            } else if edges && x >= right - EDGE {
                if looped { Grab::Loop } else { Grab::End }
            } else {
                Grab::Body
            };
            (id, grab)
        })
}

/// Fades a colour for a clip being moved.
fn faint(mut color: Hsla, moving: bool) -> Hsla {
    if moving {
        color.a *= 0.3;
    }
    color
}

impl ProjectView {
    /// How a clip on Arrange paints: its frame, label, contents and loop.
    pub(crate) fn painted_clip(
        &self,
        id: ClipId,
        clip: &AnyClip,
        label: &str,
        color: ContentColor,
        theme: &Theme,
    ) -> Painted {
        let selected = self.selected_clips.contains(&id);
        let moving = self
            .clip_drag
            .as_ref()
            .is_some_and(|d| d.started() && d.moving().contains(&id));
        self.clip_look(id, clip, label, color, selected, moving, theme)
    }

    /// A clip's painted look, `selected` (accent outline) or not, faint if
    /// `moving`: on the lanes, and as a drag's ghost (loop look and all).
    #[allow(clippy::too_many_arguments, reason = "a clip's look is all of these")]
    pub(crate) fn clip_look(
        &self,
        id: ClipId,
        clip: &AnyClip,
        label: &str,
        color: ContentColor,
        selected: bool,
        moving: bool,
        theme: &Theme,
    ) -> Painted {
        let zoom = self.arrange_view.zoom;
        let height = zoom.row_height - 2. * CLIP_INSET;
        let (label, content) = match clip {
            AnyClip::Midi(c) => (
                label.to_owned(),
                Content::Notes(
                    crate::clip_drag::note_bars(c, color.detail, zoom)
                        .into_iter()
                        .map(|b| Bar {
                            color: faint(b.color, moving),
                            ..b
                        })
                        .collect(),
                ),
            ),
            AnyClip::Audio(c) => {
                let snapshot = self.session.snapshot();
                let bpm = snapshot.tempo_bpm;
                let sample = snapshot.samples.iter().find(|s| s.id == c.sample);
                let name = sample.map_or(label, |s| s.name.as_str());
                // The tag says when the clip is being stretched.
                let tag = if c.ratio(bpm) == 1.0 {
                    name.to_owned()
                } else {
                    format!("{name} · {} BPM", crate::clip_drag::format_bpm(c.bpm))
                };
                let wave = sample.and_then(|s| s.peaks.clone()).map_or(
                    Content::Notes(Vec::new()),
                    |peaks| {
                        // One run per stretch of the file: a loop's repeats.
                        let runs = c
                            .pieces(bpm)
                            .iter()
                            .map(|piece| {
                                let from = zoom.beat_to_x((piece.start - c.start).as_beats());
                                let to = from + zoom.beat_to_x(piece.length.as_beats());
                                (
                                    peaks.clone(),
                                    Reading::of(piece, bpm, zoom.pixels_per_beat),
                                    from,
                                    to,
                                )
                            })
                            .collect();
                        Content::Wave(runs, faint(color.detail, moving))
                    },
                );
                (tag, wave)
            }
        };
        let width = zoom.beat_to_x(clip.length().as_beats());
        let seams = clip.looping().map_or_else(Vec::new, |l| {
            l.seams(clip.length())
                .into_iter()
                .map(|t| zoom.beat_to_x(t.as_beats()))
                .collect()
        });
        let mut repeat_wash = theme.window;
        repeat_wash.a = 0.45;
        let mut seam_color = color.detail;
        seam_color.a *= 0.7;
        Painted {
            id,
            left: zoom.beat_to_x(clip.start().as_beats()),
            width,
            height,
            fill: faint(color.fill, moving),
            outline: faint(
                if selected {
                    theme.accent
                } else {
                    color.outline
                },
                moving,
            ),
            border: if selected { 2. } else { 1. },
            label: label.into(),
            label_color: faint(color.detail, moving),
            content,
            seams,
            repeat_wash: faint(repeat_wash, moving),
            seam_color: faint(seam_color, moving),
            // Hovered clips show it too (the lanes set that, `show_loop_icon`).
            loop_icon: selected && width >= 3. * EDGE,
            looped: clip.looping().is_some(),
        }
    }
}

/// Shows the loop icon on the hovered clip, where it fits.
pub fn show_loop_icon(clips: &mut [Painted], hovered: ClipId) {
    for clip in clips.iter_mut().filter(|c| c.id == hovered) {
        clip.loop_icon = clip.width >= 3. * EDGE;
    }
}

/// One canvas painting a lane's clips; give it the lane's size.
pub fn clips_canvas(clips: Vec<Painted>) -> gpui::Canvas<()> {
    canvas(
        |_, _, _| {},
        move |bounds, (), window, cx| {
            let visible = window.content_mask().bounds;
            for clip in &clips {
                let rect = Bounds::new(
                    point(bounds.left() + px(clip.left), bounds.top() + px(CLIP_INSET)),
                    size(px(clip.width), px(clip.height)),
                );
                if !rect.intersects(&visible) {
                    continue;
                }
                paint_clip(clip, rect, window, cx);
            }
        },
    )
}

fn paint_clip(clip: &Painted, rect: Bounds<Pixels>, window: &mut Window, cx: &mut gpui::App) {
    window.paint_quad(
        fill(rect, clip.fill)
            .corner_radii(px(5.))
            .border_widths(px(clip.border))
            .border_color(clip.outline),
    );
    // Inside the frame only, as the clip elements clipped their contents.
    let inner = rect.intersect(&window.content_mask().bounds);
    window.with_content_mask(Some(ContentMask { bounds: inner }), |window| {
        // As the label element drew it: 10 px semibold, 6 px in and 2 px
        // down inside the 1 px border, at the app's usual line height.
        let mut style = window.text_style();
        style.font_weight = FontWeight::SEMIBOLD;
        style.color = clip.label_color;
        let font_size = px(10.);
        style.font_size = font_size.into();
        let line_height = style.line_height_in_pixels(window.rem_size());
        let run = style.to_run(clip.label.len());
        let line = window
            .text_system()
            .shape_line(clip.label.clone(), font_size, &[run], None);
        let origin = point(rect.left() + px(7.), rect.top() + px(3.));
        let _ = line.paint(origin, line_height, window, cx);
        match &clip.content {
            Content::Notes(bars) => crate::paint::paint_bars(bars, rect, window),
            Content::Wave(runs, color) => {
                let top = rect.top() + px(CLIP_LABEL_HEIGHT);
                let height = (rect.size.height - px(CLIP_LABEL_HEIGHT + 2.)).max(px(0.));
                for (peaks, reading, from, to) in runs {
                    let area = Bounds::new(
                        point(rect.left() + px(*from), top),
                        size(px(to - from), height),
                    );
                    waveform::paint(window, area, peaks, *reading, None, *color);
                }
            }
        }
        // Repeats are softer, and every cycle shows its end: clip, clip, clip.
        if let Some(first) = clip.seams.first() {
            let soft = Bounds::new(
                point(rect.left() + px(*first), rect.top()),
                size(rect.size.width - px(*first), rect.size.height),
            );
            window.paint_quad(fill(soft, clip.repeat_wash));
        }
        for x in &clip.seams {
            let line = Bounds::new(
                point(rect.left() + px(*x - 0.75), rect.top()),
                size(px(1.5), rect.size.height),
            );
            window.paint_quad(fill(line, clip.seam_color));
        }
        // The loop icon in the top-right corner, beside the end's zone.
        if clip.loop_icon {
            let band = loop_band(clip.height);
            let glyph = band.min(10.);
            let handle_left = rect.right() - px(EDGE + LOOP_HANDLE);
            let at = Bounds::new(
                point(
                    handle_left + px((LOOP_HANDLE - glyph) / 2.),
                    rect.top() + px((band - glyph) / 2. + 1.),
                ),
                size(px(glyph), px(glyph)),
            );
            let _ = window.paint_svg(
                at,
                Icon::Loop.path().into(),
                gpui::TransformationMatrix::unit(),
                clip.label_color,
                cx,
            );
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Clips 48 px tall; a point in the middle of their height.
    const HEIGHT: f32 = 48.;
    const MIDDLE: f32 = 24.;

    #[test]
    fn a_press_finds_the_clip_and_its_part() {
        let clips = [(ClipId(1), 0., 100., false), (ClipId(2), 100., 10., false)];
        let at = |x| clip_at(&clips, x, MIDDLE, HEIGHT);
        assert_eq!(at(50.), Some((ClipId(1), Grab::Body)));
        assert_eq!(at(2.), Some((ClipId(1), Grab::Start)), "left edge");
        assert_eq!(at(97.), Some((ClipId(1), Grab::End)), "right edge");
        assert_eq!(
            at(104.),
            Some((ClipId(2), Grab::Body)),
            "too narrow for edges: the body"
        );
        assert_eq!(at(120.), None, "empty lane");
    }

    #[test]
    fn the_top_of_the_right_end_and_the_loop_icon_drag_out_repeats() {
        let clips = [(ClipId(1), 0., 100., false)];
        assert_eq!(
            clip_at(&clips, 97., 2., HEIGHT),
            Some((ClipId(1), Grab::Loop)),
            "top of the right edge"
        );
        assert_eq!(
            clip_at(&clips, 100. - EDGE - LOOP_HANDLE / 2., 2., HEIGHT),
            Some((ClipId(1), Grab::Loop)),
            "the loop icon"
        );
        assert_eq!(
            clip_at(&clips, 97., HEIGHT - 2., HEIGHT),
            Some((ClipId(1), Grab::End)),
            "lower on the edge resizes"
        );
        assert_eq!(
            clip_at(&clips, 2., 2., HEIGHT),
            Some((ClipId(1), Grab::Start)),
            "the left edge never loops"
        );
    }

    #[test]
    fn a_press_just_outside_an_end_still_grabs_it() {
        // The outline is what the eye aims at: a press on it or a little
        // past it takes that end, not the empty lane.
        let clips = [(ClipId(1), 50., 100., false)];
        let at = |x| clip_at(&clips, x, MIDDLE, HEIGHT);
        assert_eq!(at(150. + OUTSIDE - 1.), Some((ClipId(1), Grab::End)));
        assert_eq!(at(50. - OUTSIDE + 1.), Some((ClipId(1), Grab::Start)));
        assert_eq!(at(150. + OUTSIDE + 1.), None, "further out is empty lane");
        assert_eq!(
            clip_at(&clips, 151., 2., HEIGHT),
            Some((ClipId(1), Grab::End)),
            "outside, the top of the end resizes too"
        );
    }

    #[test]
    fn a_clip_too_narrow_for_edges_resizes_from_just_outside() {
        let clips = [(ClipId(1), 100., 10., false)];
        let at = |x| clip_at(&clips, x, MIDDLE, HEIGHT);
        assert_eq!(at(105.), Some((ClipId(1), Grab::Body)), "inside moves it");
        assert_eq!(at(111.), Some((ClipId(1), Grab::End)));
        assert_eq!(at(98.), Some((ClipId(1), Grab::Start)));
    }

    #[test]
    fn where_clips_meet_inside_wins_over_outside() {
        // Back to back: just past the first's end is the second's start.
        let clips = [(ClipId(1), 0., 100., false), (ClipId(2), 100., 100., false)];
        let at = |x| clip_at(&clips, x, MIDDLE, HEIGHT);
        assert_eq!(at(99.), Some((ClipId(1), Grab::End)));
        assert_eq!(at(101.), Some((ClipId(2), Grab::Start)));
    }

    #[test]
    fn a_looped_clips_end_only_loops() {
        // Its right end drags repeats in or out, top to bottom, on the
        // outline and just past it; its start still trims.
        let clips = [(ClipId(1), 0., 100., true)];
        let at = |x, y| clip_at(&clips, x, y, HEIGHT);
        assert_eq!(at(97., HEIGHT - 2.), Some((ClipId(1), Grab::Loop)), "low");
        assert_eq!(at(97., MIDDLE), Some((ClipId(1), Grab::Loop)), "middle");
        assert_eq!(at(102., MIDDLE), Some((ClipId(1), Grab::Loop)), "just past");
        assert_eq!(at(2., MIDDLE), Some((ClipId(1), Grab::Start)));
        assert_eq!(at(50., MIDDLE), Some((ClipId(1), Grab::Body)));
        // Too narrow for inner zones: from just past its end.
        let narrow = [(ClipId(2), 100., 10., true)];
        assert_eq!(
            clip_at(&narrow, 111., MIDDLE, HEIGHT),
            Some((ClipId(2), Grab::Loop))
        );
    }

    #[test]
    fn a_later_clip_wins_where_they_meet() {
        let clips = [(ClipId(1), 0., 100., false), (ClipId(2), 90., 50., false)];
        assert_eq!(
            clip_at(&clips, 95., MIDDLE, HEIGHT).map(|(id, _)| id),
            Some(ClipId(2))
        );
    }
}
