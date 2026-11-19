use egui::{Grid, Slider, Ui};

use crate::{
    controller::{components::UIComponents, icons, settings::Settings},
};

pub struct ControlsWidget {}

impl Default for ControlsWidget {
    fn default() -> Self {
        Self::new()
    }
}

impl ControlsWidget {
    pub fn new() -> Self {
        Self {}
    }

    pub fn show(
        &self,
        ui: &mut Ui,
        settings: &mut Settings,
        erase_target_name: &str,
    ) {
        ui.checkbox(&mut settings.erase_brush, "Erase Brush")
            .on_hover_text("Enable the erase brush to remove tissue in the 3D view. (Ctrl + Caps Lock to toggle)");

        ui.add_enabled(
            settings.erase_brush,
            egui::Label::new(format!("Erase target: {erase_target_name}")),
        )
        .on_hover_text(
            "The brush edits the mask assigned to the active volume in the Volumes tab.",
        );
        // TODO(BRUSH-RADIUS-001): Currently this is a prototype pixel-based
        // slider. Convert it to 1-20 mm, using NIfTI voxel spacing and/or the
        // volume transform in the final resection-mask implementation.
        ui.add_enabled(
            settings.erase_brush,
            Slider::new(&mut settings.brush_radius, 0.01..=100.0)
                .text("Brush radius")
                .suffix(" px"),
        );
        ui.add_enabled(
            settings.erase_brush,
            egui::Checkbox::new(
                &mut settings.show_brush_center_dot,
                "Show center dot",
            ),
        );
        let camera_actions = [
            ("Rotate", "Left Mouse Button"),
            ("Pan", "Right Mouse Button"),
            ("Zoom", "Mouse Wheel / Middle Mouse Button"),
            ("Reset", "Backspace"),
        ];

        let light_actions = [("Rotate", "Shift + Left Mouse Button")];

        let camera_open = ui.is_new("Controls.Camera");
        ui.collapse(
            format!("{} Camera", icons::regular::VIDEO_CAMERA),
            camera_open,
            |ui| Self::actions_grid(ui, "ControlsCameraGrid", &camera_actions),
        );

        let light_open = ui.is_new("Controls.Light");
        ui.collapse(format!("{} Light", icons::regular::SUN), light_open, |ui| {
            Self::actions_grid(ui, "ControlsLightGrid", &light_actions)
        });
    }

    fn actions_grid(ui: &mut Ui, id: &str, actions: &[(&str, &str)]) {
        Grid::new(id)
            .num_columns(2)
            .spacing([40.0, ui.spacing().item_spacing.y])
            .show(ui, |ui| {
                for (action, control) in actions {
                    ui.label(*action);
                    ui.label(*control);
                    ui.end_row();
                }
            });
    }
}
