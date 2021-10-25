use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use crate::collections::Iter;

pub trait TryCanReceive<T> {
    fn try_give(&mut self, item: T) -> Result<T, T>;
}
pub trait CanReceive<T> {
    fn give(&mut self, item: T) -> T;
}

pub trait Handoff: Default + HandoffMeta {
    fn give<T>(&mut self, item: T) -> T
    where
        Self: CanReceive<T>,
    {
        <Self as CanReceive<T>>::give(self, item)
    }

    fn try_give<T>(&mut self, item: T) -> Result<T, T>
    where
        Self: TryCanReceive<T>,
    {
        <Self as TryCanReceive<T>>::try_give(self, item)
    }
}

#[derive(Default)]
pub struct NullHandoff;
impl Handoff for NullHandoff {}

/**
 * A [VecDeque]-based FIFO handoff.
 */
pub struct DequeHandoff<T>(pub(crate) VecDeque<T>);
impl<T> Default for DequeHandoff<T> {
    fn default() -> Self {
        Self(Default::default())
    }
}
impl<T> Handoff for DequeHandoff<T> {}

impl<T> CanReceive<Option<T>> for DequeHandoff<T> {
    fn give(&mut self, mut item: Option<T>) -> Option<T> {
        if let Some(item) = item.take() {
            self.0.push_back(item)
        }
        None
    }
}
impl<T, I> CanReceive<Iter<I>> for DequeHandoff<T>
where
    I: Iterator<Item = T>,
{
    fn give(&mut self, mut iter: Iter<I>) -> Iter<I> {
        self.0.extend(&mut iter.0);
        iter
    }
}
impl<T> CanReceive<VecDeque<T>> for DequeHandoff<T> {
    fn give(&mut self, mut vec: VecDeque<T>) -> VecDeque<T> {
        self.0.extend(vec.drain(..));
        vec
    }
}

// /**
//  * A trait specifying a handoff point between compiled subgraphs.
//  */
// pub trait Handoff {
//     type Item;

//     fn new() -> Self;

//     #[allow(clippy::result_unit_err)]
//     fn try_give(&mut self, item: Self::Item) -> Result<(), ()>;

//     fn is_bottom(&self) -> bool;
// }

/**
 * A handle onto the metadata part of a [Handoff], with no element type.
 */
pub trait HandoffMeta {
    // TODO(justin): more fine-grained info here.
    fn is_bottom(&self) -> bool;
}

// /**
//  * A null handoff which will panic when called.
//  *
//  * This is used in sources and sinks as the unused read or write handoff respectively.
//  */
// pub struct NullHandoff;
// impl Handoff for NullHandoff {
//     type Item = ();

//     fn new() -> Self {
//         NullHandoff
//     }

//     fn try_give(&mut self, _item: Self::Item) -> Result<(), ()> {
//         panic!("Tried to write to null handoff.");
//     }

//     fn is_bottom(&self) -> bool {
//         true
//     }
// }
impl HandoffMeta for NullHandoff {
    fn is_bottom(&self) -> bool {
        true
    }
}

// /**
//  * A [VecDeque]-based FIFO handoff.
//  */
// pub struct VecHandoff<T>(pub(crate) VecDeque<T>);
// impl<T> Handoff for VecHandoff<T> {
//     type Item = T;

//     fn new() -> Self {
//         VecHandoff(VecDeque::new())
//     }

//     fn try_give(&mut self, t: Self::Item) -> Result<(), ()> {
//         self.0.push_back(t);
//         Ok(())
//     }

//     fn is_bottom(&self) -> bool {
//         self.0.is_empty()
//     }
// }

impl<T> HandoffMeta for DequeHandoff<T> {
    fn is_bottom(&self) -> bool {
        self.0.is_empty()
    }
}

impl<H> HandoffMeta for Rc<RefCell<H>>
where
    H: HandoffMeta,
{
    fn is_bottom(&self) -> bool {
        self.borrow().is_bottom()
    }
}

struct ReaderHandoff<T> {
    contents: VecDeque<Vec<T>>,
    empties: Vec<Vec<T>>,
}

impl<T> Default for ReaderHandoff<T> {
    fn default() -> Self {
        Self {
            contents: Default::default(),
            empties: Default::default(),
        }
    }
}

impl<T> ReaderHandoff<T> {
    fn give(&mut self, mut v: &mut Vec<T>) {
        match self.empties.pop() {
            None => {
                self.contents.push_back(std::mem::take(v));
            }
            Some(mut empty) => {
                std::mem::swap(&mut empty, v);
                self.contents.push_back(empty);
            }
        }
    }

    pub fn for_each<F>(&mut self, mut f: F)
    where
        F: FnMut(&mut Vec<T>),
    {
        for mut batch in self.contents.drain(..) {
            f(&mut batch);
            assert!(batch.is_empty());
            self.empties.push(batch);
        }
    }
}

// Shamelessly cribbed this design from Timely.
struct TeeingHandoff<T> {
    contents: Vec<T>,
    buffer: Vec<T>,
    listeners: Vec<Rc<RefCell<ReaderHandoff<T>>>>,
}

impl<T> Default for TeeingHandoff<T> {
    fn default() -> Self {
        TeeingHandoff {
            contents: Vec::with_capacity(1024),
            buffer: Vec::with_capacity(1024),
            listeners: Vec::new(),
        }
    }
}

impl<T: Clone> TeeingHandoff<T> {
    fn flush(&mut self) {
        if !self.contents.is_empty() {
            for i in 1..self.listeners.len() {
                self.buffer.extend_from_slice(&self.contents);
                (*self.listeners[i]).borrow_mut().give(&mut self.buffer);
            }
            if !self.listeners.is_empty() {
                (*self.listeners[0]).borrow_mut().give(&mut self.contents);
            }
        }
    }

    pub fn give(&mut self, t: T) {
        self.contents.push(t);
        if self.contents.len() >= self.buffer.capacity() {
            self.flush();
        }
    }

    pub fn give_vec(&mut self, v: &mut Vec<T>) {
        self.flush();
        std::mem::swap(v, &mut self.contents);
        self.flush();
    }

    fn subscribe(&mut self) -> Rc<RefCell<ReaderHandoff<T>>> {
        let h = Rc::new(RefCell::new(ReaderHandoff::default()));
        self.listeners.push(h.clone());
        h
    }
}

// impl<T> HandoffMeta for BufferedHandoff<T> {
//     fn is_bottom(&self) -> bool {
//         self.contents.is_empty()
//     }
// }

// impl<T> Handoff for BufferedHandoff<T> {}

// impl<T: Clone> BufferedHandoff {
//     fn give() {}
// }
