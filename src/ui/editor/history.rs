
use std::time::{Duration, Instant};

use super::buffer::{Buffer, Edit};
use super::selection::{Selection, same_positions};

const GROUP_INTERVAL: Duration = Duration::from_millis(800);
const MAX_STEPS: usize = 10_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditKind {
    Insert,
    Delete,
    Other,
}

#[derive(Clone, Debug)]
pub struct Transaction {
    edits: Vec<(std::ops::Range<usize>, String, String)>,
}

impl Transaction {
    pub fn record(buffer: &Buffer, edits: &[Edit]) -> Self {
        Self {
            edits: edits
                .iter()
                .map(|(range, text)| (range.clone(), buffer.slice(range.clone()), text.clone()))
                .collect(),
        }
    }

    pub fn redo_edits(&self) -> Vec<Edit> {
        self.edits
            .iter()
            .map(|(range, _, new)| (range.clone(), new.clone()))
            .collect()
    }

    pub fn undo_edits(&self) -> Vec<Edit> {
        let mut delta: isize = 0;
        let mut inverse = Vec::with_capacity(self.edits.len());
        for (range, old, new) in &self.edits {
            let start = (range.start as isize + delta) as usize;
            inverse.push((start..start + new.len(), old.clone()));
            delta += new.len() as isize - (range.end - range.start) as isize;
        }
        inverse
    }
}

#[derive(Clone, Debug)]
struct Entry {
    transactions: Vec<Transaction>,
    selections_before: Vec<Selection>,
    selections_after: Vec<Selection>,
    kind: EditKind,
    at: Instant,
}

pub struct Step {
    pub edits: Vec<Vec<Edit>>,
    pub selections: Vec<Selection>,
}

#[derive(Default)]
pub struct History {
    undo: Vec<Entry>,
    redo: Vec<Entry>,
    break_group: bool,
}

impl History {
    pub fn push(
        &mut self,
        transaction: Transaction,
        before: Vec<Selection>,
        after: Vec<Selection>,
        kind: EditKind,
    ) {
        self.redo.clear();
        let now = Instant::now();
        if !self.break_group
            && kind != EditKind::Other
            && let Some(last) = self.undo.last_mut()
            && last.kind == kind
            && now.duration_since(last.at) < GROUP_INTERVAL
            && same_positions(&last.selections_after, &before)
        {
            last.transactions.push(transaction);
            last.selections_after = after;
            last.at = now;
            return;
        }
        self.break_group = false;
        self.undo.push(Entry {
            transactions: vec![transaction],
            selections_before: before,
            selections_after: after,
            kind,
            at: now,
        });
        if self.undo.len() > MAX_STEPS {
            self.undo.remove(0);
        }
    }

    pub fn set_selections_after(&mut self, selections: Vec<Selection>) {
        if let Some(last) = self.undo.last_mut() {
            last.selections_after = selections;
        }
    }

    pub fn break_group(&mut self) {
        self.break_group = true;
    }

    #[cfg(test)]
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn undo(&mut self) -> Option<Step> {
        let entry = self.undo.pop()?;
        let step = Step {
            edits: entry
                .transactions
                .iter()
                .rev()
                .map(Transaction::undo_edits)
                .collect(),
            selections: entry.selections_before.clone(),
        };
        self.redo.push(entry);
        self.break_group = true;
        Some(step)
    }

    pub fn redo(&mut self) -> Option<Step> {
        let entry = self.redo.pop()?;
        let step = Step {
            edits: entry
                .transactions
                .iter()
                .map(Transaction::redo_edits)
                .collect(),
            selections: entry.selections_after.clone(),
        };
        self.undo.push(entry);
        self.break_group = true;
        Some(step)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(
        buffer: &mut Buffer,
        history: &mut History,
        edits: Vec<Edit>,
        kind: EditKind,
        caret: usize,
    ) {
        let transaction = Transaction::record(buffer, &edits);
        buffer.apply(&edits);
        let before = vec![Selection::cursor(0, caret)];
        let after = vec![Selection::cursor(0, caret + 1)];
        history.push(transaction, before, after, kind);
    }

    #[test]
    fn typing_groups_and_undoes_together() {
        let mut buffer = Buffer::new("");
        let mut history = History::default();
        apply(
            &mut buffer,
            &mut history,
            vec![(0..0, "a".into())],
            EditKind::Insert,
            0,
        );
        apply(
            &mut buffer,
            &mut history,
            vec![(1..1, "b".into())],
            EditKind::Insert,
            1,
        );
        assert_eq!(buffer.text(), "ab");
        let step = history.undo().unwrap();
        for edits in &step.edits {
            buffer.apply(edits);
        }
        assert_eq!(buffer.text(), "");
        assert!(!history.can_undo());
        let step = history.redo().unwrap();
        for edits in &step.edits {
            buffer.apply(edits);
        }
        assert_eq!(buffer.text(), "ab");
    }

    #[test]
    fn multi_edit_transactions_invert() {
        let mut buffer = Buffer::new("one two three");
        let mut history = History::default();
        let edits = vec![(0..3, "1".into()), (8..13, "THREE!".into())];
        apply(&mut buffer, &mut history, edits, EditKind::Other, 0);
        assert_eq!(buffer.text(), "1 two THREE!");
        let step = history.undo().unwrap();
        for edits in &step.edits {
            buffer.apply(edits);
        }
        assert_eq!(buffer.text(), "one two three");
    }

    #[test]
    fn other_edits_never_group() {
        let mut buffer = Buffer::new("");
        let mut history = History::default();
        apply(
            &mut buffer,
            &mut history,
            vec![(0..0, "a".into())],
            EditKind::Other,
            0,
        );
        apply(
            &mut buffer,
            &mut history,
            vec![(1..1, "b".into())],
            EditKind::Other,
            1,
        );
        history.undo().unwrap();
        assert!(history.can_undo());
    }
}
