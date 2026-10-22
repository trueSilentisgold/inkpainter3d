//! Help > Report a bug, and the question after a crash. Both show what the report holds
//! and open a prefilled GitHub issue in the browser; nothing is sent from the app.

use super::{InkApp, dialog};
use crate::report::{self, CrashReport};
use crate::theme;
use eframe::egui::{self, RichText};

/// The bug report dialog while it is open.
#[derive(Default)]
pub(super) struct BugReport {
    title: String,
    description: String,
    include_log: bool,
}

impl BugReport {
    pub(super) fn new() -> Self {
        Self {
            include_log: true,
            ..Default::default()
        }
    }
}

impl InkApp {
    pub(super) fn report_dialogs(&mut self, ui: &mut egui::Ui) {
        self.bug_report_dialog(ui);
        // After a crash, once nothing else asks a question.
        if !self.project.dialog_open() && self.bug_report.is_none() && !self.about_open {
            self.crash_report_dialog(ui);
        }
    }

    fn bug_report_dialog(&mut self, ui: &mut egui::Ui) {
        let Some(bug) = &mut self.bug_report else {
            return;
        };
        let mut close = false;
        let modal = dialog(ui, "bug report").show(ui.ctx(), |ui| {
            ui.set_width(520.0);
            ui.label(
                RichText::new("Report a bug")
                    .font(theme::medium(18.0))
                    .color(theme::TEXT_STRONG),
            );
            ui.label(
                RichText::new(
                    "This opens a new issue on GitHub with the details below filled in. \
                     You need a GitHub account; nothing is sent until you submit it there.",
                )
                .color(theme::TEXT_WEAK),
            );
            ui.add_space(6.0);
            ui.add(
                egui::TextEdit::singleline(&mut bug.title)
                    .hint_text("Short summary, e.g. \"Brush leaves gaps on UV seams\"")
                    .desired_width(f32::INFINITY),
            );
            ui.add(
                egui::TextEdit::multiline(&mut bug.description)
                    .hint_text(
                        "What happened, and what did you expect? \
                         The steps to make it happen again help most.",
                    )
                    .desired_rows(6)
                    .desired_width(f32::INFINITY),
            );
            ui.checkbox(&mut bug.include_log, "Include the last lines of the log");
            egui::CollapsingHeader::new("What the report holds")
                .id_salt("bug report contents")
                .show(ui, |ui| {
                    ui.label(
                        RichText::new(report::system_info()).font(egui::FontId::monospace(11.0)),
                    );
                    if bug.include_log {
                        ui.label(
                            RichText::new(format!(
                                "plus up to {} lines of the log",
                                report::LOG_LINES
                            ))
                            .small()
                            .color(theme::TEXT_WEAK),
                        );
                    }
                });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let title = bug.title.trim();
                let ready = !title.is_empty();
                if ui
                    .add_enabled(ready, egui::Button::new("Open on GitHub"))
                    .on_disabled_hover_text("Give the bug a short summary first")
                    .clicked()
                {
                    let url = report::bug_issue_url(title, bug.description.trim(), bug.include_log);
                    ui.ctx().open_url(egui::OpenUrl::new_tab(url));
                    close = true;
                }
                if ui
                    .add_enabled(ready, egui::Button::new("Copy report"))
                    .on_hover_text("For pasting it somewhere else, e.g. the Discord server")
                    .clicked()
                {
                    ui.ctx().copy_text(report::bug_report_text(
                        title,
                        bug.description.trim(),
                        bug.include_log,
                    ));
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        });
        if close || modal.should_close() {
            self.bug_report = None;
        }
    }

    fn crash_report_dialog(&mut self, ui: &mut egui::Ui) {
        let Some(crash) = &self.crash_report else {
            return;
        };
        let mut answered = false;
        let modal = dialog(ui, "crash report").show(ui.ctx(), |ui| {
            ui.set_width(600.0);
            ui.label(
                RichText::new("inkpainter3d ran into an error last time")
                    .font(theme::medium(18.0))
                    .color(theme::TEXT_STRONG),
            );
            ui.label(RichText::new(crash.message()).color(theme::WARN));
            ui.add_space(4.0);
            ui.label(
                "Reporting it helps fix it. The report holds the error, the app version, \
                 your system and GPU, the last lines of the log and the size of the project; \
                 no names, paths or images.",
            );
            ui.add_space(6.0);
            egui::ScrollArea::both()
                .max_height(240.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.label(RichText::new(&crash.text).font(egui::FontId::monospace(11.0)));
                });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui
                    .button("Report on GitHub")
                    .on_hover_text(
                        "Opens a prefilled issue in the browser (needs a GitHub account). \
                         The full report is also copied, in case a long one gets cut.",
                    )
                    .clicked()
                {
                    ui.ctx().copy_text(crash.text.clone());
                    ui.ctx().open_url(egui::OpenUrl::new_tab(crash.issue_url()));
                    answered = true;
                }
                if ui.button("Copy report").clicked() {
                    ui.ctx().copy_text(crash.text.clone());
                }
                if ui.button("Open folder").clicked()
                    && let Some(dir) = crash.path.parent()
                {
                    report::open_folder(dir);
                }
                if ui.button("Don't report").clicked() {
                    answered = true;
                }
            });
        });
        if answered || modal.should_close() {
            crash.mark_seen();
            self.crash_report = None;
        }
    }
}

/// The newest crash report nobody answered yet, read at start.
pub(super) fn pending_crash() -> Option<CrashReport> {
    let crash = CrashReport::pending()?;
    log::warn!(
        "Found a crash report from an earlier run: {}",
        crash.path.display()
    );
    Some(crash)
}
