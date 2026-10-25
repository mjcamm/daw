//! The live layer: everything that moves on its own every frame (playheads,
//! meters, a fader's automation marker, a parameter's value at the
//! playhead), drawn in one pass above the panes.
//!
//! Panes are cached views that redraw only when they change
//! (`docs/design/ui-speed.md`), so nothing inside them may depend on the
//! song position or the meters. Instead, when a pane renders it places an
//! invisible marker (`ProjectView::live_slot`) wherever a moving thing
//! goes. The marker records its on-screen box and clip when the pane is
//! laid out, and keeps it while the pane is reused. Each frame the live
//! layer paints every recorded slot with the current values.

use crate::layout::{is_over, meter_height};
use crate::theme::{MONO_FONT, Theme};
use daw_automation::Automation;
use daw_engine::{Meter, NodeId};
use gpui::{
    App, Bounds, ContentMask, Hsla, Pixels, SharedString, TextRun, Window, canvas, fill, font,
    point, px, size,
};
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;
use std::time::{Duration, Instant};

/// Which pane placed a slot; a pane's slots go when it redraws.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PaneKind {
    /// The page: Arrange, Parameters, Signals or the mixer.
    Main,
    /// The piano roll or audio panel under Arrange.
    Bottom,
    /// The inspector on the right.
    Inspector,
    /// Arrange's node list, inside the page: lanes redraw without it.
    ArrangeList,
}

/// What a slot shows.
#[derive(Clone)]
pub enum Live {
    /// A one-pixel line at the song position across the slot. The slot's
    /// left edge is `start` beats; `ppb` pixels per beat.
    Playhead { start: f64, ppb: f32 },
    /// A node's stereo meter: two bars side by side.
    Meter { node: NodeId, bar: f32, gap: f32 },
    /// A mixer meter: two bars and a peak line that holds, then falls.
    StripMeter { node: NodeId, bar: f32, gap: f32 },
    /// A fader's marker where the layers have the volume at the song
    /// position. `travel` is the fader's travel in pixels, `inset` the
    /// space below it.
    FaderMarker {
        automation: Rc<Automation>,
        travel: f32,
        inset: f32,
    },
    /// A signal's level at the song position: a bar filling from the
    /// left as it rises from the centre line to the top.
    Level {
        signal: Rc<daw_automation::CompiledSignal>,
        color: Hsla,
    },
    /// A parameter's value at the song position, as text, right-aligned.
    Value {
        automation: Rc<Automation>,
        format: Rc<dyn Fn(f64) -> String>,
    },
}

struct Slot {
    pane: PaneKind,
    live: Live,
    bounds: Bounds<Pixels>,
    clip: Bounds<Pixels>,
}

/// The slots placed by the panes, and the meters' peak hold.
#[derive(Default)]
pub struct LiveLayer {
    slots: RefCell<Vec<Slot>>,
    /// Peak held per node on the mixer, and since when.
    peaks: RefCell<HashMap<NodeId, (f32, Instant)>>,
}

/// How long a mixer meter's peak line holds before falling.
const PEAK_HOLD: Duration = Duration::from_millis(1200);

/// What the live layer draws with this frame.
pub struct Now {
    pub beat: f64,
    pub meters: BTreeMap<NodeId, Meter>,
}

impl LiveLayer {
    /// Forgets a pane's slots before it renders them again.
    pub fn clear(&self, pane: PaneKind) {
        self.slots.borrow_mut().retain(|s| s.pane != pane);
    }

    /// Forgets the slots of panes not on screen this frame.
    pub fn keep_only(&self, shown: &[PaneKind]) {
        self.slots.borrow_mut().retain(|s| shown.contains(&s.pane));
    }

    fn record(&self, pane: PaneKind, live: Live, bounds: Bounds<Pixels>, clip: Bounds<Pixels>) {
        self.slots.borrow_mut().push(Slot {
            pane,
            live,
            bounds,
            clip,
        });
    }

    /// Whether any meter shows something, so frames are still needed.
    pub fn meters_moving(&self, now: &Now) -> bool {
        now.meters
            .values()
            .any(|m| meter_height(m.left.max(m.right)) > 0.)
            || !self.peaks.borrow().is_empty()
    }

    /// Paints every slot.
    pub fn paint(&self, now: &Now, theme: &Theme, window: &mut Window, cx: &mut App) {
        let slots = self.slots.borrow();
        for slot in slots.iter() {
            // Text may reach past its box to the left; the rest stay in it.
            let area = match slot.live {
                Live::Value { .. } => slot.clip,
                _ => slot.clip.intersect(&slot.bounds),
            };
            if area.size.width <= px(0.) || area.size.height <= px(0.) {
                continue;
            }
            let clip = area;
            window.with_content_mask(Some(ContentMask { bounds: clip }), |window| {
                self.paint_slot(slot, now, theme, window, cx);
            });
        }
        // Peaks of nodes no longer shown are forgotten.
        let shown: Vec<NodeId> = slots
            .iter()
            .filter_map(|s| match s.live {
                Live::StripMeter { node, .. } => Some(node),
                _ => None,
            })
            .collect();
        self.peaks.borrow_mut().retain(|n, _| shown.contains(n));
    }

    fn paint_slot(&self, slot: &Slot, now: &Now, theme: &Theme, window: &mut Window, cx: &mut App) {
        let b = slot.bounds;
        match &slot.live {
            Live::Playhead { start, ppb } => {
                let x = b.left() + px(((now.beat - start) as f32) * ppb);
                window.paint_quad(fill(
                    Bounds::new(point(x, b.top()), size(px(1.), b.size.height)),
                    theme.accent,
                ));
            }
            Live::Meter { node, bar, gap } => {
                let meter = now.meters.get(node).copied().unwrap_or_default();
                for (i, peak) in [meter.left, meter.right].into_iter().enumerate() {
                    let left = b.left() + px(i as f32 * (bar + gap));
                    meter_bar(window, theme, left, b.top(), *bar, b.size.height, peak);
                }
            }
            Live::StripMeter { node, bar, gap } => {
                let meter = now.meters.get(node).copied().unwrap_or_default();
                for (i, peak) in [meter.left, meter.right].into_iter().enumerate() {
                    let left = b.left() + px(i as f32 * (bar + gap));
                    meter_bar(window, theme, left, b.top(), *bar, b.size.height, peak);
                }
                let level = meter.left.max(meter.right);
                let held = {
                    let mut peaks = self.peaks.borrow_mut();
                    let entry = peaks.entry(*node).or_insert((0., Instant::now()));
                    if level >= entry.0 || entry.1.elapsed() > PEAK_HOLD {
                        *entry = (level, Instant::now());
                    }
                    let held = entry.0;
                    // Below what a meter shows: nothing to hold.
                    if meter_height(held) <= 0. {
                        peaks.remove(node);
                    }
                    held
                };
                let hold = meter_height(held);
                if hold > 0. {
                    let y = b.bottom() - px(b.size.height / px(1.) * hold);
                    // Red at or over 0 dB.
                    let color = if is_over(held) {
                        theme.meter_high
                    } else {
                        theme.text_secondary
                    };
                    window.paint_quad(fill(
                        Bounds::new(point(b.left(), y), size(b.size.width, px(1.))),
                        color,
                    ));
                }
            }
            Live::FaderMarker {
                automation,
                travel,
                inset,
            } => {
                let value = automation.value_at(now.beat).clamp(0., 1.) as f32;
                let bottom = inset + value * travel - 1.;
                let y = b.bottom() - px(bottom) - px(2.);
                window.paint_quad(
                    fill(
                        Bounds::new(point(b.left(), y), size(b.size.width, px(2.))),
                        theme.warning,
                    )
                    .corner_radii(px(1.)),
                );
            }
            Live::Level { signal, color } => {
                window.paint_quad(fill(b, theme.control).corner_radii(px(3.)));
                let level = ((signal.sample(now.beat).0 - 0.5) * 2.).clamp(0., 1.) as f32;
                if level > 0. {
                    let filled = Bounds::new(b.origin, size(b.size.width * level, b.size.height));
                    window.paint_quad(fill(filled, *color).corner_radii(px(3.)));
                }
            }
            Live::Value { automation, format } => {
                let text: SharedString = format(automation.value_at(now.beat)).into();
                let font_size = px(11.);
                let run = TextRun {
                    len: text.len(),
                    font: font(MONO_FONT),
                    color: theme.text_secondary,
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                let line = window
                    .text_system()
                    .shape_line(text, font_size, &[run], None);
                let line_height = window.line_height();
                let origin = point(
                    b.right() - line.width,
                    b.top() + (b.size.height - line_height) / 2.,
                );
                let _ = line.paint(origin, line_height, window, cx);
            }
        }
    }

    /// An invisible marker that records where `live` goes when the pane
    /// containing it is laid out. Give it a size (or `absolute` and
    /// `size_full`) where it sits.
    pub fn slot(self: &Rc<Self>, pane: PaneKind, live: Live) -> gpui::Canvas<()> {
        let layer = self.clone();
        canvas(
            move |bounds, window, _| {
                let clip = window.content_mask().bounds;
                layer.record(pane, live, bounds, clip);
            },
            |_, (), _, _| {},
        )
    }
}

/// One meter bar, bottom-aligned, as the old element meters drew it.
fn meter_bar(
    window: &mut Window,
    theme: &Theme,
    left: Pixels,
    top: Pixels,
    width: f32,
    height: Pixels,
    peak: f32,
) {
    let track = Bounds::new(point(left, top), size(px(width), height));
    window.paint_quad(fill(track, theme.control).corner_radii(px(1.)));
    let level = meter_height(peak);
    if level > 0. {
        let filled = px(height / px(1.) * level);
        let color: Hsla = if is_over(peak) {
            theme.meter_high
        } else {
            theme.meter_low
        };
        window.paint_quad(
            fill(
                Bounds::new(point(left, top + height - filled), size(px(width), filled)),
                color,
            )
            .corner_radii(px(1.)),
        );
    }
}
