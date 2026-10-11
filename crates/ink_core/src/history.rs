//! Undo and redo (Ctrl+Z, Ctrl+Y) as commands over the model's data. A command that is
//! done keeps nothing the model already has; an undone one holds what it took out.

use crate::model::Model;
use crate::stroke::Stroke;

/// An undoable change to the model.
#[derive(Clone, Debug)]
pub enum Command {
    /// A stroke was added on top of a texture set's paint layer. Holds the stroke while
    /// undone.
    AddStroke {
        set: usize,
        id: u64,
        stroke: Option<Box<Stroke>>,
    },
}

impl Command {
    pub fn add_stroke(set: usize, id: u64) -> Self {
        Self::AddStroke {
            set,
            id,
            stroke: None,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::AddStroke { .. } => "Stroke",
        }
    }

    /// Takes the change back. Returns false if the model no longer matches the command.
    fn undo(&mut self, model: &mut Model) -> bool {
        match self {
            Self::AddStroke { set, id, stroke } => {
                let Some(strokes) = model
                    .texture_sets
                    .get_mut(*set)
                    .map(|s| &mut s.paint.strokes)
                else {
                    return false;
                };
                if strokes.last().is_none_or(|s| s.id != *id) {
                    return false;
                }
                *stroke = strokes.pop().map(Box::new);
                true
            }
        }
    }

    /// Applies the change again after it was undone.
    fn redo(&mut self, model: &mut Model) -> bool {
        match self {
            Self::AddStroke { set, stroke, .. } => {
                match (model.texture_sets.get_mut(*set), stroke.take()) {
                    (Some(texture_set), Some(s)) => {
                        texture_set.paint.strokes.push(*s);
                        true
                    }
                    (_, s) => {
                        *stroke = s;
                        false
                    }
                }
            }
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct History {
    done: Vec<Command>,
    undone: Vec<Command>,
}

impl History {
    /// Records a change that was just made to the model. Clears the redo list.
    pub fn record(&mut self, command: Command) {
        self.done.push(command);
        self.undone.clear();
    }

    pub fn clear(&mut self) {
        self.done.clear();
        self.undone.clear();
    }

    pub fn can_undo(&self) -> bool {
        !self.done.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.undone.is_empty()
    }

    /// Undoes the last change. Returns its label, or None if there was nothing to undo.
    pub fn undo(&mut self, model: &mut Model) -> Option<&'static str> {
        let mut command = self.done.pop()?;
        if !command.undo(model) {
            // The model changed behind the history's back; drop what can't be undone.
            self.clear();
            return None;
        }
        let label = command.label();
        self.undone.push(command);
        Some(label)
    }

    /// Redoes the last undone change.
    pub fn redo(&mut self, model: &mut Model) -> Option<&'static str> {
        let mut command = self.undone.pop()?;
        if !command.redo(model) {
            self.clear();
            return None;
        }
        let label = command.label();
        self.done.push(command);
        Some(label)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brush::{Alpha, BrushMaterial, Tool};
    use crate::mesh::Mesh;
    use crate::stroke::StrokeSpace;

    fn stroke(model: &mut Model) -> Stroke {
        Stroke {
            id: model.next_stroke_id(),
            tool: Tool::Paint,
            space: StrokeSpace::Surface,
            alpha: Alpha::Round,
            hardness: 1.0,
            opacity: 1.0,
            backface_cos: 0.0,
            material: BrushMaterial::default(),
            camera: None,
            hidden_objects: Vec::new(),
            hidden_sets: Vec::new(),
            dabs: Vec::new(),
        }
    }

    fn ids(model: &Model) -> Vec<u64> {
        model.texture_sets[0]
            .paint
            .strokes
            .iter()
            .map(|s| s.id)
            .collect()
    }

    #[test]
    fn undo_and_redo_strokes() {
        let mut model = Model::from_mesh(Mesh::cube(1.0));
        let mut history = History::default();
        for _ in 0..3 {
            let s = stroke(&mut model);
            history.record(Command::add_stroke(0, s.id));
            model.texture_sets[0].paint.strokes.push(s);
        }
        assert_eq!(ids(&model), [1, 2, 3]);

        assert_eq!(history.undo(&mut model), Some("Stroke"));
        assert_eq!(history.undo(&mut model), Some("Stroke"));
        assert_eq!(ids(&model), [1]);
        assert_eq!(history.redo(&mut model), Some("Stroke"));
        assert_eq!(ids(&model), [1, 2]);

        // A new stroke drops the redo list.
        let s = stroke(&mut model);
        history.record(Command::add_stroke(0, s.id));
        model.texture_sets[0].paint.strokes.push(s);
        assert!(!history.can_redo());
        assert_eq!(ids(&model), [1, 2, 4]);

        while history.undo(&mut model).is_some() {}
        assert!(ids(&model).is_empty());
        assert!(history.can_redo() && !history.can_undo());
    }
}
