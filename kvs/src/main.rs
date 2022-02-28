#![feature(never_type)]

use std::{collections::{HashMap, HashSet}, sync::Arc, time::Duration};

use hydroflow::{
    builder::{
        prelude::{BaseSurface, PullSurface, PushSurface},
        surface::pull_iter::IterPullSurface,
        HydroflowBuilder,
    },
    lang::{
        collections::Single,
        lattice::{
            dom_pair::DomPairRepr,
            map_union::MapUnionRepr,
            ord::{Max, MaxRepr},
            set_union::{SetUnionRepr, SetUnion},
            LatticeRepr, Merge,
        },
        tag,
    },
    scheduled::handoff::VecHandoff,
    tokio::{
        self,
        sync::{
            mpsc::{channel, Receiver, Sender},
            Mutex,
        },
    },
};

#[derive(Debug, Copy, Clone, Hash, Eq, PartialEq)]
struct ActorId(u64);

type Timestamp = HashMap<usize, u64>;

type ClockRepr = MapUnionRepr<tag::HASH_MAP, usize, MaxRepr<u64>>;
type ClockUpdateRepr = MapUnionRepr<tag::SINGLE, usize, MaxRepr<u64>>;

type DataRepr<K, V> = MapUnionRepr<tag::HASH_MAP, K, DomPairRepr<ClockRepr, MaxRepr<V>>>;
type BatchRepr<K, V> = MapUnionRepr<tag::VEC, K, DomPairRepr<ClockRepr, MaxRepr<V>>>;
type UpdateRepr<K, V> = MapUnionRepr<tag::SINGLE, K, DomPairRepr<ClockRepr, MaxRepr<V>>>;

#[derive(Clone, Debug)]
enum Message<K, V>
where
    K: Send + Clone,
    V: Send + Clone,
{
    // A KV set request from a client.
    Set(K, V),
    // A set of data that I am responsible for, sent to me by another worker.
    Batch((usize, u64), Vec<(K, V)>),
}

unsafe impl<K, V> Send for Message<K, V>
where
    K: Send + Clone,
    V: Send + Clone,
{
}

fn main() {
    let workers = 2;
    let senders = spawn_threads::<String, String>(workers);
    let mut i = 0;
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(10)).await;
            for s in &senders {
                i += 1;
                s.send(Message::Set(format!("foo{}", i % 100), "bar".into()))
                    .await
                    .unwrap();
            }
        }
    });
}

type Matrix<K, V> = Vec<(Receiver<Message<K, V>>, Vec<Sender<Message<K, V>>>)>;
type MessageSender<K, V> = Sender<Message<K, V>>;

fn make_communication_matrix<K, V>(n: u64) -> (Matrix<K, V>, Vec<MessageSender<K, V>>)
where
    K: Send + Clone,
    V: Send + Clone,
{
    let mut receivers = Vec::new();
    let mut senders: Vec<_> = (0..n).map(|_| Vec::new()).collect();
    let mut extra_senders = Vec::new();
    for _ in 0..n {
        let (sender, receiver) = channel(1024);
        receivers.push(receiver);
        for s in senders.iter_mut() {
            s.push(sender.clone())
        }
        extra_senders.push(sender);
    }

    (
        receivers.into_iter().zip(senders.into_iter()).collect(),
        extra_senders,
    )
}

fn spawn<F, K, V>(n: u64, f: F) -> Vec<Sender<Message<K, V>>>
where
    F: 'static + Fn(usize, Receiver<Message<K, V>>, Vec<Sender<Message<K, V>>>) + Send + Clone,
    K: 'static + Send + Clone,
    V: 'static + Send + Clone,
{
    let (matrix, senders) = make_communication_matrix(n);
    for (i, (receiver, senders)) in matrix.into_iter().enumerate() {
        let f = f.clone();
        std::thread::spawn(move || f(i, receiver, senders));
    }

    senders
}

// TODO(justin): add a stack-allocated implementation of this.
fn owners<K: std::hash::Hash>(n: u64, _v: &K) -> Vec<usize> {
    (0..n as usize).collect()
}

// TODO(justin): this thing is hacky.
#[derive(Debug, Copy, Clone)]
struct PerQuantumPulser {
    on: bool,
}

impl PerQuantumPulser {
    fn new() -> Self {
        PerQuantumPulser { on: true }
    }
}

impl Iterator for PerQuantumPulser {
    type Item = ();
    fn next(&mut self) -> Option<()> {
        self.on = !self.on;
        if self.on {
            None
        } else {
            Some(())
        }
    }
}

fn spawn_threads<K, V>(workers: u64) -> Vec<Sender<Message<K, V>>>
where
    K: 'static + Clone + Eq + std::hash::Hash + Send + std::fmt::Debug,
    V: 'static + Clone + Send + std::fmt::Debug + Ord + Default,
{
    spawn(
        workers,
        move |id, mut receiver: Receiver<Message<K, V>>, senders: Vec<Sender<Message<K, V>>>| {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async move {
                let mut hf = HydroflowBuilder::default();

                let (z_send, z_recv) = hf.add_channel_input::<Option<u64>, VecHandoff<u64>>();

                // Construct the ticker.
                let epoch_duration = Duration::from_millis(100);
                tokio::spawn(async move {
                    let mut tick = 0;
                    loop {
                        tokio::time::sleep(epoch_duration).await;
                        z_send.give(Some(tick));
                        z_send.flush();
                        tick += 1;
                    }
                });

                let (reads_send, reads_recv) =
                    hf.add_channel_input::<Option<K>, VecHandoff<K>>();

                let _ = reads_send;

                let (q_send, q_recv) =
                    hf.add_channel_input::<Option<Message<K, V>>, VecHandoff<Message<K, V>>>();
                // Feed incoming messages into a Hydroflow channel.
                // TODO(justin): not sure why the tokio Receiver doesn't support try_iter?
                tokio::spawn(async move {
                    // TODO(justin): batch all these per try_recv?
                    while let Some(v) = receiver.recv().await {
                        q_send.give(Some(v));
                        q_send.flush();
                    }
                });

                // Make the ownership table. Everyone owns everything.
                let ownership = (0..senders.len()).map(|i| ((), Single(i)));

                let (x_send, x_recv) =
                    hf.make_edge::<VecHandoff<(usize, u64, Vec<(K, V)>)>, Option<(usize, u64, Vec<(K, V)>)>>();
                let (y_send, y_recv) = hf.make_edge::<VecHandoff<(K, V)>, Option<(K, V)>>();

                // TODO(justin): this is wrong and bad, need to figure out how to do this properly.
                // let clock: <ClockRepr as LatticeRepr>::Repr = Default::default();
                // let clock = Arc::new(Mutex::new(clock));

                // C.
                hf.add_subgraph(
                    y_recv
                        .flatten()
                        .map(|(k, v)| Single((k, v)))
                        .batch_with::<_, MapUnionRepr<tag::HASH_MAP, K, MaxRepr<V>>, MapUnionRepr<tag::SINGLE, K, MaxRepr<V>>, _>(z_recv.flatten())
                        .flat_map(|(epoch, batch)| {
                            batch.into_iter().map(move |x| (epoch, x))
                        })
                        // TODO(justin): have a real hasher here, right now have
                        // exactly one bucket everyone gets written into.
                        .map(|(epoch, kv)| {
                            ((), (epoch, kv))
                        })
                        .stream_join::<_, _, _, SetUnionRepr<tag::VEC, usize>, SetUnionRepr<tag::SINGLE, usize>>(IterPullSurface::new(ownership))
                        .flat_map(|((), (epoch, (k, v)), receiver)| {
                            receiver.into_iter().map(move |i|
                                Single(((i, epoch), Single((k.clone(), v.clone()))))
                            )
                        })
                        .batch_with::<
                            _,
                            MapUnionRepr<tag::HASH_MAP, (usize, u64), SetUnionRepr<tag::VEC, (K, V)>>, 
                            MapUnionRepr<tag::SINGLE, (usize, u64), SetUnionRepr<tag::SINGLE, (K, V)>>,
                            _,
                        >(IterPullSurface::new(PerQuantumPulser::new()))
                        .map(|((), batch)| batch)
                        .filter(|batch| !batch.is_empty())
                        .flatten()
                        .pull_to_push()
                        .for_each(move |((receiver, epoch), batch)| {
                            // TODO(justin): do we need to tag this with our current vector clock as well?
                            senders[receiver].try_send(Message::Batch((id, epoch), batch)).unwrap();
                        }),
                );

                // B.
                hf.add_subgraph(
                    q_recv.flatten().pull_to_push().partition(
                        |x| matches!(x, Message::Set(_, _)),
                        hf.start_tee()
                            .map(|msg| {
                                if let Message::Set(k, v) = msg {
                                    Some((k, v))
                                } else {
                                    unreachable!()
                                }
                            })
                            .push_to(y_send),
                        hf.start_tee()
                            .map(|msg| {
                                if let Message::Batch((id, epoch), batch) = msg {
                                    Some((id, epoch, batch))
                                } else {
                                    unreachable!()
                                }
                            })
                            .push_to(x_send),
                    ),
                );

                // A.
                hf.add_subgraph(reads_recv.flatten().map(|k| (k, ())).stream_join::<_, _, _, MaxRepr<V>, MaxRepr<V>>(
                    x_recv.flatten().flat_map(|(id, epoch, batch)| {
                        println!("got batch: {:?} {:?} {:?}", id, epoch, batch);
                        // TODO(justin): this doesn't do the clocks. I
                        // think we need to do some weird join against a
                        // singleton that I don't really understand yet.
                        batch.into_iter()
                    })).pull_to_push().for_each(|x| {
                        println!("read: {:?}", x)
                    })
                );

                hf.build().run_async().await.unwrap();
            })
        },
    )
}
