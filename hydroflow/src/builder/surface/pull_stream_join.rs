use super::{BaseSurface, PullSurface};

use std::hash::Hash;
use std::marker::PhantomData;

use crate::builder::build::pull_stream_join::StreamJoinPullBuild;
use crate::lang::lattice::{LatticeRepr, Merge};
use crate::scheduled::handoff::handoff_list::{PortList, PortListSplit};
use crate::scheduled::port::RECV;
use crate::scheduled::type_list::Extend;

pub struct StreamJoinPullSurface<PrevStream, PrevBuf, L, Update>
where
    PrevBuf: PullSurface,
    PrevStream: PullSurface,
    L: LatticeRepr,

    PrevBuf::InputHandoffs: Extend<PrevStream::InputHandoffs>,
    <PrevBuf::InputHandoffs as Extend<PrevStream::InputHandoffs>>::Extended: PortList<RECV>
        + PortListSplit<RECV, PrevBuf::InputHandoffs, Suffix = PrevStream::InputHandoffs>,
{
    prev_stream: PrevStream,
    prev_buf: PrevBuf,
    _marker: PhantomData<(L, Update)>,
}
impl<PrevBuf, PrevStream, Key, L, Update, StreamVal>
    StreamJoinPullSurface<PrevStream, PrevBuf, L, Update>
where
    PrevBuf: PullSurface<ItemOut = (Key, Update::Repr)>,
    PrevStream: PullSurface<ItemOut = (Key, StreamVal)>,
    Key: 'static + Eq + Hash + Clone,
    L: 'static + LatticeRepr + Merge<Update>,
    Update: 'static + LatticeRepr,
    StreamVal: 'static + Clone,

    PrevBuf::InputHandoffs: Extend<PrevStream::InputHandoffs>,
    <PrevBuf::InputHandoffs as Extend<PrevStream::InputHandoffs>>::Extended: PortList<RECV>
        + PortListSplit<RECV, PrevBuf::InputHandoffs, Suffix = PrevStream::InputHandoffs>,
{
    pub fn new(prev_stream: PrevStream, prev_buf: PrevBuf) -> Self {
        Self {
            prev_stream,
            prev_buf,
            _marker: PhantomData,
        }
    }
}

impl<PrevBuf, PrevStream, Key, L, Update, StreamVal> BaseSurface
    for StreamJoinPullSurface<PrevStream, PrevBuf, L, Update>
where
    PrevBuf: PullSurface<ItemOut = (Key, Update::Repr)>,
    PrevStream: PullSurface<ItemOut = (Key, StreamVal)>,
    Key: 'static + Eq + Hash + Clone,
    L: 'static + LatticeRepr + Merge<Update>,
    Update: 'static + LatticeRepr,
    StreamVal: 'static + Clone,

    PrevBuf::InputHandoffs: Extend<PrevStream::InputHandoffs>,
    <PrevBuf::InputHandoffs as Extend<PrevStream::InputHandoffs>>::Extended: PortList<RECV>
        + PortListSplit<RECV, PrevBuf::InputHandoffs, Suffix = PrevStream::InputHandoffs>,
{
    type ItemOut = (Key, StreamVal, L::Repr);
}

impl<PrevBuf, PrevStream, Key, L, Update, StreamVal> PullSurface
    for StreamJoinPullSurface<PrevStream, PrevBuf, L, Update>
where
    PrevBuf: PullSurface<ItemOut = (Key, Update::Repr)>,
    PrevStream: PullSurface<ItemOut = (Key, StreamVal)>,
    Key: 'static + Eq + Hash + Clone,
    L: 'static + LatticeRepr + Merge<Update>,
    L::Repr: Default + Clone,
    Update: 'static + LatticeRepr,
    StreamVal: 'static + Clone,

    PrevBuf::InputHandoffs: Extend<PrevStream::InputHandoffs>,
    <PrevBuf::InputHandoffs as Extend<PrevStream::InputHandoffs>>::Extended: PortList<RECV>
        + PortListSplit<RECV, PrevBuf::InputHandoffs, Suffix = PrevStream::InputHandoffs>,
{
    type InputHandoffs = <PrevBuf::InputHandoffs as Extend<PrevStream::InputHandoffs>>::Extended;
    type Build = StreamJoinPullBuild<PrevBuf::Build, PrevStream::Build, Key, L, Update, StreamVal>;

    fn into_parts(self) -> (Self::InputHandoffs, Self::Build) {
        let (connect_a, build_a) = self.prev_buf.into_parts();
        let (connect_b, build_b) = self.prev_stream.into_parts();
        let connect = connect_a.extend(connect_b);
        let build = StreamJoinPullBuild::new(build_a, build_b);
        (connect, build)
    }
}
