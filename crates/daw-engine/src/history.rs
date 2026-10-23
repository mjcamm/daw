//! Undo and redo.
//!
//! Every edit returns its own inverse, so history is two stacks of inverse
//! edits rather than copies of the project.

use crate::project::Project;
use daw_protocol::{CommandError, Edit};

#[derive(Debug, Default)]
pub struct History {
    undo: Vec<Edit>,
    redo: Vec<Edit>,
}

impl History {
    /// Applies a new edit, recording its inverse and discarding the redo stack.
    pub fn edit(&mut self, project: &mut Project, edit: Edit) -> Result<(), CommandError> {
        let inverse = project.apply(edit)?;
        self.undo.push(inverse);
        self.redo.clear();
        Ok(())
    }

    /// Records an edit already made to the project (a knob turned in a
    /// plugin's window, applied as it moved) by its inverse.
    pub fn record(&mut self, inverse: Edit) {
        self.undo.push(inverse);
        self.redo.clear();
    }

    pub fn undo(&mut self, project: &mut Project) -> Result<(), CommandError> {
        let inverse = self.undo.pop().ok_or(CommandError::NothingToUndo)?;
        let redo = Self::reapply(project, inverse);
        self.redo.push(redo);
        Ok(())
    }

    pub fn redo(&mut self, project: &mut Project) -> Result<(), CommandError> {
        let edit = self.redo.pop().ok_or(CommandError::NothingToRedo)?;
        let inverse = Self::reapply(project, edit);
        self.undo.push(inverse);
        Ok(())
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Applies an edit taken from history. These were valid when recorded, so
    /// failure means history and project have diverged.
    fn reapply(project: &mut Project, edit: Edit) -> Edit {
        project
            .apply(edit)
            .unwrap_or_else(|e| panic!("history diverged from project: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tempo(bpm: f64) -> Edit {
        Edit::SetTempo { bpm }
    }

    #[test]
    fn undo_and_redo_walk_through_edits() {
        let mut project = Project::default();
        let mut history = History::default();
        history.edit(&mut project, tempo(130.0)).unwrap();
        history.edit(&mut project, tempo(140.0)).unwrap();

        history.undo(&mut project).unwrap();
        assert_eq!(project.tempo_bpm, 130.0);
        history.undo(&mut project).unwrap();
        assert_eq!(project.tempo_bpm, 120.0);
        assert_eq!(history.undo(&mut project), Err(CommandError::NothingToUndo));

        history.redo(&mut project).unwrap();
        history.redo(&mut project).unwrap();
        assert_eq!(project.tempo_bpm, 140.0);
        assert_eq!(history.redo(&mut project), Err(CommandError::NothingToRedo));
    }

    #[test]
    fn new_edit_discards_redo() {
        let mut project = Project::default();
        let mut history = History::default();
        history.edit(&mut project, tempo(130.0)).unwrap();
        history.undo(&mut project).unwrap();
        assert!(history.can_redo());

        history.edit(&mut project, tempo(90.0)).unwrap();
        assert!(!history.can_redo());
    }

    #[test]
    fn rejected_edit_is_not_recorded() {
        let mut project = Project::default();
        let mut history = History::default();
        assert!(history.edit(&mut project, tempo(0.0)).is_err());
        assert!(!history.can_undo());
    }
}
