//! Clips on Arrange, painted (`docs/design/ui-speed.md`, step 4).
//!
//! A lane's clips are one drawing, not an element each: frame, label, notes
//! or waveform. Presses land on the lane, which finds the clip and the part
//! of it under the pointer (`clip_at`) and starts the same gestures the
//! clips had (`ProjectView::press_clip`). The look matches what the clip
//! elements drew: soft fill, outline (accent and thicker when selected),
//! the label in the detail colour, faint while the clip is being moved.

use crate::clip_edit::{AnyClip, Grab};
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
/// Width of the resize zone at each end of a clip.
pub const EDGE: f32 = 6.;

/// What a clip shows under its label.
#[derive(Clone)]
pub enum Content {
    /// Notes, placed from the clip's top-left.
    Notes(Vec<Bar>),
    /// A waveform under the label.
    Wave(Arc<Peaks>, Reading, Hsla),
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
}

/// The clip and the part of it at `x` pixels from the lane's left (later
/// clips win, as they were drawn on top). The ends resize when the clip is
/// wide enough for both zones and its middle.
pub fn clip_at(clips: &[(ClipId, f32, f32)], x: f32) -> Option<(ClipId, Grab)> {
    clips
        .iter()
        .rev()
        .find(|(_, left, width)| x >= *left && x < left + width)
        .map(|&(id, left, width)| {
            let edges = width >= 3. * EDGE;
            let grab = if edges && x < left + EDGE {
                Grab::Start
            } else if edges && x >= left + width - EDGE {
                Grab::End
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
    /// How a clip on Arrange paints: its frame, label and contents.
    pub(crate) fn painted_clip(
        &self,
        id: ClipId,
        clip: &AnyClip,
        label: &str,
        color: ContentColor,
        theme: &Theme,
    ) -> Painted {
        let zoom = self.arrange_view.zoom;
        let selected = self.selected_clips.contains(&id);
        let moving = self
            .clip_drag
            .as_ref()
            .is_some_and(|d| d.started() && d.moving().contains(&id));
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
                        Content::Wave(
                            peaks,
                            Reading::of(c, bpm, zoom.pixels_per_beat),
                            faint(color.detail, moving),
                        )
                    },
                );
                (tag, wave)
            }
        };
        Painted {
            id,
            left: zoom.beat_to_x(clip.start().as_beats()),
            width: zoom.beat_to_x(clip.length().as_beats()),
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
        }
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
            Content::Wave(peaks, reading, color) => {
                let area = Bounds::new(
                    point(rect.left(), rect.top() + px(CLIP_LABEL_HEIGHT)),
                    size(
                        rect.size.width,
                        (rect.size.height - px(CLIP_LABEL_HEIGHT + 2.)).max(px(0.)),
                    ),
                );
                waveform::paint(window, area, peaks, *reading, None, *color);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_press_finds_the_clip_and_its_part() {
        let clips = [(ClipId(1), 0., 100.), (ClipId(2), 100., 10.)];
        assert_eq!(clip_at(&clips, 50.), Some((ClipId(1), Grab::Body)));
        assert_eq!(
            clip_at(&clips, 2.),
            Some((ClipId(1), Grab::Start)),
            "left edge"
        );
        assert_eq!(
            clip_at(&clips, 97.),
            Some((ClipId(1), Grab::End)),
            "right edge"
        );
        assert_eq!(
            clip_at(&clips, 104.),
            Some((ClipId(2), Grab::Body)),
            "too narrow for edges: the body"
        );
        assert_eq!(clip_at(&clips, 120.), None, "empty lane");
    }

    #[test]
    fn a_later_clip_wins_where_they_meet() {
        let clips = [(ClipId(1), 0., 100.), (ClipId(2), 90., 50.)];
        assert_eq!(clip_at(&clips, 95.).map(|(id, _)| id), Some(ClipId(2)));
    }
}
