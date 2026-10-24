//! File → Export…: the song to a WAV or MP3 file (`docs/design/export.md`).
//!
//! The dialog chooses the format, quality, range and tail, asks where to
//! save, then renders on a thread of its own while the song plays on. While
//! it runs the dialog keeps its place but lets clicks through to the app
//! around it; Cancel stops it and leaves no file.

use crate::theme::Theme;
use daw_host::export::{
    self, ExportError, ExportJob, ExportSettings, Exported, Format, MP3_AVAILABLE, Mp3Bitrate,
    Progress, Range, Tail, WavDepth,
};
use gpui::{
    App, Context, EventEmitter, FocusHandle, Focusable, FontWeight, SharedString, Window, div,
    prelude::*, px, relative,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

const WIDTH: f32 = 440.;
/// How often a running export shows its progress.
const PROGRESS_EVERY: Duration = Duration::from_millis(50);
/// The folder in the project that exports start in.
pub const EXPORTS_FOLDER: &str = "Exports";

#[derive(Debug, Clone, PartialEq)]
pub enum ExportEvent {
    /// Render to this file (the dialog then expects [`ExportDialog::run`]).
    Start(ExportSettings, PathBuf),
    Close,
}

/// What the dialog shows.
enum Phase {
    Choosing,
    /// The save dialog is open.
    Asking,
    Running {
        path: PathBuf,
        progress: Arc<Progress>,
        thread: Option<JoinHandle<Result<Exported, ExportError>>>,
    },
    Done {
        path: PathBuf,
        exported: Exported,
    },
    Failed(String),
}

/// The song as the dialog describes it.
#[derive(Debug, Clone)]
pub struct SongFacts {
    pub name: String,
    pub folder: PathBuf,
    /// Seconds from the start to the end of the last clip.
    pub song_seconds: f64,
    /// The loop region's length in seconds, if there is one.
    pub loop_seconds: Option<f64>,
    pub sample_rate: u32,
}

pub struct ExportDialog {
    settings: ExportSettings,
    /// Each format's quality, kept while switching between them.
    depth: WavDepth,
    bitrate: Mp3Bitrate,
    song: SongFacts,
    phase: Phase,
    focus: FocusHandle,
}

impl EventEmitter<ExportEvent> for ExportDialog {}

impl Focusable for ExportDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// The settings an export starts with.
pub fn default_settings() -> ExportSettings {
    ExportSettings {
        format: Format::Wav(WavDepth::Int24),
        range: Range::Song,
        tail: Tail::UntilSilent,
    }
}

/// `seconds` as minutes and seconds, "2:05".
pub fn duration(seconds: f64) -> String {
    let whole = seconds.round() as u64;
    format!("{}:{:02}", whole / 60, whole % 60)
}

/// A file name from the song's name: characters files can't hold become
/// spaces.
pub fn file_name(song: &str, format: Format) -> String {
    let cleaned: String = song
        .chars()
        .map(|c| {
            if c.is_control() || r#"<>:"/\|?*"#.contains(c) {
                ' '
            } else {
                c
            }
        })
        .collect();
    let stem = cleaned.trim().trim_end_matches('.');
    let stem = if stem.is_empty() { "Export" } else { stem };
    format!("{stem}.{}", format.extension())
}

/// `path` with the format's extension, added if it has another or none.
pub fn with_extension(path: PathBuf, format: Format) -> PathBuf {
    let ext = format.extension();
    if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case(ext))
    {
        return path;
    }
    let mut name = path.file_name().unwrap_or_default().to_owned();
    name.push(".");
    name.push(ext);
    path.with_file_name(name)
}

impl ExportDialog {
    pub fn new(
        settings: ExportSettings,
        song: SongFacts,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus);
        let (depth, bitrate) = match settings.format {
            Format::Wav(depth) => (depth, Mp3Bitrate::Kbps320),
            Format::Mp3(bitrate) => (WavDepth::Int24, bitrate),
        };
        let mut settings = settings;
        if song.loop_seconds.is_none() {
            settings.range = Range::Song;
        }
        if !MP3_AVAILABLE && matches!(settings.format, Format::Mp3(_)) {
            settings.format = Format::Wav(depth);
        }
        Self {
            settings,
            depth,
            bitrate,
            song,
            phase: Phase::Choosing,
            focus,
        }
    }

    pub fn settings(&self) -> ExportSettings {
        self.settings
    }

    pub fn is_running(&self) -> bool {
        matches!(self.phase, Phase::Running { .. })
    }

    /// The finished file, once done.
    #[cfg(all(test, feature = "ui-tests"))]
    pub fn exported(&self) -> Option<(&Path, Exported)> {
        match &self.phase {
            Phase::Done { path, exported } => Some((path, *exported)),
            _ => None,
        }
    }

    /// Why the export failed, if it did.
    #[cfg(all(test, feature = "ui-tests"))]
    pub fn failure(&self) -> Option<&str> {
        match &self.phase {
            Phase::Failed(why) => Some(why),
            _ => None,
        }
    }

    fn range_seconds(&self) -> f64 {
        match self.settings.range {
            Range::Song => self.song.song_seconds,
            Range::Loop => self.song.loop_seconds.unwrap_or(0.0),
        }
    }

    fn can_export(&self) -> bool {
        self.range_seconds() > 0.0
    }

    fn exports_folder(&self) -> PathBuf {
        self.song.folder.join(EXPORTS_FOLDER)
    }

    /// Export…: asks where to save, starting in the project's Exports
    /// folder, then asks the project to start.
    fn ask_path(&mut self, cx: &mut Context<Self>) {
        if !self.can_export() || !matches!(self.phase, Phase::Choosing) {
            return;
        }
        let folder = self.exports_folder();
        if let Err(e) = std::fs::create_dir_all(&folder) {
            self.phase = Phase::Failed(format!("Could not make the Exports folder: {e}"));
            cx.notify();
            return;
        }
        let format = self.settings.format;
        let chosen = cx.prompt_for_new_path(&folder, Some(&file_name(&self.song.name, format)));
        self.phase = Phase::Asking;
        cx.spawn(async move |this, cx| {
            let path = chosen.await.ok().and_then(Result::ok).flatten();
            let _ = this.update(cx, |this, cx| {
                this.phase = Phase::Choosing;
                if let Some(path) = path {
                    cx.emit(ExportEvent::Start(
                        this.settings,
                        with_extension(path, this.settings.format),
                    ));
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Renders `job` on a thread of its own, showing its progress.
    pub fn run(&mut self, job: ExportJob, cx: &mut Context<Self>) {
        let progress = Arc::new(Progress::default());
        let path = job.path.clone();
        let watched = Arc::clone(&progress);
        let thread = std::thread::Builder::new()
            .name("export".into())
            .spawn(move || export::export(job, &watched));
        let thread = match thread {
            Ok(thread) => thread,
            Err(e) => {
                self.phase = Phase::Failed(format!("Could not start the export: {e}"));
                cx.notify();
                return;
            }
        };
        self.phase = Phase::Running {
            path,
            progress,
            thread: Some(thread),
        };
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(PROGRESS_EVERY).await;
                let running = this.update(cx, |this, cx| {
                    this.check(cx);
                    this.is_running()
                });
                if !matches!(running, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
        cx.notify();
    }

    /// Shows a running export's progress, and its end once it ends.
    pub fn check(&mut self, cx: &mut Context<Self>) {
        let Phase::Running { path, thread, .. } = &mut self.phase else {
            return;
        };
        if thread.as_ref().is_some_and(|t| !t.is_finished()) {
            cx.notify();
            return;
        }
        let path = path.clone();
        let result = thread.take().map(JoinHandle::join);
        self.phase = match result {
            Some(Ok(Ok(exported))) => Phase::Done { path, exported },
            Some(Ok(Err(ExportError::Cancelled))) => Phase::Choosing,
            Some(Ok(Err(e))) => Phase::Failed(format!("Could not export: {e}.")),
            Some(Err(_)) | None => Phase::Failed("The export stopped unexpectedly.".into()),
        };
        cx.notify();
    }

    fn cancel(&mut self) {
        if let Phase::Running { progress, .. } = &self.phase {
            progress.cancel();
        }
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        match self.phase {
            Phase::Running { .. } | Phase::Asking => {}
            _ => cx.emit(ExportEvent::Close),
        }
    }

    fn set_format(&mut self, format: Format, cx: &mut Context<Self>) {
        match format {
            Format::Wav(depth) => self.depth = depth,
            Format::Mp3(bitrate) => self.bitrate = bitrate,
        }
        self.settings.format = format;
        cx.notify();
    }

    /// A row of choices, one selected.
    fn segments<T: Copy + PartialEq + 'static>(
        &self,
        theme: &Theme,
        cx: &mut Context<Self>,
        name: &'static str,
        current: T,
        options: Vec<(T, SharedString, bool)>,
        choose: fn(&mut Self, T, &mut Context<Self>),
    ) -> gpui::Div {
        let hover = theme.hover;
        let buttons: Vec<_> = options
            .into_iter()
            .enumerate()
            .map(|(i, (value, label, enabled))| {
                let selected = value == current;
                div()
                    .id((name, i))
                    .debug_selector(move || format!("export-{name}-{i}"))
                    .flex_1()
                    .flex()
                    .justify_center()
                    .py(px(3.))
                    .rounded(px(5.))
                    .whitespace_nowrap()
                    .when(selected, |d| {
                        d.bg(theme.control_selected).text_color(theme.text)
                    })
                    .when(!selected && enabled, |d| {
                        d.text_color(theme.text_secondary)
                            .hover(move |s| s.bg(hover))
                    })
                    .when(!enabled, |d| d.text_color(theme.text_tertiary))
                    .child(label)
                    .when(enabled, |d| {
                        d.on_click(cx.listener(move |this, _, _, cx| {
                            if matches!(this.phase, Phase::Choosing) {
                                choose(this, value, cx);
                            }
                        }))
                    })
            })
            .collect();
        div()
            .flex_1()
            .flex()
            .p(px(2.))
            .gap(px(2.))
            .rounded(px(7.))
            .bg(theme.control)
            .text_size(px(12.))
            .children(buttons)
    }

    fn render_choices(&self, theme: &Theme, cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        let is_mp3 = matches!(self.settings.format, Format::Mp3(_));
        let format = self.segments(
            theme,
            cx,
            "format",
            is_mp3,
            vec![
                (false, "WAV".into(), true),
                (true, "MP3".into(), MP3_AVAILABLE),
            ],
            |this, mp3, cx| {
                let format = if mp3 {
                    Format::Mp3(this.bitrate)
                } else {
                    Format::Wav(this.depth)
                };
                this.set_format(format, cx);
            },
        );
        let quality = match self.settings.format {
            Format::Wav(depth) => self
                .segments(
                    theme,
                    cx,
                    "depth",
                    depth,
                    vec![
                        (WavDepth::Int16, "16-bit".into(), true),
                        (WavDepth::Int24, "24-bit".into(), true),
                        (WavDepth::Float32, "32-bit float".into(), true),
                    ],
                    |this, depth, cx| this.set_format(Format::Wav(depth), cx),
                )
                .into_any_element(),
            Format::Mp3(bitrate) => self
                .segments(
                    theme,
                    cx,
                    "bitrate",
                    bitrate,
                    [
                        Mp3Bitrate::Kbps192,
                        Mp3Bitrate::Kbps256,
                        Mp3Bitrate::Kbps320,
                    ]
                    .into_iter()
                    .map(|b| (b, format!("{} kbps", b.kbps()).into(), true))
                    .collect(),
                    |this, bitrate, cx| this.set_format(Format::Mp3(bitrate), cx),
                )
                .into_any_element(),
        };
        let loop_label = self
            .song
            .loop_seconds
            .map_or_else(|| "Loop".to_owned(), |s| format!("Loop · {}", duration(s)));
        let range = self.segments(
            theme,
            cx,
            "range",
            self.settings.range,
            vec![
                (
                    Range::Song,
                    format!("Whole song · {}", duration(self.song.song_seconds)).into(),
                    true,
                ),
                (
                    Range::Loop,
                    loop_label.into(),
                    self.song.loop_seconds.is_some(),
                ),
            ],
            |this, range, cx| {
                this.settings.range = range;
                cx.notify();
            },
        );
        let tail = self.segments(
            theme,
            cx,
            "tail",
            self.settings.tail,
            vec![
                (Tail::UntilSilent, "Until silent".into(), true),
                (Tail::Off, "Off".into(), true),
            ],
            |this, tail, cx| {
                this.settings.tail = tail;
                cx.notify();
            },
        );
        let row = |label: &'static str, control: gpui::AnyElement| {
            div()
                .flex()
                .items_center()
                .gap(px(12.))
                .child(
                    div()
                        .w(px(64.))
                        .flex_none()
                        .text_color(theme.text_secondary)
                        .child(label),
                )
                .child(control)
                .into_any_element()
        };
        vec![
            row("Format", format.into_any_element()),
            row("Quality", quality),
            row("Range", range.into_any_element()),
            row("Tail", tail.into_any_element()),
        ]
    }

    fn note(&self, theme: &Theme) -> gpui::AnyElement {
        let khz = f64::from(self.song.sample_rate) / 1000.;
        let text = if !self.can_export() {
            match self.settings.range {
                Range::Song => "The song has no clips yet.".to_owned(),
                Range::Loop => "The loop region is empty.".to_owned(),
            }
        } else {
            match self.settings.format {
                Format::Mp3(_) if self.song.sample_rate > 48_000 => {
                    format!("Stereo, {khz} kHz, saved as 48 kHz (MP3's highest).")
                }
                _ => format!("Stereo, {khz} kHz, as the song plays."),
            }
        };
        div()
            .text_size(px(11.))
            .text_color(theme.text_tertiary)
            .child(text)
            .into_any_element()
    }
}

fn button(
    theme: &Theme,
    id: &'static str,
    label: &'static str,
    primary: bool,
    enabled: bool,
) -> gpui::Stateful<gpui::Div> {
    let hover = if primary { theme.accent } else { theme.hover };
    div()
        .id(id)
        .debug_selector(move || id.into())
        .px(px(14.))
        .py(px(5.))
        .rounded(px(6.))
        .when(primary && enabled, |d| {
            d.bg(theme.accent).text_color(gpui::white())
        })
        .when(!primary || !enabled, |d| d.bg(theme.control))
        .when(!enabled, |d| d.text_color(theme.text_tertiary))
        .when(enabled && !primary, |d| d.hover(move |s| s.bg(hover)))
        .child(label)
}

fn file_label(path: &Path) -> String {
    path.file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
}

impl Render for ExportDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let title = |text: String| {
            div()
                .text_size(px(15.))
                .font_weight(FontWeight::SEMIBOLD)
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(text)
        };
        let footer = || div().flex().items_center().justify_end().gap(px(8.));
        let mut body: Vec<gpui::AnyElement> = Vec::new();
        match &self.phase {
            Phase::Choosing | Phase::Asking => {
                body.push(title(format!("Export “{}”", self.song.name)).into_any_element());
                body.extend(self.render_choices(&theme, cx));
                body.push(self.note(&theme));
                let can = self.can_export() && matches!(self.phase, Phase::Choosing);
                body.push(
                    footer()
                        .child(
                            button(&theme, "export-close", "Cancel", false, true)
                                .on_click(cx.listener(|this, _, _, cx| this.close(cx))),
                        )
                        .child(
                            button(&theme, "export-start", "Export…", true, can)
                                .on_click(cx.listener(|this, _, _, cx| this.ask_path(cx))),
                        )
                        .into_any_element(),
                );
            }
            Phase::Running { path, progress, .. } => {
                let fraction = progress.fraction();
                body.push(title(format!("Exporting {}…", file_label(path))).into_any_element());
                body.push(
                    div()
                        .debug_selector(|| "export-progress".into())
                        .h(px(6.))
                        .w_full()
                        .rounded(px(3.))
                        .bg(theme.control)
                        .overflow_hidden()
                        .child(
                            div()
                                .h_full()
                                .w(relative(fraction))
                                .rounded(px(3.))
                                .bg(theme.accent),
                        )
                        .into_any_element(),
                );
                body.push(
                    div()
                        .text_size(px(11.))
                        .text_color(theme.text_tertiary)
                        .child(format!(
                            "{:.0}% · the song plays on while it renders.",
                            fraction * 100.
                        ))
                        .into_any_element(),
                );
                body.push(
                    footer()
                        .child(
                            button(&theme, "export-cancel", "Cancel", false, true).on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.cancel();
                                    cx.notify();
                                }),
                            ),
                        )
                        .into_any_element(),
                );
            }
            Phase::Done { path, exported } => {
                body.push(
                    title(format!("Exported {}", file_label(path)))
                        .debug_selector(|| "export-done".into())
                        .into_any_element(),
                );
                let peak_db = 20.0 * f64::from(exported.peak.max(1e-9)).log10();
                let clipped = exported.peak >= 1.0;
                body.push(
                    div()
                        .text_color(if clipped {
                            theme.warning
                        } else {
                            theme.text_secondary
                        })
                        .child(format!(
                            "{} · peak {:.1} dBFS{}",
                            duration(exported.seconds),
                            peak_db,
                            if clipped { " · clipped" } else { "" }
                        ))
                        .into_any_element(),
                );
                let folder = path.clone();
                body.push(
                    footer()
                        .child(
                            button(&theme, "export-reveal", "Show in Folder", false, true)
                                .on_click(cx.listener(move |_, _, _, cx| cx.reveal_path(&folder))),
                        )
                        .child(
                            button(&theme, "export-finish", "Done", true, true)
                                .on_click(cx.listener(|this, _, _, cx| this.close(cx))),
                        )
                        .into_any_element(),
                );
            }
            Phase::Failed(why) => {
                body.push(title("Export failed".into()).into_any_element());
                body.push(
                    div()
                        .debug_selector(|| "export-failed".into())
                        .text_color(theme.record)
                        .child(why.clone())
                        .into_any_element(),
                );
                body.push(
                    footer()
                        .child(button(&theme, "export-back", "Back", false, true).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.phase = Phase::Choosing;
                                cx.notify();
                            }),
                        ))
                        .child(
                            button(&theme, "export-finish", "Close", true, true)
                                .on_click(cx.listener(|this, _, _, cx| this.close(cx))),
                        )
                        .into_any_element(),
                );
            }
        }
        let panel = div()
            .id("export-panel")
            .debug_selector(|| "export-panel".into())
            .occlude()
            .w(px(WIDTH))
            .flex()
            .flex_col()
            .gap(px(12.))
            .p(px(18.))
            .rounded(px(10.))
            .bg(theme.group)
            .border_1()
            .border_color(theme.edge)
            .shadow_lg()
            .text_size(px(13.))
            .text_color(theme.text)
            .children(body);
        let running = self.is_running();
        // While it runs, no backdrop: the app around it stays usable.
        div()
            .id("export-scrim")
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .when(!running, |d| {
                d.track_focus(&self.focus)
                    .occlude()
                    .bg(theme.scrim)
                    .on_click(cx.listener(|this, _, _, cx| this.close(cx)))
                    .on_action(cx.listener(|this, _: &crate::project::CancelDrag, _, cx| {
                        this.close(cx);
                    }))
            })
            .child(panel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_read_as_minutes_and_seconds() {
        assert_eq!(duration(0.0), "0:00");
        assert_eq!(duration(4.4), "0:04");
        assert_eq!(duration(125.0), "2:05");
        assert_eq!(duration(3600.0), "60:00");
    }

    #[test]
    fn file_names_come_from_the_song() {
        let wav = Format::Wav(WavDepth::Int24);
        assert_eq!(file_name("My Song", wav), "My Song.wav");
        assert_eq!(
            file_name("a/b: c?", Format::Mp3(Mp3Bitrate::Kbps320)),
            "a b  c.mp3"
        );
        assert_eq!(file_name("  ", wav), "Export.wav");
        assert_eq!(file_name("Trailing.", wav), "Trailing.wav");
    }

    #[test]
    fn the_formats_extension_is_added_when_missing() {
        let wav = Format::Wav(WavDepth::Int16);
        assert_eq!(
            with_extension(PathBuf::from("out/mix"), wav),
            PathBuf::from("out/mix.wav")
        );
        assert_eq!(
            with_extension(PathBuf::from("out/mix.WAV"), wav),
            PathBuf::from("out/mix.WAV")
        );
        assert_eq!(
            with_extension(PathBuf::from("out/mix.v2"), wav),
            PathBuf::from("out/mix.v2.wav")
        );
    }
}
