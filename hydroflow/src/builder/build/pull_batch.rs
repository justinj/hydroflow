use std::marker::PhantomData;

use super::{PullBuild, PullBuildBase};

use crate::compiled::pull::{BatchJoin, BatchJoinState};
use crate::lang::lattice::{LatticeRepr, Merge};
use crate::scheduled::context::Context;
use crate::scheduled::handoff::handoff_list::{PortList, PortListSplit};
use crate::scheduled::port::RECV;
use crate::scheduled::type_list::Extend;

pub struct BatchPullBuild<PrevBuf, PrevStream, L, Update, Tick>
where
    PrevBuf: PullBuild<ItemOut = Update::Repr>,
    PrevStream: PullBuild<ItemOut = Tick>,
    Update: 'static + LatticeRepr,
    L: 'static + LatticeRepr + Merge<Update>,
    L::Repr: Default,

    PrevBuf::InputHandoffs: Extend<PrevStream::InputHandoffs>,
    <PrevBuf::InputHandoffs as Extend<PrevStream::InputHandoffs>>::Extended: PortList<RECV>
        + PortListSplit<RECV, PrevBuf::InputHandoffs, Suffix = PrevStream::InputHandoffs>,
{
    prev_a: PrevBuf,
    prev_b: PrevStream,
    state: BatchJoinState<L>,
    _marker: PhantomData<Update>,
}
impl<PrevBuf, PrevStream, L, Update, Tick> BatchPullBuild<PrevBuf, PrevStream, L, Update, Tick>
where
    PrevBuf: PullBuild<ItemOut = Update::Repr>,
    PrevStream: PullBuild<ItemOut = Tick>,
    Update: 'static + LatticeRepr,
    L: 'static + LatticeRepr + Merge<Update>,
    L::Repr: Default,

    PrevBuf::InputHandoffs: Extend<PrevStream::InputHandoffs>,
    <PrevBuf::InputHandoffs as Extend<PrevStream::InputHandoffs>>::Extended: PortList<RECV>
        + PortListSplit<RECV, PrevBuf::InputHandoffs, Suffix = PrevStream::InputHandoffs>,
{
    pub fn new(prev_a: PrevBuf, prev_b: PrevStream) -> Self {
        Self {
            prev_a,
            prev_b,
            state: Default::default(),
            _marker: PhantomData,
        }
    }
}

impl<PrevBuf, PrevStream, L, Update, Tick> PullBuildBase
    for BatchPullBuild<PrevBuf, PrevStream, L, Update, Tick>
where
    PrevBuf: PullBuild<ItemOut = Update::Repr>,
    PrevStream: PullBuild<ItemOut = Tick>,
    Update: 'static + LatticeRepr,
    L: 'static + LatticeRepr + Merge<Update>,
    L::Repr: Default,

    PrevBuf::InputHandoffs: Extend<PrevStream::InputHandoffs>,
    <PrevBuf::InputHandoffs as Extend<PrevStream::InputHandoffs>>::Extended: PortList<RECV>
        + PortListSplit<RECV, PrevBuf::InputHandoffs, Suffix = PrevStream::InputHandoffs>,
{
    type ItemOut = (Tick, L::Repr);
    type Build<'slf, 'hof> =
        BatchJoin<'slf, PrevBuf::Build<'slf, 'hof>, PrevStream::Build<'slf, 'hof>, L, Update, Tick>;
}

impl<PrevBuf, PrevStream, L, Update, Tick> PullBuild
    for BatchPullBuild<PrevBuf, PrevStream, L, Update, Tick>
where
    PrevBuf: PullBuild<ItemOut = Update::Repr>,
    PrevStream: PullBuild<ItemOut = Tick>,
    Update: 'static + LatticeRepr,
    L: 'static + LatticeRepr + Merge<Update>,
    L::Repr: Default,

    PrevBuf::InputHandoffs: Extend<PrevStream::InputHandoffs>,
    <PrevBuf::InputHandoffs as Extend<PrevStream::InputHandoffs>>::Extended: PortList<RECV>
        + PortListSplit<RECV, PrevBuf::InputHandoffs, Suffix = PrevStream::InputHandoffs>,
{
    type InputHandoffs = <PrevBuf::InputHandoffs as Extend<PrevStream::InputHandoffs>>::Extended;

    fn build<'slf, 'hof>(
        &'slf mut self,
        context: &Context<'_>,
        input: <Self::InputHandoffs as PortList<RECV>>::Ctx<'hof>,
    ) -> Self::Build<'slf, 'hof> {
        let (input_a, input_b) = <Self::InputHandoffs as PortListSplit<_, _>>::split_ctx(input);
        let iter_a = self.prev_a.build(context, input_a);
        let iter_b = self.prev_b.build(context, input_b);
        BatchJoin::new(iter_a, iter_b, &mut self.state)
    }
}
