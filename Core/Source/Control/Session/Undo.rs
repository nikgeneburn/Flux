use crate::FileSystem::Operations::{Request, Summary};

#[derive(Clone, Debug)]
pub struct UndoRecord {
    pub Title: String,
    pub Forward: Request,
    pub Reverse: Request,
}

#[derive(Clone, Debug)]
pub enum Purpose {
    Normal,
    Undo(UndoRecord),
    Redo,
}

#[derive(Default)]
pub struct UndoStack {
    Done: Vec<UndoRecord>,
    Undone: Vec<UndoRecord>,
}

const Limit: usize = 100;

impl UndoStack {
    pub fn Push(&mut self, Record: UndoRecord) {
        self.Done.push(Record);
        if self.Done.len() > Limit {
            self.Done.remove(0);
        }
        self.Undone.clear();
    }

    pub fn PushUndone(&mut self, Record: UndoRecord) {
        self.Undone.push(Record);
    }

    pub fn PushDone(&mut self, Record: UndoRecord) {
        self.Done.push(Record);
    }

    pub fn TakeUndo(&mut self) -> Option<UndoRecord> {
        self.Done.pop()
    }

    pub fn TakeRedo(&mut self) -> Option<UndoRecord> {
        self.Undone.pop()
    }
}

pub fn RecordFor(Result: &Summary) -> Option<UndoRecord> {
    match &Result.Request {
        Request::CopyInto { .. } => {
            if Result.Created.is_empty() {
                return None;
            }
            Some(UndoRecord {
                Title: format!("Copy of {} item{}", Result.Created.len(), if Result.Created.len() == 1 { "" } else { "s" }),
                Forward: Result.Request.clone(),
                Reverse: Request::Recycle(Result.Created.clone()),
            })
        }
        Request::MoveInto { .. } | Request::MovePairs(_) => {
            if Result.Moved.is_empty() {
                return None;
            }
            let Reverse: Vec<_> = Result.Moved.iter().map(|(From, To)| (To.clone(), From.clone())).collect();
            let IsRename = Result.Moved.len() == 1 && Result.Moved[0].0.parent() == Result.Moved[0].1.parent();
            Some(UndoRecord {
                Title: if IsRename { "Rename".to_string() } else { format!("Move of {} item{}", Reverse.len(), if Reverse.len() == 1 { "" } else { "s" }) },
                Forward: Request::MovePairs(Result.Moved.clone()),
                Reverse: Request::MovePairs(Reverse),
            })
        }
        Request::CreateFolder(Item) | Request::CreateFile(Item) => {
            if Result.Created.is_empty() {
                return None;
            }
            Some(UndoRecord { Title: "New item".to_string(), Forward: Result.Request.clone(), Reverse: Request::Recycle(vec![Item.clone()]) })
        }
        Request::Recycle(_) | Request::DeletePermanently(_) => None,
    }
}
