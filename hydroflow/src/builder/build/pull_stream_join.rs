use super::{PullBuild, PullBuildBase};

use std::hash::Hash;

use crate::compiled::pull::{StreamJoin, StreamJoinState};
use crate::scheduled::context::Context;
use crate::scheduled::handoff::handoff_list::{PortList, PortListSplit};
use crate::scheduled::port::RECV;
use crate::scheduled::type_list::Extend;

pub struct StreamJoinPullBuild<PrevBuf, PrevStream, Key, BufVal, StreamVal>
where
    PrevBuf: PullBuild<ItemOut = (Key, BufVal)>,
    PrevStream: PullBuild<ItemOut = (Key, StreamVal)>,
    Key: 'static + Eq,
    BufVal: 'static,
    StreamVal: 'static,

    PrevBuf::InputHandoffs: Extend<PrevStream::InputHandoffs>,
    <PrevBuf::InputHandoffs as Extend<PrevStream::InputHandoffs>>::Extended: PortList<RECV>
        + PortListSplit<RECV, PrevBuf::InputHandoffs, Suffix = PrevStream::InputHandoffs>,
{
    prev_a: PrevBuf,
    prev_b: PrevStream,
    state: StreamJoinState<Key, BufVal, StreamVal>,
}
impl<PrevBuf, PrevStream, Key, BufVal, StreamVal>
    StreamJoinPullBuild<PrevBuf, PrevStream, Key, BufVal, StreamVal>
where
    PrevBuf: PullBuild<ItemOut = (Key, BufVal)>,
    PrevStream: PullBuild<ItemOut = (Key, StreamVal)>,
    Key: 'static + Eq + Hash,
    BufVal: 'static,
    StreamVal: 'static,

    PrevBuf::InputHandoffs: Extend<PrevStream::InputHandoffs>,
    <PrevBuf::InputHandoffs as Extend<PrevStream::InputHandoffs>>::Extended: PortList<RECV>
        + PortListSplit<RECV, PrevBuf::InputHandoffs, Suffix = PrevStream::InputHandoffs>,
{
    pub fn new(prev_a: PrevBuf, prev_b: PrevStream) -> Self {
        Self {
            prev_a,
            prev_b,
            state: Default::default(),
        }
    }
}

impl<PrevBuf, PrevStream, Key, BufVal, StreamVal> PullBuildBase
    for StreamJoinPullBuild<PrevBuf, PrevStream, Key, BufVal, StreamVal>
where
    PrevBuf: PullBuild<ItemOut = (Key, BufVal)>,
    PrevStream: PullBuild<ItemOut = (Key, StreamVal)>,
    Key: 'static + Eq + Hash + Clone,
    BufVal: 'static + Clone,
    StreamVal: 'static + Clone,

    PrevBuf::InputHandoffs: Extend<PrevStream::InputHandoffs>,
    <PrevBuf::InputHandoffs as Extend<PrevStream::InputHandoffs>>::Extended: PortList<RECV>
        + PortListSplit<RECV, PrevBuf::InputHandoffs, Suffix = PrevStream::InputHandoffs>,
{
    type ItemOut = (Key, StreamVal, BufVal);
    type Build<'slf, 'hof> = StreamJoin<
        'slf,
        Key,
        PrevBuf::Build<'slf, 'hof>,
        BufVal,
        PrevStream::Build<'slf, 'hof>,
        StreamVal,
    >;
}

impl<PrevBuf, PrevStream, Key, BufVal, StreamVal> PullBuild
    for StreamJoinPullBuild<PrevBuf, PrevStream, Key, BufVal, StreamVal>
where
    PrevBuf: PullBuild<ItemOut = (Key, BufVal)>,
    PrevStream: PullBuild<ItemOut = (Key, StreamVal)>,
    Key: 'static + Eq + Hash + Clone,
    BufVal: 'static + Clone,
    StreamVal: 'static + Clone,

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
        StreamJoin::new(iter_a, iter_b, &mut self.state)
    }
}
