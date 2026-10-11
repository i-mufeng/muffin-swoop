use egui::{Frame, Margin, RichText};
use swoop_core::{FileConflictPolicy, TaskId, TaskStatus};

use crate::app::SwoopApp;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConflictTarget {
    File(TaskId),
    Media(u64),
}
impl ConflictTarget {
    fn key(self) -> (u64, u8) {
        match self {
            Self::File(id) => (id.0, 0),
            Self::Media(id) => (id, 1),
        }
    }
}

pub struct ConflictPrompt {
    pub target: ConflictTarget,
    remember: bool,
    error: Option<String>,
}

fn pending_id(current: Option<ConflictTarget>, ids: impl Iterator<Item = ConflictTarget>) -> Option<ConflictTarget> {
    let mut first = None;
    for id in ids {
        if Some(id) == current {
            return Some(id);
        }
        first = Some(first.map_or(id, |first: ConflictTarget| if first.key() < id.key() { first } else { id }));
    }
    first
}

impl SwoopApp {
    pub(crate) fn poll_file_conflicts(&mut self, ctx: &egui::Context) {
        if self.is_quitting() {
            return;
        }
        // Reconcile snapshots as well as events: restart or a lagged event must not hide a conflict.
        let current = self.file_conflict.as_ref().map(|prompt| prompt.target);
        let files = self
            .engine
            .snapshot()
            .into_iter()
            .filter(|task| task.status == TaskStatus::FileConflict)
            .map(|task| ConflictTarget::File(task.id));
        let media = self
            .media_tasks
            .jobs
            .iter()
            .filter(|job| job.status == crate::media_tasks::MediaStatus::FileConflict)
            .map(|job| ConflictTarget::Media(job.id));
        let next = pending_id(current, files.chain(media));
        if current != next {
            self.file_conflict = next.map(|target| ConflictPrompt { target, remember: false, error: None });
            if next.is_some() {
                self.completion.countdown.cancel();
                self.show_window(ctx);
            }
        }
    }

    pub fn file_conflict_ui(&mut self, ctx: &egui::Context) {
        if self.is_quitting() {
            return;
        }
        let Some(prompt) = &mut self.file_conflict else {
            return;
        };
        let path = match prompt.target {
            ConflictTarget::File(id) => self
                .engine
                .snapshot_one(id)
                .filter(|task| task.status == TaskStatus::FileConflict)
                .map(|task| task.path),
            ConflictTarget::Media(id) => self
                .media_tasks
                .jobs
                .iter()
                .find(|job| job.id == id && job.status == crate::media_tasks::MediaStatus::FileConflict)
                .map(|job| job.destination.clone()),
        };
        let Some(path) = path else {
            self.file_conflict = None;
            return;
        };
        let p = self.palette;
        let mut choice = None;
        let modal = egui::Modal::new(egui::Id::new(("file_conflict", prompt.target.key())))
            .frame(Frame::window(&ctx.style_of(ctx.theme())).fill(p.surface).inner_margin(Margin::same(22)))
            .show(ctx, |ui| {
                ui.set_width(440.0);
                ui.horizontal(|ui| {
                    ui.label(super::icon_text(egui_phosphor::regular::FILES, 22.0, p.tint_amber));
                    ui.label(RichText::new(swoop_i18n::tr!("已存在同名文件")).size(18.0).strong().color(p.text));
                });
                ui.add_space(14.0);
                ui.label(RichText::new(path.file_name().unwrap_or_default().to_string_lossy()).strong().color(p.text));
                if let Some(directory) = path.parent() {
                    ui.label(RichText::new(directory.to_string_lossy()).size(12.0).color(p.text_muted));
                }
                ui.add_space(14.0);
                ui.checkbox(&mut prompt.remember, swoop_i18n::tr!("记住选择"));
                if let Some(error) = &prompt.error {
                    ui.colored_label(p.danger, error);
                }
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    if ui.button(RichText::new(swoop_i18n::tr!("覆盖")).color(p.danger)).clicked() {
                        choice = Some(FileConflictPolicy::Overwrite);
                    }
                    if super::primary_button(ui, &p, swoop_i18n::tr!("重命名")).clicked() {
                        choice = Some(FileConflictPolicy::Rename);
                    }
                    if ui.button(swoop_i18n::tr!("取消")).clicked() {
                        choice = Some(FileConflictPolicy::Skip);
                    }
                });
            });
        let remember = choice.is_some() && prompt.remember;
        if choice.is_none() && modal.should_close() {
            choice = Some(FileConflictPolicy::Skip);
        }
        if let Some(policy) = choice {
            // Persist a remembered choice before resolving. A failed settings write must not start a download.
            if remember && let Err(error) = self.engine.set_file_conflict_policy(policy) {
                prompt.error = Some(swoop_i18n::trf!("保存选择失败：{error}"));
                return;
            }
            let result = match prompt.target {
                ConflictTarget::File(id) => {
                    self.engine.resolve_file_conflict(id, policy).map_err(|error| error.to_string())
                }
                ConflictTarget::Media(id) => {
                    self.media_tasks.update_file_policy(&self.engine);
                    self.media_tasks.resolve_file_conflict(id, policy, ctx)
                }
            };
            match result {
                Ok(()) => self.file_conflict = None,
                Err(error) => prompt.error = Some(error.to_string()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retains_current_prompt_then_chooses_stable_next_id() {
        use ConflictTarget::{File, Media};
        assert_eq!(pending_id(None, [File(TaskId(8)), File(TaskId(3)), Media(2)].into_iter()), Some(Media(2)));
        assert_eq!(pending_id(Some(Media(5)), [File(TaskId(3)), Media(5)].into_iter()), Some(Media(5)));
        assert_eq!(pending_id(Some(Media(5)), [Media(8), File(TaskId(3))].into_iter()), Some(File(TaskId(3))));
        assert_eq!(pending_id(None, [Media(3), File(TaskId(3))].into_iter()), Some(File(TaskId(3))));
        assert_eq!(pending_id(Some(Media(5)), [].into_iter()), None);
    }
}
