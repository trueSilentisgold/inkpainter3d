//! Undo and redo (Ctrl+Z, Ctrl+Y) as commands over the model's data. A command that is
//! done keeps nothing the model already has; an undone one holds what it took out.

use crate::layer::LayerStack;
use crate::model::Model;
use crate::stroke::Stroke;
use std::sync::Arc;

/// An undoable change to the model.
#[derive(Clone, Debug)]
pub enum Command {
    /// A stroke was added on top of a paint layer (of a layer, a mask or a paint effect).
    /// Holds the stroke while undone.
    AddStroke {
        set: usize,
        paint: u64,
        id: u64,
        stroke: Option<Arc<Stroke>>,
    },
    /// Layers of a texture set were added, removed, moved or changed. Holds the stack as it
    /// was before while done, and as it was after while undone: undo and redo swap it in.
    /// Strokes are shared between both versions, so this is cheap.
    EditStack {
        set: usize,
        label: String,
        stack: LayerStack,
    },
}

impl Command {
    pub fn add_stroke(set: usize, paint: u64, id: u64) -> Self {
        Self::AddStroke {
            set,
            paint,
            id,
            stroke: None,
        }
    }

    /// `before` is the texture set's layer stack before the change.
    pub fn edit_stack(set: usize, label: impl Into<String>, before: LayerStack) -> Self {
        Self::EditStack {
            set,
            label: label.into(),
            stack: before,
        }
    }

    pub fn label(&self) -> &str {
        match self {
            Self::AddStroke { .. } => "Stroke",
            Self::EditStack { label, .. } => label,
        }
    }

    /// Takes the change back. Returns false if the model no longer matches the command.
    fn undo(&mut self, model: &mut Model) -> bool {
        match self {
            Self::AddStroke {
                set,
                paint,
                id,
                stroke,
            } => {
                let Some(strokes) = model
                    .texture_sets
                    .get_mut(*set)
                    .and_then(|s| s.stack.paint_mut(*paint))
                    .map(|p| &mut p.strokes)
                else {
                    return false;
                };
                if strokes.last().is_none_or(|s| s.id != *id) {
                    return false;
                }
                *stroke = strokes.pop();
                true
            }
            Self::EditStack { .. } => self.swap_stack(model),
        }
    }

    /// Applies the change again after it was undone.
    fn redo(&mut self, model: &mut Model) -> bool {
        match self {
            Self::AddStroke {
                set, paint, stroke, ..
            } => match (
                model
                    .texture_sets
                    .get_mut(*set)
                    .and_then(|s| s.stack.paint_mut(*paint)),
                stroke.take(),
            ) {
                (Some(layer), Some(s)) => {
                    layer.strokes.push(s);
                    true
                }
                (_, s) => {
                    *stroke = s;
                    false
                }
            },
            Self::EditStack { .. } => self.swap_stack(model),
        }
    }

    fn swap_stack(&mut self, model: &mut Model) -> bool {
        let Self::EditStack { set, stack, .. } = self else {
            return false;
        };
        match model.texture_sets.get_mut(*set) {
            Some(texture_set) => {
                std::mem::swap(&mut texture_set.stack, stack);
                true
            }
            None => false,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct History {
    /// With the serial number each command got when it was recorded.
    done: Vec<(u64, Command)>,
    undone: Vec<(u64, Command)>,
    /// The last serial number handed out.
    serial: u64,
    /// The state before the oldest command still done.
    base: u64,
}

impl History {
    /// Records a change that was just made to the model. Clears the redo list.
    pub fn record(&mut self, command: Command) {
        self.serial += 1;
        self.done.push((self.serial, command));
        self.undone.clear();
    }

    pub fn clear(&mut self) {
        self.done.clear();
        self.undone.clear();
        self.serial += 1;
        self.base = self.serial;
    }

    /// Identifies the model's state as far as the history knows it: changes when a command
    /// is recorded and when one is undone or redone, and comes back when undo or redo
    /// return to an earlier state. Compare it with the state at saving to tell unsaved
    /// changes.
    pub fn state(&self) -> u64 {
        self.done.last().map_or(self.base, |(serial, _)| *serial)
    }

    pub fn can_undo(&self) -> bool {
        !self.done.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.undone.is_empty()
    }

    /// Undoes the last change. Returns its label, or None if there was nothing to undo.
    pub fn undo(&mut self, model: &mut Model) -> Option<String> {
        let (serial, mut command) = self.done.pop()?;
        if !command.undo(model) {
            // The model changed behind the history's back; drop what can't be undone.
            self.clear();
            return None;
        }
        let label = command.label().to_owned();
        self.undone.push((serial, command));
        Some(label)
    }

    /// Redoes the last undone change.
    pub fn redo(&mut self, model: &mut Model) -> Option<String> {
        let (serial, mut command) = self.undone.pop()?;
        if !command.redo(model) {
            self.clear();
            return None;
        }
        let label = command.label().to_owned();
        self.done.push((serial, command));
        Some(label)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brush::{Alpha, BrushMaterial, Tool};
    use crate::layer::{Content, Layer, LayerKind};
    use crate::mesh::Mesh;
    use crate::stroke::StrokeSpace;

    fn stroke(model: &mut Model) -> Stroke {
        Stroke {
            id: model.next_id(),
            tool: Tool::Paint,
            space: StrokeSpace::Surface,
            alpha: Alpha::Round,
            hardness: 1.0,
            alignment: Default::default(),
            fill: None,
            stencil: None,
            opacity: 1.0,
            backface_cos: 0.0,
            material: BrushMaterial::default(),
            camera: None,
            hidden_objects: Vec::new(),
            hidden_sets: Vec::new(),
            symmetry: Vec::new(),
            dabs: Vec::new(),
        }
    }

    fn paint_id(model: &Model) -> u64 {
        match &model.texture_sets[0].stack.layers[0].content {
            Content::Paint(p) => p.id,
            _ => unreachable!(),
        }
    }

    fn ids(model: &Model) -> Vec<u64> {
        let paint = paint_id(model);
        model.texture_sets[0]
            .stack
            .paint(paint)
            .unwrap()
            .strokes
            .iter()
            .map(|s| s.id)
            .collect()
    }

    fn add(model: &mut Model, history: &mut History) -> u64 {
        let s = stroke(model);
        let id = s.id;
        let paint = paint_id(model);
        history.record(Command::add_stroke(0, paint, id));
        model.texture_sets[0]
            .stack
            .paint_mut(paint)
            .unwrap()
            .strokes
            .push(Arc::new(s));
        id
    }

    #[test]
    fn undo_and_redo_strokes() {
        let mut model = Model::from_mesh(Mesh::cube(1.0));
        let mut history = History::default();
        let added: Vec<u64> = (0..3).map(|_| add(&mut model, &mut history)).collect();
        assert_eq!(ids(&model), added);

        assert_eq!(history.undo(&mut model).as_deref(), Some("Stroke"));
        assert_eq!(history.undo(&mut model).as_deref(), Some("Stroke"));
        assert_eq!(ids(&model), added[..1]);
        assert_eq!(history.redo(&mut model).as_deref(), Some("Stroke"));
        assert_eq!(ids(&model), added[..2]);

        // A new stroke drops the redo list.
        let fourth = add(&mut model, &mut history);
        assert!(!history.can_redo());
        assert_eq!(ids(&model), [added[0], added[1], fourth]);

        while history.undo(&mut model).is_some() {}
        assert!(ids(&model).is_empty());
        assert!(history.can_redo() && !history.can_undo());
    }

    #[test]
    fn states_tell_unsaved_changes() {
        let mut model = Model::from_mesh(Mesh::cube(1.0));
        let mut history = History::default();
        let empty = history.state();
        add(&mut model, &mut history);
        let saved = history.state();
        assert_ne!(saved, empty);
        add(&mut model, &mut history);
        assert_ne!(history.state(), saved);
        history.undo(&mut model);
        assert_eq!(history.state(), saved, "undo returns to the saved state");
        history.undo(&mut model);
        assert_eq!(history.state(), empty);
        history.redo(&mut model);
        assert_eq!(history.state(), saved);
        // A different change after undo is a different state.
        history.undo(&mut model);
        add(&mut model, &mut history);
        assert_ne!(history.state(), saved);
        assert_ne!(history.state(), empty);
        // So is clearing the history.
        history.clear();
        assert_ne!(history.state(), empty);
    }

    #[test]
    fn stack_edits_swap_whole_stacks() {
        let mut model = Model::from_mesh(Mesh::cube(1.0));
        let mut history = History::default();
        add(&mut model, &mut history);
        let before = model.texture_sets[0].stack.clone();
        let layer = Layer::new(LayerKind::Fill, "Fill layer 1".into(), &mut model.ids);
        model.texture_sets[0].stack.layers.push(layer);
        history.record(Command::edit_stack(0, "Add fill layer", before));
        assert_eq!(model.texture_sets[0].stack.layers.len(), 2);

        assert_eq!(history.undo(&mut model).as_deref(), Some("Add fill layer"));
        assert_eq!(model.texture_sets[0].stack.layers.len(), 1);
        // The stroke below the edit can still be undone and redone.
        assert_eq!(history.undo(&mut model).as_deref(), Some("Stroke"));
        assert!(ids(&model).is_empty());
        assert_eq!(history.redo(&mut model).as_deref(), Some("Stroke"));
        assert_eq!(history.redo(&mut model).as_deref(), Some("Add fill layer"));
        assert_eq!(model.texture_sets[0].stack.layers.len(), 2);
        assert_eq!(ids(&model).len(), 1);
    }
}
