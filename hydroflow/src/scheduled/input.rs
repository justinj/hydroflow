use std::{cell::RefCell, marker::PhantomData, rc::Rc, sync::mpsc::SyncSender};

use super::{OpId, Reactor};

pub trait Give<T> {
    fn give(&self, t: T) -> bool;
}

pub struct Buffer<T>(pub(crate) Rc<RefCell<Vec<T>>>);
impl<T> Give<T> for Buffer<T> {
    fn give(&self, t: T) -> bool {
        (*self.0).borrow_mut().push(t);
        true
    }
}

impl<T> Default for Buffer<T> {
    fn default() -> Self {
        Buffer(Rc::new(RefCell::new(Vec::new())))
    }
}

impl<T> Clone for Buffer<T> {
    fn clone(&self) -> Self {
        Buffer(self.0.clone())
    }
}

impl<T> Give<T> for SyncSender<T> {
    fn give(&self, t: T) -> bool {
        matches!(self.send(t), Ok(_))
    }
}

// TODO(justin): this thing should probably give Vecs to the Givable, and buffer
// stuff up and automatically flush, but postponing that until we have occasion
// to benchmark it.
pub struct Input<T, G>
where
    G: Give<T>,
{
    reactor: Reactor,
    op_id: OpId,
    givable: G,
    _marker: PhantomData<T>,
}
impl<T, G> Input<T, G>
where
    G: Give<T>,
{
    pub fn new(reactor: Reactor, op_id: OpId, givable: G) -> Self {
        Input {
            reactor,
            op_id,
            givable,
            _marker: PhantomData,
        }
    }

    pub fn give(&self, t: T) {
        self.givable.give(t);
    }

    pub fn flush(&self) {
        self.reactor.trigger(self.op_id).unwrap(/* TODO(justin) */);
    }
}