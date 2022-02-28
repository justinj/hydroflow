//! Structs used to create the user-facing Surface API.
//!
//! Main user-facing traits are [`BaseSurface`], [`PullSurface`], and
//! [`PushSurface`], which provide an iterator-like API with easy method
//! chaining. The traits need to be imported when using the Surface API.
//! You can use the prelude to do this easily:
//! ```ignore
//! use hydroflow::build::prelude::*;
//! ```
//!
//! * [`BaseSurface`] provides linear chaining methods like [`BaseSurface::map`], [`BaseSurface::filter`], etc..
//! * [`PullSurface`] provides methods to combine multiple input streams: [`PullSurface::chain`], [`PullSurface::join`].
//!     * To switch to push, call [`PullSurface::pull_to_push`].
//! * [`PushSurface`] provides sink chaining methods and methods to split into multiple output streams: [`PushSurface::tee`], [`PushSurface::for_each`].
//!
//! For implementation info see [super].

use super::build::{PullBuild, PushBuild};

pub mod filter;
pub mod filter_map;
pub mod flatten;
pub mod map;
pub mod pivot;

pub mod pull_batch;
pub mod pull_chain;
pub mod pull_cross_join;
pub mod pull_handoff;
pub mod pull_iter;
pub mod pull_join;
pub mod pull_stream_join;

pub mod push_for_each;
pub mod push_handoff;
pub mod push_partition;
pub mod push_pivot;
pub mod push_start;
pub mod push_tee;

pub mod exchange;

use std::hash::Hash;

use crate::lang::lattice::{LatticeRepr, Merge};
use crate::scheduled::handoff::handoff_list::{PortList, PortListSplit};
use crate::scheduled::port::{RECV, SEND};
use crate::scheduled::type_list::Extend;

/// Common trait shared between push and pull surface APIs.
///
/// Provides non-push/pull-specific chaining methods.
pub trait BaseSurface {
    type ItemOut;

    fn map<Func, Out>(self, func: Func) -> map::MapSurface<Self, Func>
    where
        Self: Sized,
        Func: FnMut(Self::ItemOut) -> Out,
    {
        map::MapSurface::new(self, func)
    }

    fn flat_map<Func, Out>(self, func: Func) -> flatten::FlattenSurface<map::MapSurface<Self, Func>>
    where
        Self: Sized,
        Func: FnMut(Self::ItemOut) -> Out,
        Out: IntoIterator,
    {
        self.map(func).flatten()
    }

    fn flatten(self) -> flatten::FlattenSurface<Self>
    where
        Self: Sized,
        Self::ItemOut: IntoIterator,
    {
        flatten::FlattenSurface::new(self)
    }

    fn filter<Func>(self, func: Func) -> filter::FilterSurface<Self, Func>
    where
        Self: Sized,
        Func: FnMut(&Self::ItemOut) -> bool,
    {
        filter::FilterSurface::new(self, func)
    }

    fn filter_map<Func, Out>(self, func: Func) -> filter_map::FilterMapSurface<Self, Func>
    where
        Self: Sized,
        Func: FnMut(Self::ItemOut) -> Option<Out>,
    {
        filter_map::FilterMapSurface::new(self, func)
    }

    fn inspect<Func>(self, mut func: Func) -> map::MapSurface<Self, InspectMapFunc<Self, Func>>
    where
        Self: Sized,
        Func: FnMut(&Self::ItemOut),
    {
        self.map(move |item| {
            func(&item);
            item
        })
    }
}

pub type InspectMapFunc<Prev: BaseSurface, Func> = impl FnMut(Prev::ItemOut) -> Prev::ItemOut;

pub trait PullSurface: BaseSurface {
    type InputHandoffs: PortList<RECV>;
    type Build: PullBuild<InputHandoffs = Self::InputHandoffs, ItemOut = Self::ItemOut>;

    fn into_parts(self) -> (Self::InputHandoffs, Self::Build);

    fn chain<Other>(self, other: Other) -> pull_chain::ChainPullSurface<Self, Other>
    where
        Self: Sized,
        Other: PullSurface<ItemOut = Self::ItemOut>,

        Self::InputHandoffs: Extend<Other::InputHandoffs>,
        <Self::InputHandoffs as Extend<Other::InputHandoffs>>::Extended: PortList<RECV>
            + PortListSplit<RECV, Self::InputHandoffs, Suffix = Other::InputHandoffs>,
    {
        pull_chain::ChainPullSurface::new(self, other)
    }

    fn join<Other, Key, ValSelf, ValOther>(
        self,
        other: Other,
    ) -> pull_join::JoinPullSurface<Self, Other>
    where
        Self: Sized + PullSurface<ItemOut = (Key, ValSelf)>,
        Other: PullSurface<ItemOut = (Key, ValOther)>,
        Key: 'static + Eq + Hash + Clone,
        ValSelf: 'static + Eq + Clone,
        ValOther: 'static + Eq + Clone,

        Self::InputHandoffs: Extend<Other::InputHandoffs>,
        <Self::InputHandoffs as Extend<Other::InputHandoffs>>::Extended: PortList<RECV>
            + PortListSplit<RECV, Self::InputHandoffs, Suffix = Other::InputHandoffs>,
    {
        pull_join::JoinPullSurface::new(self, other)
    }

    fn batch_with<Other, L, Update, Tick>(
        self,
        other: Other,
    ) -> pull_batch::BatchPullSurface<Self, Other, L, Update, Tick>
    where
        Self: Sized + PullSurface<ItemOut = Update::Repr>,
        Other: PullSurface<ItemOut = Tick>,
        Update: 'static + LatticeRepr,
        L: 'static + LatticeRepr + Merge<Update>,

        Self::InputHandoffs: Extend<Other::InputHandoffs>,
        <Self::InputHandoffs as Extend<Other::InputHandoffs>>::Extended: PortList<RECV>
            + PortListSplit<RECV, Self::InputHandoffs, Suffix = Other::InputHandoffs>,
    {
        pull_batch::BatchPullSurface::new(self, other)
    }

    fn stream_join<Other, Key, ValSelf, L, Update>(
        self,
        other: Other,
    ) -> pull_stream_join::StreamJoinPullSurface<Self, Other, L, Update>
    where
        Self: Sized + PullSurface<ItemOut = (Key, ValSelf)>,
        Other: PullSurface<ItemOut = (Key, Update::Repr)>,
        Key: 'static + Eq + Hash + Clone,
        ValSelf: 'static + Clone,
        Update: 'static + LatticeRepr,
        L: 'static + LatticeRepr + Merge<Update>,
        L::Repr: Clone,

        Other::InputHandoffs: Extend<Self::InputHandoffs>,
        <Other::InputHandoffs as Extend<Self::InputHandoffs>>::Extended: PortList<RECV>
            + PortListSplit<RECV, Other::InputHandoffs, Suffix = Self::InputHandoffs>,
    {
        pull_stream_join::StreamJoinPullSurface::new(self, other)
    }

    fn cross_join<Other>(self, other: Other) -> pull_cross_join::CrossJoinPullSurface<Self, Other>
    where
        Self: Sized + PullSurface,
        Other: PullSurface,
        Self::ItemOut: 'static + Eq + Clone,
        Other::ItemOut: 'static + Eq + Clone,

        Self::InputHandoffs: Extend<Other::InputHandoffs>,
        <Self::InputHandoffs as Extend<Other::InputHandoffs>>::Extended: PortList<RECV>
            + PortListSplit<RECV, Self::InputHandoffs, Suffix = Other::InputHandoffs>,
    {
        pull_cross_join::CrossJoinPullSurface::new(self, other)
    }

    fn pull_to_push(self) -> push_pivot::PivotPushSurface<Self>
    where
        Self: Sized,
    {
        push_pivot::PivotPushSurface::new(self)
    }
}

pub trait PushSurface: BaseSurface {
    /// This should usually be a type which impls [PushSurfaceReversed], but it is not enforced since we also need to return a Pivot in the end.
    type Output<Next>
    where
        Next: PushSurfaceReversed<ItemIn = Self::ItemOut>;

    fn push_to<Next>(self, next: Next) -> Self::Output<Next>
    where
        Next: PushSurfaceReversed<ItemIn = Self::ItemOut>;

    /// To create a output tee, use [`HydroflowBuilder::start_tee()`](crate::builder::HydroflowBuilder::start_tee).
    fn tee<NextA, NextB>(
        self,
        next_a: NextA,
        next_b: NextB,
    ) -> Self::Output<push_tee::TeePushSurfaceReversed<NextA, NextB>>
    where
        Self: Sized,
        Self::ItemOut: Clone,
        NextA: PushSurfaceReversed<ItemIn = Self::ItemOut>,
        NextB: PushSurfaceReversed<ItemIn = Self::ItemOut>,

        NextA::OutputHandoffs: Extend<NextB::OutputHandoffs>,
        <NextA::OutputHandoffs as Extend<NextB::OutputHandoffs>>::Extended: PortList<SEND>
            + PortListSplit<SEND, NextA::OutputHandoffs, Suffix = NextB::OutputHandoffs>,
    {
        let next = push_tee::TeePushSurfaceReversed::new(next_a, next_b);
        self.push_to(next)
    }

    fn for_each<Func>(
        self,
        func: Func,
    ) -> Self::Output<push_for_each::ForEachPushSurfaceReversed<Func, Self::ItemOut>>
    where
        Self: Sized,
        Func: FnMut(Self::ItemOut),
    {
        let next = push_for_each::ForEachPushSurfaceReversed::new(func);
        self.push_to(next)
    }

    fn partition<Func, NextA, NextB>(
        self,
        func: Func,
        next_a: NextA,
        next_b: NextB,
    ) -> Self::Output<push_partition::PartitionPushSurfaceReversed<NextA, NextB, Func>>
    where
        Self: Sized,
        Func: Fn(&Self::ItemOut) -> bool,
        NextA: PushSurfaceReversed<ItemIn = Self::ItemOut>,
        NextB: PushSurfaceReversed<ItemIn = Self::ItemOut>,

        NextA::OutputHandoffs: Extend<NextB::OutputHandoffs>,
        <NextA::OutputHandoffs as Extend<NextB::OutputHandoffs>>::Extended: PortList<SEND>
            + PortListSplit<SEND, NextA::OutputHandoffs, Suffix = NextB::OutputHandoffs>,
    {
        let next = push_partition::PartitionPushSurfaceReversed::new(func, next_a, next_b);
        self.push_to(next)
    }
}

/// This extra layer is needed due to the ownership order. In the functional
/// chaining syntax each operator owns the previous (can only go in order
/// things are called/defined), but in the end we need each pusherator to own
/// the _next_ pusherator which it's pushing to.
///
/// This is the already-reversed, [PushSurface] does the actual reversing.
pub trait PushSurfaceReversed {
    type ItemIn;

    type OutputHandoffs: PortList<SEND>;
    type Build: PushBuild<OutputHandoffs = Self::OutputHandoffs, ItemIn = Self::ItemIn>;

    fn into_parts(self) -> (Self::OutputHandoffs, Self::Build);
}
