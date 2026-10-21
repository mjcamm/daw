//! Preferences → Plugins: the folders searched, scanning (in the background
//! from startup, only new or changed plugins), and the plugins that could
//! not load (`docs/design/vst3-hosting.md`, Scanning).

use crate::app::AppView;
use crate::icons::{Icon, icon};
use crate::inspector::{group, section_label};
use crate::theme::Theme;
use crate::tooltip;
use daw_plugins::scan::{Catalog, Progress};
use gpui::{AnyElement, Context, ElementId, FontWeight, PathPromptOptions, div, prelude::*, px};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

/// The catalog and the scan that keeps it current.
#[derive(Debug, Default)]
pub struct PluginScan {
    /// The last scan's result (`None` until the first one finishes).
    pub catalog: Option<Catalog>,
    pub progress: Arc<Progress>,
    pub scanning: bool,
    /// Why the last scan could not run at all.
    pub error: Option<String>,
}

/// The inspector's section heading and grouped panel, flush with the page.
fn label(text: &'static str, theme: &Theme) -> gpui::Div {
    section_label(text, theme).px(px(0.))
}

fn panel(theme: &Theme) -> gpui::Div {
    group(theme).mx(px(0.))
}

/// How often an open Plugins page shows the scan's progress.
const PROGRESS_EVERY: Duration = Duration::from_millis(200);

impl AppView {
    /// Brings the catalog up to date in the background: standard folders
    /// and the user's, scanning only what is new or changed.
    pub(crate) fn start_scan(&mut self, cx: &mut Context<Self>) {
        if self.scan.scanning {
            return;
        }
        self.scan.scanning = true;
        self.scan.progress = Arc::default();
        let extra = self.preferences.preferences.plugin_folders.clone();
        let progress = self.scan.progress.clone();
        let task = cx
            .background_executor()
            .spawn(async move { daw_host::plugins::update_catalog(&extra, &progress) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.scan.scanning = false;
                match result {
                    Ok(catalog) => {
                        this.scan.catalog = Some(catalog);
                        this.scan.error = None;
                    }
                    Err(e) => this.scan.error = Some(e),
                }
                cx.notify();
            });
        })
        .detach();
        // While it runs, an open Plugins page shows how far it has got.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(PROGRESS_EVERY).await;
                let scanning = this
                    .update(cx, |this, cx| {
                        if this.showing_plugins() {
                            cx.notify();
                        }
                        this.scan.scanning
                    })
                    .unwrap_or(false);
                if !scanning {
                    break;
                }
            }
        })
        .detach();
    }

    fn add_plugin_folder(&mut self, cx: &mut Context<Self>) {
        let chosen = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Add plugin folder".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = chosen.await
                && let Some(folder) = paths.into_iter().next()
            {
                let _ = this.update(cx, |this, cx| {
                    let folders = &mut this.preferences.preferences.plugin_folders;
                    if !folders.contains(&folder) {
                        folders.push(folder);
                        this.save_preferences();
                        this.start_scan(cx);
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn remove_plugin_folder(&mut self, folder: &Path, cx: &mut Context<Self>) {
        self.preferences
            .preferences
            .plugin_folders
            .retain(|f| f != folder);
        self.save_preferences();
        self.start_scan(cx);
        cx.notify();
    }

    fn retry_plugin(&mut self, bundle: &Path, cx: &mut Context<Self>) {
        if let Err(e) = daw_host::plugins::forget_bundle(bundle) {
            eprintln!("could not update the plugin list: {e}");
        }
        self.start_scan(cx);
        cx.notify();
    }

    pub(crate) fn render_plugin_settings(
        &self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let hover = theme.hover;
        let button = |id: ElementId, label: &'static str| {
            div()
                .id(id)
                .px(px(12.))
                .py(px(4.))
                .rounded(px(6.))
                .bg(theme.control)
                .hover(move |s| s.bg(hover))
                .text_size(px(12.))
                .child(label)
        };

        // Folders: the standard ones, then the user's (removable).
        let row = |path: &Path, separated: bool| {
            div()
                .min_h(px(34.))
                .px(px(12.))
                .flex()
                .items_center()
                .gap(px(8.))
                .when(separated, |d| d.border_t_1().border_color(theme.hairline))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .text_size(px(12.))
                        .child(path.display().to_string()),
                )
        };
        let standard = daw_plugins::scan::standard_folders();
        let mut folders: Vec<AnyElement> = standard
            .iter()
            .enumerate()
            .map(|(i, path)| {
                row(path, i > 0)
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(theme.text_tertiary)
                            .child("Standard"),
                    )
                    .into_any_element()
            })
            .collect();
        for (i, path) in self
            .preferences
            .preferences
            .plugin_folders
            .iter()
            .enumerate()
        {
            let owned: PathBuf = path.clone();
            folders.push(
                row(path, true)
                    .child(
                        div()
                            .id(("plugin-folder-remove", i))
                            .p(px(3.))
                            .rounded(px(4.))
                            .hover(move |s| s.bg(hover))
                            .child(icon(Icon::Close, theme.text_secondary, 12.))
                            .tooltip(tooltip::text("Stop searching this folder"))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.remove_plugin_folder(&owned, cx);
                            })),
                    )
                    .into_any_element(),
            );
        }
        folders.push(
            div()
                .id("plugin-folder-add")
                .debug_selector(|| "plugin-folder-add".into())
                .min_h(px(34.))
                .px(px(12.))
                .flex()
                .items_center()
                .gap(px(6.))
                .border_t_1()
                .border_color(theme.hairline)
                .text_size(px(12.))
                .text_color(theme.accent)
                .hover(move |s| s.bg(hover))
                .child(icon(Icon::Plus, theme.accent, 12.))
                .child("Add Folder")
                .on_click(cx.listener(|this, _, _, cx| this.add_plugin_folder(cx)))
                .into_any_element(),
        );

        // Status: how many were found, or how far a scan has got.
        let found = self.scan.catalog.as_ref().map_or(0, |c| c.plugins().len());
        let status = if self.scan.scanning {
            let done = self.scan.progress.done.load(Ordering::Relaxed);
            let total = self.scan.progress.total.load(Ordering::Relaxed);
            if total == 0 {
                "Looking for plugins…".to_owned()
            } else {
                format!("Scanning {} of {total}…", (done + 1).min(total))
            }
        } else if let Some(error) = &self.scan.error {
            format!("Could not scan: {error}")
        } else {
            format!("{found} plugin{}", if found == 1 { "" } else { "s" })
        };
        let rescan = button("plugin-rescan".into(), "Rescan")
            .debug_selector(|| "plugin-rescan".into())
            .when(self.scan.scanning, |d| d.opacity(0.5))
            .tooltip(tooltip::text("Look for new or changed plugins"))
            .on_click(cx.listener(|this, _, _, cx| {
                this.start_scan(cx);
                cx.notify();
            }));

        // Plugins that could not load, with why.
        let failures: Vec<(PathBuf, String)> = self
            .scan
            .catalog
            .as_ref()
            .map(|c| {
                c.failures()
                    .into_iter()
                    .map(|(p, why)| (p.to_path_buf(), why.to_owned()))
                    .collect()
            })
            .unwrap_or_default();
        let failed = (!failures.is_empty()).then(|| {
            let rows = failures.into_iter().enumerate().map(|(i, (bundle, why))| {
                let name = bundle.file_stem().map_or_else(
                    || bundle.display().to_string(),
                    |s| s.to_string_lossy().into_owned(),
                );
                div()
                    .min_h(px(40.))
                    .px(px(12.))
                    .py(px(4.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .when(i > 0, |d| d.border_t_1().border_color(theme.hairline))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(name),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(theme.text_secondary)
                                    .child(why),
                            ),
                    )
                    .child(
                        button(("plugin-retry", i).into(), "Retry").on_click(cx.listener(
                            move |this, _, _, cx| {
                                this.retry_plugin(&bundle, cx);
                            },
                        )),
                    )
            });
            div()
                .flex()
                .flex_col()
                .child(label("COULDN'T LOAD", theme))
                .child(panel(theme).children(rows))
        });

        div()
            .id("plugin-settings")
            .size_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .child(
                div()
                    .text_size(px(15.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Plugins"),
            )
            .child(label("FOLDERS", theme))
            .child(panel(theme).children(folders))
            .child(
                div()
                    .mt(px(12.))
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .child(rescan)
                    .child(
                        div()
                            .debug_selector(|| "plugin-status".into())
                            .text_size(px(12.))
                            .text_color(theme.text_secondary)
                            .child(status),
                    ),
            )
            .children(failed)
    }
}
