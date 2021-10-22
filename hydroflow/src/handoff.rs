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
