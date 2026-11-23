//! The rows of a panel of settings, the Export page's and Properties: each setting's name on
//! the right of a column of names, and what is set in it after that, so that the names and
//! the fields of the whole panel are in line.

/// How wide the column of the settings' names is.
pub(crate) const NAME_WIDTH: f32 = 96.0;
/// How wide a setting's list or text field is.
pub(crate) const FIELD_WIDTH: f32 = 160.0;

/// A setting's name, in its column: the first cell of a row, or of a grid's row.
pub(crate) fn name(ui: &mut egui::Ui, name: impl Into<egui::WidgetText>) -> egui::Response {
    let height = ui.spacing().interact_size.y;
    ui.allocate_ui_with_layout(egui::vec2(NAME_WIDTH, height), egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.set_min_size(egui::vec2(NAME_WIDTH, height));
        ui.label(name)
    })
    .inner
}

/// A row of settings: its name, and what is set in it after that.
pub(crate) fn row<R>(ui: &mut egui::Ui, title: impl Into<egui::WidgetText>, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.horizontal_top(|ui| {
        name(ui, title);
        add(ui)
    })
    .inner
}

/// Where the settings of the tool in hand are drawn: one after another in the tool options
/// bar, or in the Tool Properties panel as rows of a name and what is set, as the other panels
/// of settings have them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Place {
    Bar,
    Panel,
}

impl Place {
    /// A row: in the panel, the name in its column and what is set after it, wrapped to the
    /// panel's width; in the bar, what is set alone.
    pub(crate) fn row<R>(self, ui: &mut egui::Ui, title: impl Into<egui::WidgetText>, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
        match self {
            Place::Bar => add(ui),
            Place::Panel => row(ui, title, |ui| ui.horizontal_wrapped(add).inner),
        }
    }

    /// A line of text that says what to do or what is so: in the panel, under the fields.
    pub(crate) fn note(self, ui: &mut egui::Ui, text: impl Into<egui::WidgetText>) -> egui::Response {
        self.row(ui, "", |ui| ui.label(text))
    }

    /// What is written on a slider or a number in the bar, which the panel has in its
    /// column of names instead.
    pub(crate) fn on_bar(self, text: &str) -> &str {
        match self {
            Place::Bar => text,
            Place::Panel => "",
        }
    }
}
