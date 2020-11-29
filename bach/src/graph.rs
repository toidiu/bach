use alloc::{boxed::Box, vec::Vec};
use anymap::AnyMap;
use core::{fmt, future::Future};
use petgraph::{
    graph::{DiGraph, NodeIndex},
    visit::EdgeRef,
    Direction,
};

pub mod dot;

use dot::Dot;

// TODO replace with something else
use alloc::collections::BTreeMap as Map;

pub type Spawn = Box<dyn 'static + Future<Output = ()> + Send>;

#[derive(Clone, Copy, Debug, Default, PartialEq, PartialOrd, Eq, Ord)]
pub struct ChannelID(u32);

impl ChannelID {
    fn new(idx: NodeIndex<u32>) -> Self {
        Self(idx.index() as _)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, PartialOrd, Eq, Ord)]
pub struct PortID(u32);

struct Pair<M>(
    flume::Sender<(ChannelID, M)>,
    flume::Receiver<(ChannelID, M)>,
);

impl<M> Default for Pair<M> {
    fn default() -> Self {
        let (sender, receiver) = flume::unbounded();
        Self(sender, receiver)
    }
}

impl<M: 'static> Pair<M> {
    fn create_into(id: ChannelID, map: &mut PortMap) {
        map.ports
            .entry::<Map<ChannelID, Self>>()
            .or_insert_with(Default::default)
            .insert(id, Pair::default());
    }

    fn resolve(id: ChannelID, map: &PortMap) -> &Self {
        map.ports
            .get::<Map<ChannelID, Self>>()
            .expect("missing channel type")
            .get(&id)
            .expect("missing pair")
    }
}

struct ChannelNode {
    // creates a channel into the channels map
    create_into: fn(id: ChannelID, channels: &mut PortMap),
    // initializes a channel instance from the channel information
    resolve_into: fn(id: ChannelID, outgoing: &[ChannelID], instances: &mut PortMap),
}

impl fmt::Debug for ChannelNode {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Channel")
    }
}

struct ChannelEdge(&'static str);

impl ChannelEdge {
    fn new<T>() -> Self {
        Self(core::any::type_name::<T>())
    }
}

impl fmt::Debug for ChannelEdge {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:?}", self.0)
    }
}
type ChannelGraph = DiGraph<ChannelNode, ChannelEdge, u32>;

trait NodeInstance {
    fn spawn(&self, map: &mut PortMap) -> Spawn;

    fn dot(&self, f: &mut Dot) -> fmt::Result;
}

pub struct Graph {
    nodes: Vec<Box<dyn NodeInstance>>,
    channels: ChannelGraph,
}

impl Default for Graph {
    fn default() -> Self {
        Self {
            nodes: Vec::new(),
            channels: Default::default(),
        }
    }
}

impl Graph {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add<T: Template>(&mut self, template: T) -> <<T::Node as Node>::Port as Port>::Handle {
        let mut nodes = NodeMap { graph: self };
        template.new(&mut nodes)
    }

    pub fn connect<S>(&mut self, source: S) -> Source<S> {
        Source {
            graph: self,
            source,
        }
    }

    pub fn dot(&self) -> GraphDot {
        GraphDot(self)
    }

    pub fn create(&self) -> Instance {
        let mut map = PortMap {
            ports: AnyMap::new(),
        };

        for idx in self.channels.node_indices() {
            let channel = &self.channels[idx];
            let id = ChannelID::new(idx);
            (channel.create_into)(id, &mut map);
        }

        let mut outgoing = Vec::new();

        for idx in self.channels.node_indices() {
            outgoing.clear();

            outgoing.extend(
                self.channels
                    .edges_directed(idx, Direction::Outgoing)
                    .map(|edge| edge.target())
                    .map(ChannelID::new),
            );

            let channel = &self.channels[idx];
            let id = ChannelID::new(idx);
            (channel.resolve_into)(id, &outgoing, &mut map);
        }

        let mut instance = Instance::default();

        for node in &self.nodes {
            instance.futures.push(node.spawn(&mut map));
        }

        instance
    }
}

pub struct GraphDot<'a>(&'a Graph);

impl<'a> fmt::Debug for GraphDot<'a> {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        write!(fmt, "{}", self)
    }
}

impl<'a> fmt::Display for GraphDot<'a> {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        writeln!(fmt, "digraph G {{")?;

        let mut dot = Dot::new(fmt);

        for node in self.0.nodes.iter() {
            node.dot(&mut dot)?;
        }

        for from in self.0.channels.node_indices() {
            for to in self.0.channels.edges_directed(from, Direction::Outgoing) {
                writeln!(
                    fmt,
                    "  channel_{} -> channel_{} [label = {:?}];",
                    from.index(),
                    to.target().index(),
                    to.weight(),
                )?;
            }
        }

        writeln!(fmt, "}}")?;

        Ok(())
    }
}

pub struct NodeMap<'a> {
    graph: &'a mut Graph,
}

impl<'a> NodeMap<'a> {
    fn add<N: Node>(&mut self, node: N) -> <<N as Node>::Port as Port>::Handle {
        let mut map = ChannelMap { graph: self.graph };
        let handle = <<<N as Node>::Port as Port>::Handle as Handle>::new(&mut map);

        struct I<N: Node> {
            handle: <<N as Node>::Port as Port>::Handle,
            node: N,
        }

        impl<N: Node> NodeInstance for I<N> {
            fn spawn(&self, map: &mut PortMap) -> Spawn {
                let port = self.handle.initialize(map);
                self.node.spawn(port)
            }

            fn dot(&self, dot: &mut Dot) -> fmt::Result {
                dot.subgraph::<N, _>(|dot| self.handle.dot(dot))
            }
        }

        self.graph.nodes.push(Box::new(I { handle, node }));

        handle
    }
}

pub trait Template: 'static {
    type Node: Node;

    fn new(self, nodes: &mut NodeMap) -> <<Self::Node as Node>::Port as Port>::Handle;
}

pub trait Node: 'static {
    type Port: Port;

    fn spawn(&self, port: Self::Port) -> Spawn;
}

impl<P: Port> Node for fn(P) -> Spawn {
    type Port = P;

    fn spawn(&self, port: Self::Port) -> Spawn {
        (self)(port)
    }
}

impl<N: Node> Template for N {
    type Node = N;

    fn new(self, nodes: &mut NodeMap) -> <<Self::Node as Node>::Port as Port>::Handle {
        nodes.add(self)
    }
}

pub trait Port: 'static + Sized {
    type Handle: Handle<Port = Self>;
}

pub trait Handle: 'static + Copy {
    type Port: 'static;

    fn new(channels: &mut ChannelMap) -> Self;

    fn initialize(&self, instances: &mut PortMap) -> Self::Port;

    fn dot(&self, dot: &mut Dot) -> fmt::Result {
        let id = dot.nodes;
        dot.node::<Self, _>(format_args!("default_{}", id))?;
        dot.nodes += 1;
        Ok(())
    }
}

#[derive(Debug)]
pub struct PortMap {
    ports: AnyMap,
}

pub struct ChannelMap<'a> {
    graph: &'a mut Graph,
}

impl<'a> ChannelMap<'a> {
    pub fn handle<P: Port>(&mut self) -> P::Handle {
        <<P as Port>::Handle as Handle>::new(self)
    }

    pub fn connect<S>(&mut self, source: S) -> Source<S> {
        Source {
            graph: self.graph,
            source,
        }
    }
}

pub mod channel {
    use super::*;

    macro_rules! handle {
        () => {
            #[derive(Debug)]
            pub struct Handle<M: 'static> {
                pub(crate) id: petgraph::graph::NodeIndex<u32>,
                pub(crate) msg: core::marker::PhantomData<M>,
            }

            impl<M: 'static> Clone for Handle<M> {
                fn clone(&self) -> Self {
                    Self {
                        id: self.id,
                        msg: Default::default(),
                    }
                }
            }

            impl<M: 'static> Copy for Handle<M> {}

            impl<M: 'static> PartialEq for Handle<M> {
                fn eq(&self, other: &Self) -> bool {
                    self.cmp(&other) == core::cmp::Ordering::Equal
                }
            }

            impl<M: 'static> PartialOrd for Handle<M> {
                fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
                    Some(self.cmp(&other))
                }
            }

            impl<M: 'static> Eq for Handle<M> {}

            impl<M: 'static> Ord for Handle<M> {
                fn cmp(&self, other: &Self) -> core::cmp::Ordering {
                    self.id.cmp(&other.id)
                }
            }
        };
    }

    pub mod sender {
        use super::*;

        handle!();

        #[derive(Debug)]
        pub struct Sender<M: 'static> {
            destinations: Map<ChannelID, flume::Sender<(ChannelID, M)>>,
            local_id: ChannelID,
        }

        impl<M: 'static> Port for Sender<M> {
            type Handle = Handle<M>;
        }

        impl<M: 'static> Sender<M> {
            pub fn send_to(&self, destination: ChannelID, message: M) -> Result<(), M> {
                self.destinations
                    .get(&destination)
                    .unwrap()
                    .send((self.local_id, message))
                    .map_err(|err| (err.0).1)
            }

            pub fn broadcast(&self, message: M)
            where
                M: Clone,
            {
                let id = self.local_id;
                for channel in self.destinations.values() {
                    let _ = channel.send((id, message.clone()));
                }
            }
        }

        impl<M: 'static> Connection<receiver::Handle<M>> for Handle<M> {
            fn connect(&self, destination: receiver::Handle<M>, map: &mut ChannelMap) {
                map.graph
                    .channels
                    .add_edge(self.id, destination.id, ChannelEdge::new::<M>());
            }
        }

        impl<M: 'static> super::Handle for Handle<M> {
            type Port = Sender<M>;

            fn new(map: &mut ChannelMap) -> Self {
                fn create_into(_id: ChannelID, _map: &mut PortMap) {
                    // Nothing to create - senders just look up other receivers
                }

                fn resolve_into<M: 'static>(
                    local_id: ChannelID,
                    outgoing: &[ChannelID],
                    instances: &mut PortMap,
                ) {
                    let destinations = outgoing
                        .iter()
                        .copied()
                        .map(|id| {
                            let channel = Pair::resolve(id, instances).0.clone();
                            (id, channel)
                        })
                        .collect();

                    let sender = Sender {
                        destinations,
                        local_id,
                    };

                    instances
                        .ports
                        .entry::<Map<ChannelID, Sender<M>>>()
                        .or_insert_with(Default::default)
                        .insert(local_id, sender);
                }

                let id = map.graph.channels.add_node(ChannelNode {
                    create_into,
                    resolve_into: resolve_into::<M>,
                });

                Handle {
                    id,
                    msg: Default::default(),
                }
            }

            fn initialize(&self, map: &mut PortMap) -> Self::Port {
                map.ports
                    .get_mut::<Map<ChannelID, Sender<M>>>()
                    .expect("missing channel type")
                    .remove(&ChannelID::new(self.id))
                    .expect("missing pair")
            }

            fn dot(&self, dot: &mut Dot) -> fmt::Result {
                dot.node::<Self::Port, _>(format_args!("channel_{}", self.id.index()))?;
                Ok(())
            }
        }
    }

    pub mod receiver {
        use super::*;

        #[derive(Debug)]
        pub struct Receiver<M: 'static> {
            channel: flume::Receiver<(ChannelID, M)>,
        }

        impl<M: 'static> Port for Receiver<M> {
            type Handle = Handle<M>;
        }

        impl<M: 'static> Receiver<M> {
            pub async fn receive_from(&self) -> Option<(ChannelID, M)> {
                self.channel.recv_async().await.ok()
            }

            pub fn try_receive_from(&self) -> Option<(ChannelID, M)> {
                self.channel.try_recv().ok()
            }
        }

        impl<M: 'static> Connection<sender::Handle<M>> for Handle<M> {
            fn connect(&self, source: sender::Handle<M>, map: &mut ChannelMap) {
                map.graph
                    .channels
                    .add_edge(source.id, self.id, ChannelEdge::new::<M>());
            }
        }

        handle!();

        impl<M: 'static> super::Handle for Handle<M> {
            type Port = Receiver<M>;

            fn new(map: &mut ChannelMap) -> Self {
                fn resolve_into(_id: ChannelID, _outgoing: &[ChannelID], _instances: &mut PortMap) {
                    // nothing to do - we just receive from others
                }

                let id = map.graph.channels.add_node(ChannelNode {
                    create_into: <Pair<M>>::create_into,
                    resolve_into,
                });

                Handle {
                    id,
                    msg: Default::default(),
                }
            }

            fn initialize(&self, instances: &mut PortMap) -> Self::Port {
                let channel = <Pair<M>>::resolve(ChannelID::new(self.id), instances)
                    .1
                    .clone();
                Receiver { channel }
            }

            fn dot(&self, dot: &mut Dot) -> fmt::Result {
                dot.node::<Self::Port, _>(format_args!("channel_{}", self.id.index()))?;
                Ok(())
            }
        }
    }

    pub use receiver::Receiver;
    pub use sender::Sender;
}

pub struct Source<'a, S> {
    graph: &'a mut Graph,
    source: S,
}

impl<'a, S> Source<'a, S> {
    pub fn to<Destination>(&mut self, destination: Destination) -> &mut Self
    where
        S: Connection<Destination>,
    {
        self.source
            .connect(destination, &mut ChannelMap { graph: self.graph });
        self
    }
}

pub trait Connection<Destination> {
    fn connect(&self, destination: Destination, map: &mut ChannelMap);
}

pub mod duplex {
    use super::*;

    pub struct Duplex<R: 'static, T: 'static> {
        pub rx: channel::Receiver<R>,
        pub tx: channel::Sender<T>,
    }

    impl<R: 'static, T: 'static> Port for Duplex<R, T> {
        type Handle = Handle<R, T>;
    }

    pub struct Handle<R: 'static, T: 'static> {
        pub rx: channel::receiver::Handle<R>,
        pub tx: channel::sender::Handle<T>,
    }

    impl<R: 'static, T: 'static> Clone for Handle<R, T> {
        fn clone(&self) -> Self {
            Self {
                rx: self.rx,
                tx: self.tx,
            }
        }
    }

    impl<R: 'static, T: 'static> Copy for Handle<R, T> {}

    impl<R: 'static, T: 'static> super::Handle for Handle<R, T> {
        type Port = Duplex<R, T>;

        fn new(map: &mut ChannelMap) -> Self {
            Self {
                rx: map.handle::<channel::Receiver<R>>(),
                tx: map.handle::<channel::Sender<T>>(),
            }
        }

        fn initialize(&self, instances: &mut PortMap) -> Self::Port {
            Duplex {
                rx: self.rx.initialize(instances),
                tx: self.tx.initialize(instances),
            }
        }

        fn dot(&self, dot: &mut Dot) -> fmt::Result {
            dot.subgraph::<Self::Port, _>(|dot| {
                dot.set_label("rx");
                self.rx.dot(dot)?;

                dot.set_label("tx");
                self.tx.dot(dot)?;

                Ok(())
            })
        }
    }

    impl<A: 'static, B: 'static> Connection<Handle<B, A>> for Handle<A, B> {
        fn connect(&self, destination: Handle<B, A>, map: &mut ChannelMap) {
            map.connect(self.rx).to(destination.tx);
            map.connect(self.tx).to(destination.rx);
        }
    }
}
pub use duplex::Duplex;

#[derive(Default)]
pub struct Instance {
    futures: Vec<Spawn>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Server;

    impl Node for Server {
        type Port = Duplex<u16, u32>;

        fn spawn(&self, _port: Self::Port) -> Spawn {
            Box::new(async {
                // TODO
            })
        }
    }

    #[derive(Default)]
    struct Client;

    impl Node for Client {
        type Port = Duplex<u32, u16>;

        fn spawn(&self, _port: Self::Port) -> Spawn {
            Box::new(async {
                // TODO
            })
        }
    }

    #[test]
    fn example() {
        let mut graph = Graph::new();

        let server = graph.add(Server::default());
        let client = graph.add(Client::default());

        graph.connect(server).to(client);

        std::println!("{:?}", graph.dot());

        let _instance = graph.create();
    }
}
