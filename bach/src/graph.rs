use alloc::{boxed::Box, vec::Vec};
use anymap::AnyMap;
use core::{fmt, future::Future, pin::Pin, task};
use idmap::IdMap as Map;
use petgraph::{
    graph::{DiGraph, NodeIndex},
    visit::EdgeRef,
    Direction,
};

pub mod viz;

pub struct Spawn(Pin<Box<dyn 'static + Future<Output = ()> + Send>>);

impl Spawn {
    pub fn poll(&mut self, cx: &mut task::Context) -> task::Poll<()> {
        self.0.as_mut().poll(cx)
    }
}

impl<F: 'static + Future<Output = ()> + Send> From<F> for Spawn {
    fn from(f: F) -> Self {
        Self(Box::pin(f))
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, PartialOrd, Eq, Ord)]
pub(crate) struct ChannelId(u32);

impl ChannelId {
    fn new(idx: NodeIndex<u32>) -> Self {
        Self(idx.index() as _)
    }

    fn as_index(self) -> NodeIndex<u32> {
        NodeIndex::new(self.0 as _)
    }
}

impl idmap::IntegerId for ChannelId {
    fn from_id(id: u64) -> Self {
        Self(id as _)
    }

    fn id(&self) -> u64 {
        self.0 as _
    }

    fn id32(&self) -> u32 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, PartialOrd, Eq, Ord)]
pub struct NodeId(NodeIdx);

type NodeIdx = u32;

#[derive(Clone, Copy, Debug, Default)]
pub struct Message<Body> {
    pub source: NodeId,
    pub destination: NodeId,
    pub body: Body,
}

struct Pair<M>(flume::Sender<Message<M>>, flume::Receiver<Message<M>>);

impl<M> Default for Pair<M> {
    fn default() -> Self {
        let (sender, receiver) = flume::unbounded();
        Self(sender, receiver)
    }
}

impl<M: 'static> Pair<M> {
    fn create_into(id: ChannelId, map: &mut PortMap) {
        map.ports
            .entry::<Map<ChannelId, Self>>()
            .or_insert_with(Default::default)
            .insert(id, Pair::default());
    }

    fn resolve(id: ChannelId, map: &PortMap) -> &Self {
        map.ports
            .get::<Map<ChannelId, Self>>()
            .expect("missing channel type")
            .get(&id)
            .expect("missing pair")
    }
}

struct ChannelNode {
    // The Node that owns this particular channel
    node_id: NodeId,
    // creates a channel into the channels map
    create_into: fn(id: ChannelId, channels: &mut PortMap),
    // initializes a channel instance from the channel information
    resolve_into: fn(node_id: NodeId, id: ChannelId, graph: &ChannelGraph, instances: &mut PortMap),
}

impl fmt::Debug for ChannelNode {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Channel")
    }
}

#[derive(Clone, Copy)]
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
    fn run(&self, map: &mut PortMap) -> Spawn;

    fn viz(&self, f: &mut viz::Viz) -> fmt::Result;
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

    pub fn insert<P: Port, T: Template<P>>(&mut self, template: T) -> P::Handle {
        let id = NodeId(self.nodes.len() as _);
        let mut nodes = NodeMap { graph: self, id };
        template.apply(&mut nodes)
    }

    pub fn connect<S>(&mut self, source: S) -> Source<S> {
        Source {
            graph: self,
            source,
        }
    }

    pub fn dot(&self) -> Viz {
        Viz(self)
    }

    pub fn create(&self) -> Instance {
        let mut map = PortMap {
            ports: AnyMap::new(),
        };

        for idx in self.channels.node_indices() {
            let channel = &self.channels[idx];
            let id = ChannelId::new(idx);
            (channel.create_into)(id, &mut map);
        }

        for idx in self.channels.node_indices() {
            let channel = &self.channels[idx];
            let node_id = channel.node_id;
            let id = ChannelId::new(idx);
            (channel.resolve_into)(node_id, id, &self.channels, &mut map);
        }

        let mut instance = Instance::default();

        for node in &self.nodes {
            instance.futures.push(node.run(&mut map));
        }

        instance
    }
}

pub struct Viz<'a>(&'a Graph);

impl<'a> fmt::Debug for Viz<'a> {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        write!(fmt, "{}", self)
    }
}

impl<'a> fmt::Display for Viz<'a> {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        writeln!(fmt, "digraph G {{")?;

        let mut viz = viz::Viz::new(fmt);

        for node in self.0.nodes.iter() {
            node.viz(&mut viz)?;
        }

        for from in self.0.channels.node_indices() {
            for to in self.0.channels.edges_directed(from, Direction::Outgoing) {
                if !to.weight().0.is_empty() {
                    writeln!(
                        fmt,
                        "  channel_{} -> channel_{} [label = {:?}];",
                        from.index(),
                        to.target().index(),
                        to.weight(),
                    )?;
                }
            }
        }

        writeln!(fmt, "}}")?;

        Ok(())
    }
}

pub struct NodeMap<'a> {
    graph: &'a mut Graph,
    id: NodeId,
}

impl<'a> NodeMap<'a> {
    fn insert<P: Port, N: Node<P>>(&mut self, node: N) -> P::Handle {
        let mut map = ChannelMap {
            graph: self.graph,
            id: self.id,
        };
        let handle = <P::Handle as Handle>::new(&mut map);

        struct I<P: Port, N: Node<P>> {
            handle: P::Handle,
            node: N,
        }

        impl<P: Port, N: Node<P>> NodeInstance for I<P, N> {
            fn run(&self, map: &mut PortMap) -> Spawn {
                let port = self.handle.initialize(map);
                self.node.run(port)
            }

            fn viz(&self, viz: &mut viz::Viz) -> fmt::Result {
                viz.subgraph::<N, _>(|v| self.handle.viz(v))
            }
        }

        self.graph.nodes.push(Box::new(I { handle, node }));

        handle
    }
}

pub trait Template<P: Port>: 'static {
    type Node: Node<P>;

    fn apply(self, nodes: &mut NodeMap) -> P::Handle;
}

pub trait Node<P: Port>: 'static {
    fn run(&self, port: P) -> Spawn;
}

impl<P, F, R> Node<P> for F
where
    P: Port,
    F: 'static + Fn(P) -> R,
    R: 'static + Future<Output = ()> + Send,
{
    fn run(&self, port: P) -> Spawn {
        (self)(port).into()
    }
}

impl<P: Port, N: Node<P>> Template<P> for N {
    type Node = N;

    fn apply(self, nodes: &mut NodeMap) -> P::Handle {
        nodes.insert(self)
    }
}

pub trait Port: 'static + Sized {
    type Handle: Handle<Port = Self>;
}

pub trait Handle: 'static + Copy {
    type Port: 'static + Port<Handle = Self>;

    fn new(channels: &mut ChannelMap) -> Self;

    fn initialize(&self, instances: &mut PortMap) -> Self::Port;

    fn node_id(&self) -> NodeId;

    fn viz(&self, viz: &mut viz::Viz) -> fmt::Result {
        let id = viz.nodes;
        viz.node::<Self, _>(format_args!("default_{}", id))?;
        viz.nodes += 1;
        Ok(())
    }
}

#[derive(Debug)]
pub struct PortMap {
    ports: AnyMap,
}

pub struct ChannelMap<'a> {
    graph: &'a mut Graph,
    id: NodeId,
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

pub struct ConnectionMap<'a> {
    graph: &'a mut Graph,
}

impl<'a> ConnectionMap<'a> {
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
                pub(crate) node_id: NodeId,
                pub(crate) channel_id: ChannelId,
                pub(crate) msg: core::marker::PhantomData<M>,
            }

            impl<M: 'static> Handle<M> {
                pub fn node_id(&self) -> NodeId {
                    self.node_id
                }
            }

            impl<M: 'static> Clone for Handle<M> {
                fn clone(&self) -> Self {
                    Self {
                        node_id: self.node_id,
                        channel_id: self.channel_id,
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
                    self.channel_id.cmp(&other.channel_id)
                }
            }
        };
    }

    pub mod sender {
        use super::*;

        handle!();

        type ChannelSender<M> = flume::Sender<Message<M>>;

        #[derive(Debug)]
        pub struct Sender<M: 'static> {
            destinations: Map<NodeIdx, ChannelSender<M>>,
            local_id: NodeId,
        }

        impl<M: 'static> Port for Sender<M> {
            type Handle = Handle<M>;
        }

        impl<M: 'static> Sender<M> {
            pub fn forward(&self, message: Message<M>) -> Result<(), Message<M>> {
                self.destinations
                    .get(&message.destination.0)
                    .expect("destination unreachable")
                    .send(message)
                    .map_err(|err| err.0)
            }

            pub fn send(&self, destination: NodeId, body: M) -> Result<(), M> {
                self.destinations
                    .get(&destination.0)
                    .expect("destination unreachable")
                    .send(Message {
                        source: self.local_id,
                        destination,
                        body,
                    })
                    .map_err(|err| err.0.body)
            }

            pub fn broadcast(&self, body: M)
            where
                M: Clone,
            {
                for (destination, sender) in self.destinations.iter() {
                    let _ = sender.send(Message {
                        source: self.local_id,
                        destination: NodeId(*destination),
                        body: body.clone(),
                    });
                }
            }
        }

        impl<M: 'static> Connection<receiver::Handle<M>> for Handle<M> {
            fn connect(&self, destination: receiver::Handle<M>, map: &mut ConnectionMap) {
                map.graph.channels.add_edge(
                    self.channel_id.as_index(),
                    destination.channel_id.as_index(),
                    ChannelEdge::new::<M>(),
                );
            }
        }

        impl<M: 'static> super::Handle for Handle<M> {
            type Port = Sender<M>;

            fn new(map: &mut ChannelMap) -> Self {
                fn create_into(_id: ChannelId, _map: &mut PortMap) {
                    // Nothing to create - senders just look up other receivers
                }

                fn resolve_into<M: 'static>(
                    node_id: NodeId,
                    channel_id: ChannelId,
                    graph: &ChannelGraph,
                    instances: &mut PortMap,
                ) {
                    use petgraph::algo::dijkstra;

                    let mut destinations: Map<NodeIdx, ChannelSender<M>> = Default::default();

                    let mut direct_neighbors = graph
                        .neighbors_directed(channel_id.as_index(), Direction::Outgoing)
                        .detach();

                    while let Some(neighbor_channel) =
                        direct_neighbors.next_node(graph).map(ChannelId::new)
                    {
                        let sender = &<Pair<M>>::resolve(neighbor_channel, instances).0;
                        let neighbor_node = graph[neighbor_channel.as_index()].node_id;
                        destinations.insert(neighbor_node.0, sender.clone());

                        let connected_channels =
                            dijkstra(graph, neighbor_channel.as_index(), None, |_edge| 1);

                        for (channel_idx, weight) in connected_channels {
                            if weight != 0 {
                                let node_id = graph[channel_idx].node_id;
                                destinations.insert(node_id.0, sender.clone());
                            }
                        }
                    }

                    let sender = Sender {
                        destinations,
                        local_id: node_id,
                    };

                    instances
                        .ports
                        .entry::<Map<ChannelId, Sender<M>>>()
                        .or_insert_with(Default::default)
                        .insert(channel_id, sender);
                }

                let idx = map.graph.channels.add_node(ChannelNode {
                    node_id: map.id,
                    create_into,
                    resolve_into: resolve_into::<M>,
                });

                Handle {
                    channel_id: ChannelId::new(idx),
                    node_id: map.id,
                    msg: Default::default(),
                }
            }

            fn initialize(&self, map: &mut PortMap) -> Self::Port {
                map.ports
                    .get_mut::<Map<ChannelId, Sender<M>>>()
                    .expect("missing channel type")
                    .remove(&self.channel_id)
                    .expect("missing pair")
            }

            fn node_id(&self) -> NodeId {
                self.node_id
            }

            fn viz(&self, viz: &mut viz::Viz) -> fmt::Result {
                viz.node::<Self::Port, _>(format_args!("channel_{}", self.channel_id.0))?;
                Ok(())
            }
        }
    }

    pub mod receiver {
        use super::*;

        #[derive(Debug)]
        pub struct Receiver<M: 'static> {
            channel: flume::Receiver<Message<M>>,
        }

        impl<M: 'static> Port for Receiver<M> {
            type Handle = Handle<M>;
        }

        impl<M: 'static> Receiver<M> {
            pub async fn receive(&self) -> Option<Message<M>> {
                self.channel.recv_async().await.ok()
            }

            pub fn poll_receive(
                &self,
                cx: &mut core::task::Context,
            ) -> core::task::Poll<Option<Message<M>>> {
                use core::task::Poll;
                let mut r = self.channel.recv_async();
                let fut = Pin::new(&mut r);
                match Future::poll(fut, cx) {
                    Poll::Pending => Poll::Pending,
                    Poll::Ready(Ok(m)) => Poll::Ready(Some(m)),
                    Poll::Ready(Err(_)) => Poll::Ready(None),
                }
            }

            pub fn try_receive(&self) -> Option<Message<M>> {
                self.channel.try_recv().ok()
            }
        }

        impl<M: 'static> Connection<sender::Handle<M>> for Handle<M> {
            fn connect(&self, source: sender::Handle<M>, map: &mut ConnectionMap) {
                map.graph.channels.add_edge(
                    source.channel_id.as_index(),
                    self.channel_id.as_index(),
                    ChannelEdge::new::<M>(),
                );
            }
        }

        handle!();

        impl<M: 'static> super::Handle for Handle<M> {
            type Port = Receiver<M>;

            fn new(map: &mut ChannelMap) -> Self {
                fn resolve_into(
                    _node: NodeId,
                    _id: ChannelId,
                    _graph: &ChannelGraph,
                    _instances: &mut PortMap,
                ) {
                    // nothing to do - we just receive from others
                }

                let idx = map.graph.channels.add_node(ChannelNode {
                    node_id: map.id,
                    create_into: <Pair<M>>::create_into,
                    resolve_into,
                });

                Handle {
                    channel_id: ChannelId::new(idx),
                    node_id: map.id,
                    msg: Default::default(),
                }
            }

            fn initialize(&self, instances: &mut PortMap) -> Self::Port {
                let channel = <Pair<M>>::resolve(self.channel_id, instances).1.clone();
                Receiver { channel }
            }

            fn node_id(&self) -> NodeId {
                self.node_id
            }

            fn viz(&self, viz: &mut viz::Viz) -> fmt::Result {
                viz.node::<Self::Port, _>(format_args!("channel_{}", self.channel_id.0))?;
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
            .connect(destination, &mut ConnectionMap { graph: self.graph });
        self
    }
}

pub trait Connection<Destination> {
    fn connect(&self, destination: Destination, map: &mut ConnectionMap);
}

pub mod duplex {
    use super::*;

    pub struct Duplex<R: 'static, T: 'static> {
        pub rx: channel::Receiver<R>,
        pub tx: channel::Sender<T>,
    }

    impl<R: 'static, T: 'static> Duplex<R, T> {
        pub fn forward(&self, message: Message<T>) -> Result<(), Message<T>> {
            self.tx.forward(message)
        }

        pub fn send(&self, destination: NodeId, body: T) -> Result<(), T> {
            self.tx.send(destination, body)
        }

        pub async fn receive(&self) -> Option<Message<R>> {
            self.rx.receive().await
        }

        pub fn try_receive(&self) -> Option<Message<R>> {
            self.rx.try_receive()
        }
    }

    impl<R: 'static, T: 'static> Port for Duplex<R, T> {
        type Handle = Handle<R, T>;
    }

    pub struct Handle<R: 'static, T: 'static> {
        pub rx: channel::receiver::Handle<R>,
        pub tx: channel::sender::Handle<T>,
    }

    impl<R: 'static, T: 'static> Handle<R, T> {
        pub fn node_id(&self) -> NodeId {
            self.rx.node_id()
        }
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
            let rx = map.handle::<channel::Receiver<R>>();
            let tx = map.handle::<channel::Sender<T>>();

            // weakly connect these two together
            map.graph.channels.add_edge(
                rx.channel_id.as_index(),
                tx.channel_id.as_index(),
                ChannelEdge(""),
            );

            Self { rx, tx }
        }

        fn initialize(&self, instances: &mut PortMap) -> Self::Port {
            Duplex {
                rx: self.rx.initialize(instances),
                tx: self.tx.initialize(instances),
            }
        }

        fn node_id(&self) -> NodeId {
            self.rx.node_id()
        }

        fn viz(&self, viz: &mut viz::Viz) -> fmt::Result {
            viz.subgraph::<Self::Port, _>(|viz| {
                viz.set_label("rx");
                self.rx.viz(viz)?;

                viz.set_label("tx");
                self.tx.viz(viz)?;

                Ok(())
            })
        }
    }

    impl<A: 'static, B: 'static> Connection<Handle<B, A>> for Handle<A, B> {
        fn connect(&self, destination: Handle<B, A>, map: &mut ConnectionMap) {
            map.connect(self.rx).to(destination.tx);
            map.connect(self.tx).to(destination.rx);
        }
    }
}
pub use duplex::Duplex;

mod tuple {
    use super::*;

    impl<R: Port, T: Port> Port for (R, T) {
        type Handle = (R::Handle, T::Handle);
    }

    impl<R: Handle, T: Handle> Handle for (R, T) {
        type Port = (R::Port, T::Port);

        fn new(map: &mut ChannelMap) -> Self {
            (map.handle::<R::Port>(), map.handle::<T::Port>())
        }

        fn initialize(&self, instances: &mut PortMap) -> Self::Port {
            (self.0.initialize(instances), self.1.initialize(instances))
        }

        fn node_id(&self) -> NodeId {
            self.0.node_id()
        }

        fn viz(&self, viz: &mut viz::Viz) -> fmt::Result {
            viz.subgraph::<Self::Port, _>(|viz| {
                viz.set_label("0");
                self.0.viz(viz)?;

                viz.set_label("1");
                self.1.viz(viz)?;

                Ok(())
            })
        }
    }
}

#[derive(Default)]
pub struct Instance {
    futures: Vec<Spawn>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::sync::Arc;
    use core::sync::atomic::{AtomicBool, Ordering};

    #[derive(Default)]
    struct Server;

    impl Node<Duplex<u16, u32>> for Server {
        fn run(&self, port: Duplex<u16, u32>) -> Spawn {
            async move {
                while let Some(msg) = port.receive().await {
                    port.send(msg.source, 456).unwrap();
                }
            }
            .into()
        }
    }

    #[derive(Clone)]
    struct Client {
        server: NodeId,
        finished: Arc<AtomicBool>,
    }

    impl Client {
        pub fn new(server: NodeId, finished: Arc<AtomicBool>) -> Self {
            Self { server, finished }
        }
    }

    impl Node<Duplex<u32, u16>> for Client {
        fn run(&self, port: Duplex<u32, u16>) -> Spawn {
            let server_id = self.server;
            let finished = self.finished.clone();
            async move {
                port.send(server_id, 123).unwrap();
                let msg = port.receive().await.unwrap();
                assert_eq!(msg.body, 456);
                finished.store(true, Ordering::SeqCst);
            }
            .into()
        }
    }

    fn test_helper<F: FnOnce(&mut Graph, Arc<AtomicBool>)>(f: F) {
        let mut graph = Graph::new();

        let finished = Arc::new(AtomicBool::new(false));

        f(&mut graph, finished.clone());

        let instance = graph.create();

        let mut executor = crate::executor::tests::executor();
        for f in instance.futures {
            executor.spawn(f.0);
        }

        executor.block();

        assert!(
            finished.load(Ordering::SeqCst),
            "finished should be set to true at the end of the test"
        );
    }

    #[test]
    fn client_server_test() {
        test_helper(|graph, finished| {
            let server = graph.insert(Server::default());
            let client = graph.insert(Client::new(server.node_id(), finished));

            graph.connect(server).to(client);
        });
    }

    #[test]
    fn middleboxes_test() {
        test_helper(|graph, finished| {
            let server = graph.insert(Server::default());
            let middlebox_a = graph.insert(|duplex: Duplex<_, _>| async move {
                while let Some(msg) = duplex.receive().await {
                    duplex.forward(msg).unwrap();
                }
            });
            let middlebox_b = graph.insert(|duplex: Duplex<_, _>| async move {
                while let Some(msg) = duplex.receive().await {
                    duplex.forward(msg).unwrap();
                }
            });
            let client = graph.insert(Client::new(server.node_id(), finished));

            graph.connect(server.tx).to(middlebox_a.rx);
            graph.connect(middlebox_a.tx).to(client.rx);

            graph.connect(client.tx).to(middlebox_b.rx);
            graph.connect(middlebox_b.tx).to(server.rx);
        });
    }
}
