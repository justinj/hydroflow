pub mod context;
pub mod graph;
pub mod graph_ext;
pub mod handoff;
#[cfg(feature = "variadic_generics")]
pub mod input;
pub mod net;
pub mod port;
pub mod query;
pub mod reactor;
pub mod state;
pub(crate) mod subgraph;
pub mod type_list;
pub mod util;

pub type SubgraphId = usize;
pub type HandoffId = usize;
pub type StateId = usize;

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc};

    use crate::{
        builder::{
            prelude::{BaseSurface, PullSurface, PushSurface},
            HydroflowBuilder,
        },
        scheduled::handoff::VecHandoff,
    };

    #[test]
    fn test_batcher() {
        let outputs = Rc::new(RefCell::new(Vec::new()));
        let mut df = HydroflowBuilder::default();

        let (stream_input, stream_hoff) = df.add_channel_input::<Option<u64>, VecHandoff<_>>();
        let (ticks_input, ticks_hoff) = df.add_channel_input::<Option<u64>, VecHandoff<_>>();

        let outputs_inner = outputs.clone();
        df.add_subgraph(
            stream_hoff
                .flatten()
                .map(|x| ((), x))
                .batch_with(ticks_hoff.flatten().map(|x| ((), x)))
                .map(|((), x, v)| (x, v))
                .pull_to_push()
                .for_each(move |x| (*outputs_inner).borrow_mut().push(x)),
        );

        let mut df = df.build();

        stream_input.give(Some(1));
        stream_input.give(Some(2));
        stream_input.give(Some(3));
        stream_input.flush();
        ticks_input.give(Some(1));
        ticks_input.flush();

        df.tick();
        assert_eq!(vec![(vec![1, 2, 3], 1)], *outputs.borrow());

        ticks_input.give(Some(2));
        ticks_input.flush();

        df.tick();
        assert_eq!(vec![(vec![1, 2, 3], 1)], *outputs.borrow());

        stream_input.give(Some(4));
        stream_input.give(Some(5));
        stream_input.flush();

        df.tick();
        assert_eq!(vec![(vec![1, 2, 3], 1)], *outputs.borrow());

        ticks_input.give(Some(3));
        ticks_input.flush();

        df.tick();
        assert_eq!(vec![(vec![1, 2, 3], 1), (vec![4, 5], 3)], *outputs.borrow());
    }
}
