pub mod collections;
pub mod ctx;
pub mod handoff;
#[cfg(feature = "variadic_generics")]
pub mod input;
pub mod net;
pub mod query;
pub mod state;
pub(crate) mod subgraph;
pub mod util;

mod handoff_list;
pub use handoff_list::HandoffList;

use std::any::Any;
use std::cell::Cell;
use std::collections::{HashMap, VecDeque};
use std::marker::PhantomData;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver, RecvError, SyncSender, TrySendError};
use std::task::{self, Poll};
use std::time::Duration;

use futures::stream::Stream;
use ref_cast::RefCast;

use crate::tl;
use ctx::{InputPort, OutputPort, RecvCtx, SendCtx};
use handoff::{Handoff, HandoffMeta, VecHandoff};
use state::StateHandle;
#[cfg(feature = "variadic_generics")]
use subgraph::Subgraph;

use self::handoff::CanReceive;
use self::input::{Buffer, Input};

pub type SubgraphId = usize;
pub type HandoffId = usize;
pub type StateId = usize;

/**
 * A Hydroflow graph. Owns, schedules, and runs the compiled subgraphs.
 */
pub struct Hydroflow {
    subgraphs: Vec<SubgraphData>,
    handoffs: Vec<HandoffData>,

    states: Vec<StateData>,

    // TODO(mingwei): separate scheduler into its own struct/trait?
    ready_queue: VecDeque<SubgraphId>,
    event_queue_send: SyncSender<SubgraphId>, // TODO(mingwei) remove this, to prevent hanging.
    event_queue_recv: Receiver<SubgraphId>,

    name_map: HashMap<String, SubgraphId>,
}
impl Default for Hydroflow {
    fn default() -> Self {
        let (subgraphs, handoffs, states, ready_queue, name_map) = Default::default();
        let (event_queue_send, event_queue_recv) = mpsc::sync_channel(8_000);
        Self {
            subgraphs,
            handoffs,
            states,
            ready_queue,
            event_queue_send,
            event_queue_recv,
            name_map,
        }
    }
}
impl Hydroflow {
    /**
     * Create an new empty Dataflow graph.
     */
    pub fn new() -> Self {
        Default::default()
    }

    /**
     * Returns a reactor for externally scheduling subgraphs, possibly from another thread.
     */
    pub fn reactor(&self) -> Reactor {
        Reactor {
            event_queue_send: self.event_queue_send.clone(),
        }
    }

    fn enqueue_jobs(&mut self) {
        for sg in self.event_queue_recv.try_iter() {
            if !self.subgraphs[sg].scheduled {
                self.ready_queue.push_back(sg);
                self.subgraphs[sg].scheduled = true;
            }
        }
    }

    /**
     * Runs the dataflow until no more work is currently available.
     */
    pub fn tick(&mut self) {
        // Add any external jobs to ready queue.
        self.enqueue_jobs();

        while let Some(sg_id) = self.ready_queue.pop_front() {
            self.subgraphs[sg_id].scheduled = false;
            let sg_data = self.subgraphs.get_mut(sg_id).unwrap(/* TODO(mingwei) */);
            let context = Context {
                subgraph_id: sg_id,
                handoffs: &mut self.handoffs,
                states: &mut self.states,
                event_queue_send: &mut self.event_queue_send,
            };
            sg_data.subgraph.run(context);
            for handoff_id in sg_data.succs.iter().copied() {
                let handoff = self.handoffs.get(handoff_id).unwrap(/* TODO(mingwei) */);
                let succ_id = handoff.succ;
                if self.ready_queue.contains(&succ_id) {
                    // TODO(mingwei): Slow? O(N)
                    continue;
                }
                if !handoff.handoff.is_bottom() {
                    self.ready_queue.push_back(succ_id);
                }
            }

            self.enqueue_jobs();
        }
    }

    /**
     * Run the dataflow graph, blocking until completion.
     */
    pub fn run(&mut self) -> Result<!, RecvError> {
        loop {
            self.tick();
            self.poll_events()?;
        }
    }

    /**
     * Run the dataflow graph to completition asynchronously.
     */
    pub async fn run_async(&mut self) -> Result<!, RecvError> {
        loop {
            self.tick();
            self.poll_events()?;
            tokio::task::yield_now().await;
            // TODO(mingwei): this busy-spins when other tasks are not running.
        }
    }

    /**
     * Block and wait for an external event.
     */
    fn poll_events(&mut self) -> Result<(), RecvError> {
        self.ready_queue.extend(self.event_queue_recv.try_iter());
        Ok(())
    }

    /**
     * TODO(mingwei): Hack to re-enqueue all subgraphs.
     */
    pub fn wake_all(&mut self) {
        self.ready_queue.clear();
        self.ready_queue.extend(0..self.subgraphs.len());
    }

    fn add_subgraph_metadata(
        &mut self,
        id: SubgraphId,
        name: Option<String>,
        s: impl 'static + Subgraph,
    ) {
        let name = name.unwrap_or_else(|| format!("__subgraph_{}", self.subgraphs.len()));
        match self.name_map.get(&name) {
            None => {
                self.subgraphs.push(SubgraphData::new(&name, s));
                self.name_map.insert(name, id);
                self.ready_queue.push_back(id);
            }
            Some(_) => {
                panic!("duplicate name: {}", name)
            }
        }
    }

    /**
     * Adds a new compiled subgraph with the specified inputs and outputs with a
     * given name.
     *
     * See [HandoffList] for how to specify inputs and outputs.
     */
    #[cfg(feature = "variadic_generics")]
    #[must_use]
    fn add_subgraph_internal<F, R, W>(
        &mut self,
        name: Option<String>,
        mut subgraph: F,
    ) -> (R::InputPort, W::OutputPort)
    where
        F: 'static + FnMut(&Context<'_>, R::RecvCtx<'_>, W::SendCtx<'_>),
        R: 'static + HandoffList,
        W: 'static + HandoffList,
    {
        // TODO(justin): make this less sketchy, we just know we're the only person who will append here.
        let sg_id = self.subgraphs.len();

        let (input_hids, input_ports) = R::make_input(sg_id);
        let (output_hids, output_ports) = W::make_output(sg_id);

        let subgraph = move |context: Context<'_>| {
            let recv = R::make_recv(context.handoffs, &input_hids);
            let send = W::make_send(context.handoffs, &output_hids);
            (subgraph)(&context, recv, send);
        };
        self.add_subgraph_metadata(sg_id, name, subgraph);

        (input_ports, output_ports)
    }

    /**
     * Adds a new compiled subgraph with the specified inputs and outputs with a
     * given name.
     *
     * See [HandoffList] for how to specify inputs and outputs.
     */
    #[cfg(feature = "variadic_generics")]
    #[must_use]
    pub fn add_named_subgraph<F, R, W>(
        &mut self,
        name: &str,
        subgraph: F,
    ) -> (R::InputPort, W::OutputPort)
    where
        F: 'static + FnMut(&Context<'_>, R::RecvCtx<'_>, W::SendCtx<'_>),
        R: 'static + HandoffList,
        W: 'static + HandoffList,
    {
        self.add_subgraph_internal(Some(name.to_owned()), subgraph)
    }

    /**
     * Adds a new compiled subgraph with the specified inputs and outputs. A
     * name will be autogenerated for the subgraph.
     *
     * See [HandoffList] for how to specify inputs and outputs.
     */
    #[cfg(feature = "variadic_generics")]
    #[must_use]
    pub fn add_subgraph<F, R, W>(&mut self, subgraph: F) -> (R::InputPort, W::OutputPort)
    where
        F: 'static + FnMut(&Context<'_>, R::RecvCtx<'_>, W::SendCtx<'_>),
        R: 'static + HandoffList,
        W: 'static + HandoffList,
    {
        self.add_subgraph_internal(None, subgraph)
    }

    /**
     * Adds a new compiled subraph with a single input and output, and returns the input/output handles.
     */
    #[cfg(feature = "variadic_generics")]
    pub fn add_inout<F, R, W>(&mut self, mut subgraph: F) -> (InputPort<R>, OutputPort<W>)
    where
        F: 'static + FnMut(&Context<'_>, &RecvCtx<R>, &SendCtx<W>),
        R: 'static + Handoff,
        W: 'static + Handoff,
    {
        let (tl!(input_port), tl!(output_port)) =
            self.add_subgraph::<_, tl!(R), tl!(W)>(move |ctx, tl!(recv), tl!(send)| {
                (subgraph)(ctx, recv, send)
            });
        (input_port, output_port)
    }

    /**
     * Adds a new compiled subraph with one input and two outputs, and returns the input/output handles.
     */
    pub fn add_binary_out<F, R, W1, W2>(
        &mut self,
        mut subgraph: F,
    ) -> (InputPort<R>, OutputPort<W1>, OutputPort<W2>)
    where
        F: 'static + FnMut(&Context<'_>, &RecvCtx<R>, &SendCtx<W1>, &SendCtx<W2>),
        R: 'static + Handoff,
        W1: 'static + Handoff,
        W2: 'static + Handoff,
    {
        let (tl!(input_port), tl!(output_port1, output_port2)) = self
            .add_subgraph::<_, tl!(R), tl!(W1, W2)>(move |ctx, tl!(recv), tl!(send1, send2)| {
                (subgraph)(ctx, recv, send1, send2)
            });
        (input_port, output_port1, output_port2)
    }

    /**
     * Adds a new compiled subraph with a variable number of inputs and outputs.
     */
    pub fn add_n_in_m_out<F, R, W>(
        &mut self,
        n: usize,
        m: usize,
        f: F,
    ) -> (Vec<InputPort<R>>, Vec<OutputPort<W>>)
    where
        F: 'static + FnMut(&[&RecvCtx<R>], &[&SendCtx<W>]),
        R: 'static + Handoff,
        W: 'static + Handoff,
    {
        // TODO(justin): is there a nice way to encapsulate the below?
        let sg_id = self.subgraphs.len();

        let mut input_hids = Vec::new();
        input_hids.resize_with(n, <Rc<Cell<Option<HandoffId>>>>::default);
        let mut output_hids = Vec::new();
        output_hids.resize_with(m, <Rc<Cell<Option<HandoffId>>>>::default);

        let input_ports = input_hids
            .iter()
            .cloned()
            .map(|handoff_id| InputPort {
                sg_id,
                handoff_id,
                _phantom: PhantomData,
            })
            .collect();
        let output_ports = output_hids
            .iter()
            .cloned()
            .map(|handoff_id| OutputPort {
                sg_id,
                handoff_id,
                _phantom: PhantomData,
            })
            .collect();

        let mut f = f;
        let subgraph = move |context: Context<'_>| {
            let recvs: Vec<&RecvCtx<R>> = input_hids
                .iter()
                .map(|hid| hid.get().expect("Attempted to use unattached handoff."))
                .map(|hid| context.handoffs.get(hid).unwrap())
                .map(|h_data| {
                    h_data
                        .handoff
                        .any_ref()
                        .downcast_ref()
                        .expect("Attempted to cast handoff to wrong type.")
                })
                .map(RefCast::ref_cast)
                .collect();

            let sends: Vec<&SendCtx<W>> = output_hids
                .iter()
                .map(|hid| hid.get().expect("Attempted to use unattached handoff."))
                .map(|hid| context.handoffs.get(hid).unwrap())
                .map(|h_data| {
                    h_data
                        .handoff
                        .any_ref()
                        .downcast_ref()
                        .expect("Attempted to cast handoff to wrong type.")
                })
                .map(RefCast::ref_cast)
                .collect();

            // self.handoffs.
            f(&recvs, &sends)
        };
        self.add_subgraph_metadata(sg_id, None, subgraph);

        (input_ports, output_ports)
    }

    /**
     * Adds a new compiled subraph with one input and two outputs, and returns the input/output handles.
     */
    pub fn add_binary_in_binary_out<F, R1, R2, W1, W2>(
        &mut self,
        mut subgraph: F,
    ) -> (InputPort<R1>, InputPort<R2>, OutputPort<W1>, OutputPort<W2>)
    where
        F: 'static + FnMut(&Context<'_>, &RecvCtx<R1>, &RecvCtx<R2>, &SendCtx<W1>, &SendCtx<W2>),
        R1: 'static + Handoff,
        R2: 'static + Handoff,
        W1: 'static + Handoff,
        W2: 'static + Handoff,
    {
        let (tl!(input_port1, input_port2), tl!(output_port1, output_port2)) = self
            .add_subgraph::<_, tl!(R1, R2), tl!(W1, W2)>(
                move |ctx, tl!(recv1, recv2), tl!(send1, send2)| {
                    (subgraph)(ctx, recv1, recv2, send1, send2)
                },
            );
        (input_port1, input_port2, output_port1, output_port2)
    }

    /**
     * Adds a new compiled subraph with two inputs and a single output, and returns the input/output handles.
     */
    #[cfg(feature = "variadic_generics")]
    pub fn add_binary<F, R1, R2, W>(
        &mut self,
        mut subgraph: F,
    ) -> (InputPort<R1>, InputPort<R2>, OutputPort<W>)
    where
        F: 'static + FnMut(&Context<'_>, &RecvCtx<R1>, &RecvCtx<R2>, &SendCtx<W>),
        R1: 'static + Handoff,
        R2: 'static + Handoff,
        W: 'static + Handoff,
    {
        let (tl!(input_port1, input_port2), tl!(output_port)) = self
            .add_subgraph::<_, tl!(R1, R2), tl!(W)>(move |ctx, tl!(recv1, recv2), tl!(send)| {
                (subgraph)(ctx, recv1, recv2, send)
            });
        (input_port1, input_port2, output_port)
    }

    /**
     * Adds a new compiled subraph with two inputs and no outputs, and returns the input handles.
     */
    #[cfg(feature = "variadic_generics")]
    pub fn add_binary_sink<F, R1, R2>(&mut self, mut subgraph: F) -> (InputPort<R1>, InputPort<R2>)
    where
        F: 'static + FnMut(&Context<'_>, &RecvCtx<R1>, &RecvCtx<R2>),
        R1: 'static + Handoff,
        R2: 'static + Handoff,
    {
        let (tl!(input_port1, input_port2), tl!()) =
            self.add_subgraph::<_, tl!(R1, R2), tl!()>(move |ctx, tl!(recv1, recv2), tl!()| {
                (subgraph)(ctx, recv1, recv2)
            });
        (input_port1, input_port2)
    }

    /**
     * Adds an "input" operator, along with a handle to insert data into it.
     */
    #[cfg(feature = "variadic_generics")]
    pub fn add_input<T, W>(&mut self) -> (Input<T, Buffer<T>>, OutputPort<W>)
    where
        T: 'static,
        W: 'static + Handoff + CanReceive<T>,
    {
        let input = Buffer::default();
        let inner_input = input.clone();
        let output_port = self.add_source::<_, W>(move |_ctx, send| {
            for x in (*inner_input.0).borrow_mut().drain(..) {
                send.give(x);
            }
        });
        let id = output_port.sg_id;
        (Input::new(self.reactor(), id, input), output_port)
    }

    pub fn add_input_from_stream<T, W, S>(&mut self, mut s: S) -> OutputPort<W>
    where
        S: 'static + Stream<Item = T> + Unpin,
        W: 'static + Handoff + CanReceive<T>,
    {
        let output_port = self.add_source::<_, W>(move |ctx, send| {
            let waker = ctx.waker();
            let mut cx = task::Context::from_waker(&waker);
            let mut r = Pin::new(&mut s);
            while let Poll::Ready(Some(v)) = r.poll_next(&mut cx) {
                send.give(v);
                r = Pin::new(&mut s);
            }
        });
        output_port
    }

    /**
     * Adds a threadsafe "input" operator, along with a handle to insert data into it.
     */
    #[cfg(feature = "variadic_generics")]
    pub fn add_channel_input<T, W>(&mut self) -> (Input<T, SyncSender<T>>, OutputPort<W>)
    where
        T: 'static,
        W: 'static + Handoff + CanReceive<T>,
    {
        let (sender, receiver) = mpsc::sync_channel(8000);
        let output_port = self.add_source::<_, W>(move |_ctx, send| {
            for x in receiver.try_iter() {
                send.give(x);
            }
        });
        let id = output_port.sg_id;
        (Input::new(self.reactor(), id, sender), output_port)
    }

    pub fn subgraph_id_from_name(&self, name: &str) -> Option<SubgraphId> {
        self.name_map.get(name).cloned()
    }

    /**
     * Adds a new compiled subgraph with no inputs and one output.
     */
    #[cfg(feature = "variadic_generics")]
    pub fn add_source<F, W>(&mut self, mut subgraph: F) -> OutputPort<W>
    where
        F: 'static + FnMut(&Context<'_>, &SendCtx<W>),
        W: 'static + Handoff,
    {
        let (tl!(), tl!(output_port)) = self
            .add_subgraph::<_, tl!(), tl!(W)>(move |ctx, tl!(), tl!(send)| (subgraph)(ctx, send));
        output_port
    }

    /**
     * Adds a new compiled subgraph with one input and no outputs.
     */
    #[cfg(feature = "variadic_generics")]
    pub fn add_sink<F, R>(&mut self, mut subgraph: F) -> InputPort<R>
    where
        F: 'static + FnMut(&Context<'_>, &RecvCtx<R>),
        R: 'static + Handoff,
    {
        let (tl!(input_port), tl!()) = self
            .add_subgraph::<_, tl!(R), tl!()>(move |ctx, tl!(recv), tl!()| (subgraph)(ctx, recv));
        input_port
    }

    /**
     * Adds a new compiled subgraph with one input and no outputs.
     */
    #[cfg(feature = "variadic_generics")]
    pub fn add_named_sink<F, R>(&mut self, name: &str, mut subgraph: F) -> InputPort<R>
    where
        F: 'static + FnMut(&Context<'_>, &RecvCtx<R>),
        R: 'static + Handoff,
    {
        let (tl!(input_port), tl!()) = self
            .add_named_subgraph::<_, tl!(R), tl!()>(name, move |ctx, tl!(recv), tl!()| {
                (subgraph)(ctx, recv)
            });
        input_port
    }

    pub fn add_edge<H>(&mut self, output_port: OutputPort<H>, input_port: InputPort<H>)
    where
        H: 'static + Handoff,
    {
        let handoff_id: HandoffId = self.handoffs.len();

        // Send handoff_ids.
        input_port.handoff_id.set(Some(handoff_id));
        output_port.handoff_id.set(Some(handoff_id));

        // Create and insert handoff.
        let handoff = H::default();
        self.handoffs.push(HandoffData::new(
            handoff,
            output_port.sg_id,
            input_port.sg_id,
        ));

        // Add successor & predecessor.
        self.subgraphs[output_port.sg_id].succs.push(handoff_id);
        self.subgraphs[input_port.sg_id].preds.push(handoff_id);
    }

    pub fn add_state<T>(&mut self, state: T) -> StateHandle<T>
    where
        T: Any,
    {
        let state_id: StateId = self.states.len();

        let state_data = StateData {
            state: Box::new(state),
        };
        self.states.push(state_data);

        StateHandle {
            state_id,
            _phantom: PhantomData,
        }
    }
}

pub struct Context<'a> {
    subgraph_id: SubgraphId,
    handoffs: &'a mut [HandoffData],
    states: &'a mut [StateData],
    event_queue_send: &'a mut SyncSender<SubgraphId>,
}
impl<'a> Context<'a> {
    pub fn waker(&self) -> std::task::Waker {
        use futures::task::ArcWake;
        use std::sync::Arc;

        struct ContextWaker {
            subgraph_id: SubgraphId,
            event_queue_send: SyncSender<SubgraphId>,
        }
        impl ArcWake for ContextWaker {
            fn wake_by_ref(arc_self: &Arc<Self>) {
                arc_self.event_queue_send.send(arc_self.subgraph_id).unwrap(/* TODO(mingwei) */);
            }
        }

        let context_waker = ContextWaker {
            subgraph_id: self.subgraph_id,
            event_queue_send: self.event_queue_send.clone(),
        };
        futures::task::waker(Arc::new(context_waker))
    }

    pub fn state_ref<T>(&self, handle: StateHandle<T>) -> &T
    where
        T: Any,
    {
        self.states
            .get(handle.state_id)
            .expect("Failed to find state with given handle.")
            .state
            .downcast_ref()
            .expect("StateHandle wrong type T for casting.")
    }

    pub fn state_mut<T>(&mut self, handle: StateHandle<T>) -> &mut T
    where
        T: Any,
    {
        self.states
            .get_mut(handle.state_id)
            .expect("Failed to find state with given handle.")
            .state
            .downcast_mut()
            .expect("StateHandle wrong type T for casting.")
    }
}

/**
 * A handle into a specific [Hydroflow] instance for triggering subgraphs to run, possibly from another thread.
 */
#[derive(Clone)]
pub struct Reactor {
    event_queue_send: SyncSender<SubgraphId>,
}
impl Reactor {
    pub fn trigger(&self, sg_id: SubgraphId) -> Result<(), TrySendError<usize>> {
        self.event_queue_send.try_send(sg_id)
    }

    #[cfg(feature = "async")]
    pub fn into_waker(self, sg_id: SubgraphId) -> std::task::Waker {
        use futures::task::ArcWake;
        use std::sync::Arc;

        struct ReactorWaker {
            reactor: Reactor,
            sg_id: SubgraphId,
        }
        impl ArcWake for ReactorWaker {
            fn wake_by_ref(arc_self: &Arc<Self>) {
                arc_self.reactor.trigger(arc_self.sg_id).unwrap(/* TODO(mingwei) */);
            }
        }

        let reactor_waker = ReactorWaker {
            reactor: self,
            sg_id,
        };
        futures::task::waker(Arc::new(reactor_waker))
    }
}

/**
 * A handoff and its input and output [SubgraphId]s.
 *
 * NOT PART OF PUBLIC API.
 */
pub struct HandoffData {
    handoff: Box<dyn HandoffMeta>,
    #[allow(dead_code)] // TODO(mingwei)
    pred: SubgraphId,
    succ: SubgraphId,
}
impl HandoffData {
    pub fn new(handoff: impl 'static + HandoffMeta, pred: SubgraphId, succ: SubgraphId) -> Self {
        Self {
            handoff: Box::new(handoff),
            pred,
            succ,
        }
    }
}

/**
 * A subgraph along with its predecessor and successor [SubgraphId]s.
 * Used internally by the [Hydroflow] struct to represent the dataflow graph structure.
 */
struct SubgraphData {
    subgraph: Box<dyn Subgraph>,
    preds: Vec<HandoffId>,
    succs: Vec<HandoffId>,
    name: String,
    scheduled: bool,
}
impl SubgraphData {
    pub fn new(name: &str, subgraph: impl 'static + Subgraph) -> Self {
        Self {
            subgraph: Box::new(subgraph),
            preds: Default::default(),
            succs: Default::default(),
            name: name.to_owned(),
            scheduled: true,
        }
    }
}

pub struct StateData {
    state: Box<dyn Any>,
}

#[test]
fn map_filter() {
    use std::cell::RefCell;
    use std::rc::Rc;

    // A simple dataflow with one source feeding into one sink with some processing in the middle.
    let mut df = Hydroflow::new();

    let data = [1, 2, 3, 4];
    let source = df.add_source(move |_ctx, send| {
        for x in data.into_iter() {
            send.give(Some(x));
        }
    });

    let (map_in, map_out) = df.add_inout(
        |_ctx, recv: &RecvCtx<VecHandoff<i32>>, send: &SendCtx<VecHandoff<_>>| {
            for x in recv.take_inner().into_iter() {
                send.give(Some(3 * x + 1));
            }
        },
    );

    let (filter_in, filter_out) = df.add_inout(
        |_ctx, recv: &RecvCtx<VecHandoff<i32>>, send: &SendCtx<VecHandoff<_>>| {
            for x in recv.take_inner().into_iter() {
                if x % 2 == 0 {
                    send.give(Some(x));
                }
            }
        },
    );

    let outputs = Rc::new(RefCell::new(Vec::new()));
    let inner_outputs = outputs.clone();
    let sink = df.add_sink(move |_ctx, recv: &RecvCtx<VecHandoff<i32>>| {
        for x in recv.take_inner().into_iter() {
            (*inner_outputs).borrow_mut().push(x);
        }
    });

    df.add_edge(source, map_in);
    df.add_edge(map_out, filter_in);
    df.add_edge(filter_out, sink);

    df.tick();

    assert_eq!((*outputs).borrow().clone(), vec![4, 10]);
}

mod tests {
    #![allow(unused_imports)]
    use std::{
        cell::{Cell, RefCell},
        collections::{HashMap, HashSet},
        rc::Rc,
    };

    use crate::scheduled::{handoff::Handoff, Hydroflow, RecvCtx, SendCtx, VecHandoff};

    #[test]
    fn test_basic_variadic() {
        let mut df = Hydroflow::new();
        let source_handle = df.add_source(move |_ctx, send: &SendCtx<VecHandoff<usize>>| {
            send.give(Some(5));
        });

        let val = <Rc<Cell<Option<usize>>>>::default();
        let val_ref = val.clone();

        let sink_handle = df.add_sink(move |_ctx, recv: &RecvCtx<VecHandoff<usize>>| {
            for v in recv.take_inner().into_iter() {
                let old_val = val_ref.replace(Some(v));
                assert!(old_val.is_none()); // Only run once.
            }
        });

        df.add_edge(source_handle, sink_handle);
        df.tick();

        assert_eq!(Some(5), val.get());
    }

    #[test]
    fn test_basic_n_m() {
        let mut df = Hydroflow::new();
        let (_, mut source_handle) = df.add_n_in_m_out(
            0,
            1,
            move |_: &[&RecvCtx<VecHandoff<usize>>], send: &[&SendCtx<VecHandoff<usize>>]| {
                send[0].give(Some(5));
            },
        );

        let val = <Rc<Cell<Option<usize>>>>::default();
        let val_ref = val.clone();

        let (mut sink_handle, _) = df.add_n_in_m_out(
            1,
            0,
            move |recv: &[&RecvCtx<VecHandoff<usize>>], _: &[&SendCtx<VecHandoff<usize>>]| {
                for v in recv[0].take_inner().into_iter() {
                    let old_val = val_ref.replace(Some(v));
                    assert!(old_val.is_none()); // Only run once.
                }
            },
        );

        df.add_edge(source_handle.pop().unwrap(), sink_handle.pop().unwrap());
        df.tick();

        assert_eq!(Some(5), val.get());
    }

    #[test]
    fn test_cycle() {
        // A dataflow that represents graph reachability.

        let mut edges: HashMap<usize, Vec<usize>> = HashMap::new();
        for (from, to) in &[
            (1_usize, 2_usize),
            (1, 3),
            (1, 4),
            (2, 3),
            (2, 5),
            (5, 1),
            (6, 7),
            (7, 8),
        ] {
            edges.entry(*from).or_insert_with(Vec::new).push(*to);
        }

        let mut df = Hydroflow::new();

        let mut initially_reachable = vec![1];
        let reachable = df.add_source(move |_ctx, send: &SendCtx<VecHandoff<usize>>| {
            for v in initially_reachable.drain(..) {
                send.give(Some(v));
            }
        });

        let mut seen = HashSet::new();
        let (distinct_in, distinct_out) = df.add_inout(
            move |_ctx, recv: &RecvCtx<VecHandoff<usize>>, send: &SendCtx<VecHandoff<usize>>| {
                for v in recv.take_inner().into_iter() {
                    if seen.insert(v) {
                        send.give(Some(v));
                    }
                }
            },
        );

        let (merge_lhs, merge_rhs, merge_out) = df.add_binary(
            |_ctx,
             recv1: &RecvCtx<VecHandoff<usize>>,
             recv2: &RecvCtx<VecHandoff<usize>>,
             send: &SendCtx<VecHandoff<usize>>| {
                for v in (recv1.take_inner().into_iter()).chain(recv2.take_inner().into_iter()) {
                    send.give(Some(v));
                }
            },
        );

        let (neighbors_in, neighbors_out) =
            df.add_inout(move |_ctx, recv: &RecvCtx<VecHandoff<usize>>, send| {
                for v in recv.take_inner().into_iter() {
                    if let Some(neighbors) = edges.get(&v) {
                        for &n in neighbors {
                            send.give(Some(n));
                        }
                    }
                }
            });

        let (tee_in, tee_out1, tee_out2) = df.add_binary_out(
            |_ctx,
             recv: &RecvCtx<VecHandoff<usize>>,
             send1: &SendCtx<VecHandoff<usize>>,
             send2: &SendCtx<VecHandoff<usize>>| {
                for v in recv.take_inner().into_iter() {
                    send1.give(Some(v));
                    send2.give(Some(v));
                }
            },
        );

        let reachable_verts = Rc::new(RefCell::new(Vec::new()));
        let reachable_inner = reachable_verts.clone();
        let sink_in = df.add_sink(move |_ctx, recv: &RecvCtx<VecHandoff<usize>>| {
            for v in recv.take_inner().into_iter() {
                (*reachable_inner).borrow_mut().push(v);
            }
        });

        df.add_edge(reachable, merge_lhs);
        df.add_edge(neighbors_out, merge_rhs);
        df.add_edge(merge_out, distinct_in);
        df.add_edge(distinct_out, tee_in);
        df.add_edge(tee_out1, neighbors_in);
        df.add_edge(tee_out2, sink_in);

        df.tick();

        assert_eq!((*reachable_verts).borrow().clone(), vec![1, 2, 3, 4, 5]);
    }
}

// #[test]
// fn test_auto_tee() {
//     use std::cell::RefCell;
//     use std::rc::Rc;

//     use crate::scheduled::handoff::TeeingHandoff;

//     let mut df = Hydroflow::new();

//     let mut data = vec![1, 2, 3, 4];
//     let source = df.add_source(move |send: &SendCtx<TeeingHandoff<_>>| {
//         send.give(std::mem::take(&mut data));
//     });

//     let out1 = Rc::new(RefCell::new(Vec::new()));
//     let out1_inner = out1.clone();

//     let sink1 = df.add_sink(move |recv: &RecvCtx<_>| {
//         for v in recv.take_inner() {
//             out1_inner.borrow_mut().extend(v);
//         }
//     });

//     let out2 = Rc::new(RefCell::new(Vec::new()));
//     let out2_inner = out2.clone();
//     let sink2 = df.add_sink(move |recv: &RecvCtx<_>| {
//         for v in recv.take_inner() {
//             out2_inner.borrow_mut().extend(v);
//         }
//     });

//     df.add_edge(source.clone(), sink1);
//     df.add_edge(source, sink2);

//     df.tick();

//     assert_eq!((*out1).borrow().clone(), vec![1, 2, 3, 4]);
//     assert_eq!((*out2).borrow().clone(), vec![1, 2, 3, 4]);
// }

#[test]
fn test_input_handle() {
    use std::cell::RefCell;

    let mut df = Hydroflow::new();

    let (input, output_port) = df.add_input();

    let vec = Rc::new(RefCell::new(Vec::new()));
    let inner_vec = vec.clone();
    let input_port = df.add_sink(move |_ctx, recv: &RecvCtx<VecHandoff<usize>>| {
        for v in recv.take_inner() {
            (*inner_vec).borrow_mut().push(v);
        }
    });

    df.add_edge(output_port, input_port);

    input.give(Some(1));
    input.give(Some(2));
    input.give(Some(3));
    input.flush();

    df.tick();

    assert_eq!((*vec).borrow().clone(), vec![1, 2, 3]);

    input.give(Some(4));
    input.give(Some(5));
    input.give(Some(6));
    input.flush();

    df.tick();

    assert_eq!((*vec).borrow().clone(), vec![1, 2, 3, 4, 5, 6]);
}

#[test]
fn test_input_handle_thread() {
    use std::cell::RefCell;

    let mut df = Hydroflow::new();

    let (input, output_port) = df.add_channel_input();

    let vec = Rc::new(RefCell::new(Vec::new()));
    let inner_vec = vec.clone();
    let input_port = df.add_sink(move |_ctx, recv: &RecvCtx<VecHandoff<usize>>| {
        for v in recv.take_inner() {
            (*inner_vec).borrow_mut().push(v);
        }
    });

    df.add_edge(output_port, input_port);

    let (done, wait) = mpsc::channel();

    std::thread::spawn(move || {
        input.give(Some(1));
        input.give(Some(2));
        input.give(Some(3));
        input.flush();
        done.send(()).unwrap();
    });

    wait.recv().unwrap();

    df.tick();

    assert_eq!((*vec).borrow().clone(), vec![1, 2, 3]);
}

#[test]
fn test_input_channel() {
    // This test creates two parallel Hydroflow graphs and bounces messages back
    // and forth between them.

    use futures::channel::mpsc::channel;
    use std::cell::Cell;

    let (s1, r1) = channel(8000);
    let (s2, r2) = channel(8000);

    let mut s1_outer = s1.clone();
    let pairs = [(s1, r2), (s2, r1)];

    // logger/recv is a channel that each graph plops their messages into, to be
    // able to trace what happens.
    let (logger, mut recv) = channel(8000);

    for (mut sender, receiver) in pairs {
        let mut logger = logger.clone();
        std::thread::spawn(move || {
            let done = Rc::new(Cell::new(false));
            let done_inner = done.clone();
            let mut df = Hydroflow::new();

            let in_chan = df.add_input_from_stream::<_, VecHandoff<usize>, _>(receiver);
            let input = df.add_sink(move |_ctx, recv| {
                for v in recv.take_inner() {
                    logger.try_send(v).unwrap();
                    if v > 0 && sender.try_send(Some(v - 1)).is_err() {
                        (*done_inner).set(true);
                    }
                }
            });
            df.add_edge(in_chan, input);

            while !(*done).get() {
                df.tick();
                df.poll_events().unwrap();
            }
        });
    }

    s1_outer.try_send(Some(10_usize)).unwrap();

    let mut result = Vec::new();
    let expected = vec![10, 9, 8, 7, 6, 5, 4, 3, 2, 1, 0];
    loop {
        let val = recv.try_next();
        match val {
            Err(_) => {
                if result.len() >= expected.len() {
                    break;
                }
            }
            Ok(None) => {
                break;
            }
            Ok(Some(v)) => {
                result.push(v);
            }
        }
    }
    assert_eq!(result, expected);
}

#[test]
fn test_names() {
    use self::collections::Iter;

    std::thread::spawn(|| {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async move {
            let mut df = Hydroflow::new();

            // TODO(justin): get the OS to allocate a port.
            let (net_input, net_output) = df.bind_one(26258).await;
            println!("first connected");

            let sink_input = df.add_sink(move |_ctx, recv: &RecvCtx<VecHandoff<net::Message>>| {});

            let mut data = vec![1_usize, 2, 3];
            let output = df.add_source(move |_ctx, send: &SendCtx<VecHandoff<net::Message>>| {
                send.give(Iter(data.drain(..).map(|i| net::Message::Data {
                    address: 0,
                    batch: bytes::Bytes::from_iter(i.to_le_bytes().into_iter()),
                })));
            });
            df.add_edge(output, net_input);
            df.add_edge(net_output, sink_input);

            df.run_async().await.unwrap();
        });
    });

    std::thread::spawn(|| {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async move {
            let mut df = Hydroflow::new();

            // TODO(justin): use port that was assigned by OS to the other thread
            let (net_input, net_output) = df.connect("localhost:26258").await;
            println!("second connected");

            let output = df.add_source(move |_ctx, send: &SendCtx<VecHandoff<net::Message>>| {});
            df.add_edge(output, net_input);

            let sink_input = df.add_sink(move |_ctx, recv: &RecvCtx<VecHandoff<net::Message>>| {
                for v in recv.take_inner() {
                    println!("{:?}", v);
                }
            });
            df.add_edge(net_output, sink_input);
            df.run_async().await.unwrap();
        });
    });

    std::thread::sleep(Duration::from_millis(1000));
}
