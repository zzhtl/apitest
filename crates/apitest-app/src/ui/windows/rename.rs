use eframe::egui;

use crate::app::ApiTestApp;

impl ApiTestApp {
    /// Inline rename for a resource-tree node.
    pub(crate) fn rename_window(&mut self, context: &egui::Context) {
        let Some((node, mut name)) = self.rename_target.clone() else {
            return;
        };
        let mut confirm = false;
        let mut cancel = false;
        egui::Window::new(self.tr("重命名", "Rename"))
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .resizable(false)
            .collapsible(false)
            .show(context, |ui| {
                ui.set_min_width(320.0);
                let field = ui.add_sized(
                    [ui.available_width(), 32.0],
                    egui::TextEdit::singleline(&mut name),
                );
                field.request_focus();
                if field.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) {
                    confirm = true;
                }
                ui.add_space(10.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_enabled(
                            !name.trim().is_empty(),
                            egui::Button::new(self.tr("确定", "Rename")),
                        )
                        .clicked()
                    {
                        confirm = true;
                    }
                    if ui.button(self.tr("取消", "Cancel")).clicked() {
                        cancel = true;
                    }
                });
                if ui.input(|input| input.key_pressed(egui::Key::Escape)) {
                    cancel = true;
                }
            });
        if confirm {
            self.rename_resource(node, &name);
            self.rename_target = None;
        } else if cancel {
            self.rename_target = None;
        } else {
            self.rename_target = Some((node, name));
        }
    }
}
