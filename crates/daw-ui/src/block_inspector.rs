//! A selected block's settings in the inspector (Parameters and Signals):
//! depth and fades on sliders, repeat and nudge dragged sideways, invert
//! as a switch. Drags go through the block drag, so the lanes show them,
//! they are heard as they move, and each lands as one edit.

use crate::block_edit::Setting;
use crate::inspector::{SLIDER_GAP, VALUE_WIDTH, group, labeled_row, section_label, slider_length};
use crate::project::ProjectView;
use crate::slider::{self, Axis};
use crate::theme::{MONO_FONT, Theme};
use daw_engine::{Block, BlockId, Command, Edit, Ticks};
use gpui::{Context, Div, MouseButton, MouseDownEvent, div, prelude::*, px};

impl ProjectView {
    /// The selected block's settings, if a block is selected on this page.
    pub(crate) fn block_settings(&self, theme: &Theme, cx: &mut Context<Self>) -> Option<Div> {
        let (id, block, windows) = self.inspected_block()?;
        let dragged = self.dragged_block_setting();
        let mut rows = vec![
            self.block_slider(id, &block, Setting::Depth, dragged, false, theme, cx),
            self.block_slider(id, &block, Setting::FadeIn, dragged, true, theme, cx),
            self.block_slider(id, &block, Setting::FadeOut, dragged, true, theme, cx),
        ];
        // A window has no repeat: the signal decides what plays through it.
        if !windows {
            rows.push(self.block_scrub(id, &block, Setting::Cycle, dragged, theme, cx));
        }
        rows.push(self.block_scrub(id, &block, Setting::Nudge, dragged, theme, cx));
        let invert = crate::switch::switch("block-invert", block.invert, theme)
            .debug_selector(|| "block-invert".into())
            .on_click(cx.listener(move |this, _, _, _| {
                if let Some((_, block, _)) = this.inspected_block() {
                    this.set_block(
                        id,
                        Block {
                            invert: !block.invert,
                            ..block
                        },
                    );
                }
            }));
        rows.push(
            labeled_row("Invert", true, theme)
                .justify_between()
                .child(invert),
        );
        Some(
            div()
                .flex()
                .flex_col()
                .child(section_label("BLOCK", theme))
                .child(group(theme).children(rows)),
        )
    }

    fn set_block(&mut self, id: BlockId, block: Block) {
        self.session
            .apply(Command::Edit(Edit::SetBlock { id, block }));
    }

    /// A setting on a slider, its value beside it. Double-click resets it.
    #[allow(clippy::too_many_arguments, reason = "plain row data")]
    fn block_slider(
        &self,
        id: BlockId,
        block: &Block,
        setting: Setting,
        dragged: Option<Setting>,
        separated: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Div {
        let length = slider_length(self.inspector_width);
        let position = setting.position(block).unwrap_or(0.) as f32;
        let control = slider::slider(
            ("block-slider", setting as usize),
            Axis::Horizontal,
            position,
            length,
            dragged == Some(setting),
            theme,
        )
        .debug_selector(move || selector(setting).into())
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, event: &MouseDownEvent, _, _| {
                if event.click_count >= 2 {
                    if let Some((_, block, _)) = this.inspected_block() {
                        this.set_block(id, reset(setting, block));
                    }
                    return;
                }
                this.start_block_setting(id, setting, event.position, Some(length));
            }),
        );
        labeled_row(setting.label(), separated, theme)
            .gap(px(SLIDER_GAP))
            .child(control)
            .child(value(setting.format(block), theme))
    }

    /// A setting dragged sideways (a length with no natural end).
    fn block_scrub(
        &self,
        id: BlockId,
        block: &Block,
        setting: Setting,
        dragged: Option<Setting>,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Div {
        let hover = theme.hover;
        let active = dragged == Some(setting);
        let pill = div()
            .id(("block-scrub", setting as usize))
            .debug_selector(move || selector(setting).into())
            .min_w(px(VALUE_WIDTH))
            .px(px(6.))
            .py(px(1.))
            .rounded(px(4.))
            .bg(if active {
                theme.control_selected
            } else {
                theme.control
            })
            .hover(move |s| s.bg(hover))
            .cursor_ew_resize()
            .flex()
            .justify_end()
            .font_family(MONO_FONT)
            .text_size(px(11.))
            .child(setting.format(block))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, _| {
                    this.start_block_setting(id, setting, event.position, None);
                }),
            );
        labeled_row(setting.label(), true, theme)
            .justify_between()
            .child(pill)
    }
}

/// The test name of a setting's control.
fn selector(setting: Setting) -> &'static str {
    match setting {
        Setting::Depth => "block-depth",
        Setting::FadeIn => "block-fade-in",
        Setting::FadeOut => "block-fade-out",
        Setting::Cycle => "block-repeat",
        Setting::Nudge => "block-nudge",
    }
}

/// `block` with `setting` back at its default: full depth, no fade.
fn reset(setting: Setting, block: Block) -> Block {
    match setting {
        Setting::Depth => Block { depth: 1., ..block },
        Setting::FadeIn => Block {
            fade_in: Ticks::ZERO,
            ..block
        },
        Setting::FadeOut => Block {
            fade_out: Ticks::ZERO,
            ..block
        },
        Setting::Cycle | Setting::Nudge => block,
    }
}

fn value(text: String, theme: &Theme) -> Div {
    div()
        .w(px(VALUE_WIDTH))
        .flex_none()
        .flex()
        .justify_end()
        .font_family(MONO_FONT)
        .text_size(px(11.))
        .text_color(theme.text_secondary)
        .child(text)
}
