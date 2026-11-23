//! Branching undo history of whole-project snapshots.
//!
//! The states form a tree. Undo moves to the parent and Redo back along the line last walked.
//! A new step made after undoing starts a new branch and the undone states stay in the tree,
//! reachable with [`History::goto`].
//!
//! Every state is kept for the life of the session: snapshots share their unchanged data, so a
//! step costs what it changed. A limit comes with the memory budget (plan §6).

use std::sync::Arc;

use iskra_model::Project;
use serde::Serialize;

struct Node {
    label: String,
    state: Arc<Project>,
    parent: Option<usize>,
    /// The child Redo moves to.
    redo: Option<usize>,
}

/// A place in the history: how many states it had, the current one, and that one's redo.
pub(crate) type Mark = (usize, usize, Option<usize>);

pub struct History {
    nodes: Vec<Node>,
    current: usize,
}

/// One state of the history tree, as listed by `edit.history`.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    /// The state's index, stable for the life of the session.
    pub index: usize,
    /// The step that made this state.
    pub label: String,
    pub parent: Option<usize>,
    pub current: bool,
}

impl Default for History {
    fn default() -> Self {
        Self::new(Arc::new(Project::starting()))
    }
}

impl History {
    /// A history whose only state is `project`.
    pub fn new(project: Arc<Project>) -> Self {
        Self { nodes: vec![Node { label: "Original".to_string(), state: project, parent: None, redo: None }], current: 0 }
    }

    pub fn state(&self) -> &Arc<Project> {
        &self.nodes[self.current].state
    }

    /// The index of the current state, as [`HistoryEntry::index`] has it.
    pub fn current(&self) -> usize {
        self.current
    }

    /// Add `state` as a step after the current state and move to it.
    pub fn push(&mut self, label: &str, state: Arc<Project>) {
        let index = self.nodes.len();
        self.nodes.push(Node { label: label.to_string(), state, parent: Some(self.current), redo: None });
        self.nodes[self.current].redo = Some(index);
        self.current = index;
    }

    pub fn can_undo(&self) -> bool {
        self.nodes[self.current].parent.is_some()
    }

    pub fn can_redo(&self) -> bool {
        self.nodes[self.current].redo.is_some()
    }

    /// Label of the step Undo would take back.
    pub fn undo_label(&self) -> Option<&str> {
        self.can_undo().then(|| self.nodes[self.current].label.as_str())
    }

    /// Label of the step Redo would apply again.
    pub fn redo_label(&self) -> Option<&str> {
        self.nodes[self.current].redo.map(|i| self.nodes[i].label.as_str())
    }

    pub fn undo(&mut self) -> bool {
        let Some(parent) = self.nodes[self.current].parent else { return false };
        self.nodes[parent].redo = Some(self.current);
        self.current = parent;
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(child) = self.nodes[self.current].redo else { return false };
        self.current = child;
        true
    }

    /// Jump to any state of the tree. Redo from its ancestors then leads back to it.
    pub fn goto(&mut self, index: usize) -> bool {
        if index >= self.nodes.len() {
            return false;
        }
        let mut child = index;
        while let Some(parent) = self.nodes[child].parent {
            self.nodes[parent].redo = Some(child);
            child = parent;
        }
        self.current = index;
        true
    }

    /// Where the history is now, to come back to with [`History::fold`].
    pub(crate) fn mark(&self) -> Mark {
        (self.nodes.len(), self.current, self.nodes[self.current].redo)
    }

    /// Make one step of all that was pushed since `mark`, named `label`: the state the history
    /// is at now, straight after the state it was at then. With `keep` false, or if nothing
    /// came of it, the steps are forgotten and the history is as it was at the mark.
    pub(crate) fn fold(&mut self, (length, current, redo): Mark, label: &str, keep: bool) {
        let (was, now) = (self.nodes[current].state.clone(), self.state().clone());
        self.nodes.truncate(length);
        self.current = current;
        self.nodes[current].redo = redo;
        if keep && !Arc::ptr_eq(&was, &now) && *was != *now {
            self.push(label, now);
        }
    }

    /// The snapshot of every state, in the order they were made.
    pub(crate) fn states(&self) -> impl Iterator<Item = &Arc<Project>> {
        self.nodes.iter().map(|n| &n.state)
    }

    /// Replace the snapshot of each state for which `f` gives another. This is for changes
    /// that are not edits, such as loading a shot's pixels: the tree and its labels stay.
    pub(crate) fn replace_states(&mut self, mut f: impl FnMut(&Arc<Project>) -> Option<Arc<Project>>) {
        for node in &mut self.nodes {
            if let Some(state) = f(&node.state) {
                node.state = state;
            }
        }
    }

    /// Every state, in the order they were made.
    pub fn entries(&self) -> Vec<HistoryEntry> {
        self.nodes
            .iter()
            .enumerate()
            .map(|(index, n)| HistoryEntry { index, label: n.label.clone(), parent: n.parent, current: index == self.current })
            .collect()
    }
}
