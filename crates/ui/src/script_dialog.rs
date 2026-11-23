//! A script's dialog (plan §9.5): what `iskra.dialog` asked for, drawn with the app's own
//! widgets over the window, which waits for it. Its answer is added to the run's and the script
//! run again from its start; a cancelled one is answered null, for the script to go on knowing.

use egui::{Color32, Key, Modifiers, vec2};
use iskra_engine::ScriptRequest;
use serde_json::{Map, Value};

/// One of the options of a field that picks: what the script is given, what it is called, and
/// a swatch's colour.
struct Choice {
    value: Value,
    label: String,
    color: Option<Color32>,
}

/// A field of the dialog, with what it holds now.
struct Field {
    name: String,
    label: String,
    kind: String,
    value: Value,
    /// A number's bounds and step.
    range: (Option<f64>, Option<f64>, Option<f64>),
    options: Vec<Choice>,
}

/// What the dialog came to.
pub enum Answer {
    /// The values of its fields, by name.
    Given(Value),
    Cancelled,
}

/// A dialog a script waits on, with the run it is for.
pub struct Asking {
    pub request: ScriptRequest,
    title: String,
    ok: String,
    fields: Vec<Field>,
}

impl Asking {
    /// The dialog `asked` as the outcome of the run of `request` gives it.
    pub fn new(request: ScriptRequest, asked: &Value) -> Self {
        let text = |v: &Value, or: &str| v.as_str().unwrap_or(or).to_string();
        let colour = |v: &Value| {
            let c: Vec<u8> = v.as_array()?.iter().map(|c| c.as_u64().map(|c| c.min(255) as u8)).collect::<Option<_>>()?;
            Some(Color32::from_rgba_unmultiplied(*c.first()?, *c.get(1)?, *c.get(2)?, c.get(3).copied().unwrap_or(255)))
        };
        let fields = asked["fields"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|f| Field {
                name: text(&f["name"], ""),
                label: text(&f["label"], ""),
                kind: text(&f["kind"], "text"),
                value: f["value"].clone(),
                range: (f["min"].as_f64(), f["max"].as_f64(), f["step"].as_f64()),
                options: f["options"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|o| Choice { value: o["value"].clone(), label: text(&o["label"], ""), color: colour(&o["color"]) })
                    .collect(),
            })
            .collect();
        Asking { title: text(&asked["title"], "Script"), ok: text(&asked["ok"], "OK"), fields, request }
    }

    /// The values of the fields, by name, as the dialog holds them.
    pub fn values(&self) -> Value {
        Value::Object(self.fields.iter().map(|field| (field.name.clone(), field.value.clone())).collect::<Map<_, _>>())
    }

    /// Set the field `name` to `value`, as typing in it would.
    #[cfg(test)]
    pub fn set(&mut self, name: &str, value: Value) {
        if let Some(field) = self.fields.iter_mut().find(|field| field.name == name) {
            field.value = value;
        }
    }

    /// Draw the dialog over the window. Gives what it came to once it is answered: Enter or its
    /// button for the values, Escape or Cancel for none.
    pub fn show(&mut self, ctx: &egui::Context) -> Option<Answer> {
        let mut answer = None;
        let modal = egui::Modal::new(egui::Id::new("script dialog")).show(ctx, |ui| {
            ui.set_min_width(300.0);
            ui.heading(&self.title);
            ui.add_space(4.0);
            egui::Grid::new("script dialog fields").num_columns(2).spacing(vec2(10.0, 6.0)).show(ui, |ui| {
                for field in &mut self.fields {
                    ui.label(&field.label);
                    field_widget(ui, field);
                    ui.end_row();
                }
            });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button(&self.ok).clicked() {
                    answer = Some(Answer::Given(Value::Null));
                }
                if ui.button("Cancel").clicked() {
                    answer = Some(Answer::Cancelled);
                }
            });
        });
        let (entered, escaped) = ctx.input_mut(|i| (i.consume_key(Modifiers::NONE, Key::Enter), i.consume_key(Modifiers::NONE, Key::Escape)));
        if escaped || modal.should_close() && answer.is_none() {
            answer = Some(Answer::Cancelled);
        }
        if entered && answer.is_none() {
            answer = Some(Answer::Given(Value::Null));
        }
        match answer {
            Some(Answer::Given(_)) => Some(Answer::Given(self.values())),
            other => other,
        }
    }
}

/// The widget of a field, by its kind: a picking one is a list of its options.
fn field_widget(ui: &mut egui::Ui, field: &mut Field) {
    match field.kind.as_str() {
        "text" => {
            let mut text = field.value.as_str().unwrap_or_default().to_string();
            if ui.add(egui::TextEdit::singleline(&mut text).desired_width(200.0)).changed() {
                field.value = Value::String(text);
            }
        }
        "number" => {
            let (min, max, step) = field.range;
            let mut number = field.value.as_f64().unwrap_or_default();
            let whole = step.is_none_or(|step| step.fract() == 0.0) && number.fract() == 0.0;
            let mut drag = egui::DragValue::new(&mut number).speed(step.unwrap_or(if whole { 0.2 } else { 0.01 }));
            drag = drag.range(min.unwrap_or(f64::NEG_INFINITY)..=max.unwrap_or(f64::INFINITY));
            if whole {
                drag = drag.fixed_decimals(0);
            }
            if ui.add(drag).changed() {
                field.value = if whole { Value::from(number.round() as i64) } else { Value::from(number) };
            }
        }
        "check" => {
            let mut on = field.value.as_bool().unwrap_or_default();
            if ui.checkbox(&mut on, "").changed() {
                field.value = Value::Bool(on);
            }
        }
        _ if field.options.is_empty() => {
            ui.weak("None to choose from");
        }
        _ => {
            let picked = field.options.iter().find(|o| o.value == field.value).map_or("", |o| o.label.as_str()).to_string();
            egui::ComboBox::from_id_salt(("script dialog", &field.name)).selected_text(picked).width(200.0).show_ui(ui, |ui| {
                for option in &field.options {
                    ui.horizontal(|ui| {
                        if let Some(color) = option.color {
                            let (chip, _) = ui.allocate_exact_size(vec2(14.0, 14.0), egui::Sense::hover());
                            ui.painter().rect_filled(chip, 2.0, color);
                        }
                        if ui.selectable_label(option.value == field.value, &option.label).clicked() {
                            field.value = option.value.clone();
                        }
                    });
                }
            });
        }
    }
}
