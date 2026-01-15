mod behaviour;
mod outgoing;

use behaviour::Behaviour;
use libp2p_stream::IncomingStreams;

use eyre::Context;
use libp2p::{
    PeerId, Swarm, Transport as _,
    identity::{self, ed25519},
    noise,
    swarm::SwarmEvent,
    tcp, yamux,
};
use multiaddr::Protocol;
use std::{collections::HashMap, time::Duration};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

pub use libp2p::{Multiaddr, StreamProtocol};

/// Commands that can be sent to the p2p node for peer management
#[derive(Debug)]
pub enum NodeCommand {
    /// Dial a peer at the given multiaddress
    DialPeer {
        addr: Multiaddr,
        response: oneshot::Sender<Result<PeerId, String>>,
    },
    /// List all connected peers
    ListPeers {
        response: oneshot::Sender<Vec<PeerInfo>>,
    },
    /// Get node info (peer ID, listen addresses)
    GetInfo {
        response: oneshot::Sender<NodeInfo>,
    },
}

/// Information about a connected peer
#[derive(Debug, Clone, serde::Serialize)]
pub struct PeerInfo {
    pub peer_id: String,
    pub protocols: Vec<String>,
}

/// Information about this node
#[derive(Debug, Clone, serde::Serialize)]
pub struct NodeInfo {
    pub peer_id: String,
    pub listen_addrs: Vec<String>,
    pub connected_peers: usize,
}

/// Handle for sending commands to a running p2p node
#[derive(Clone)]
pub struct NodeHandle {
    command_tx: mpsc::Sender<NodeCommand>,
    peer_id: PeerId,
    listen_addrs: Vec<Multiaddr>,
}

impl NodeHandle {
    /// Get this node's peer ID
    pub fn peer_id(&self) -> PeerId {
        self.peer_id
    }

    /// Get this node's listen addresses with peer ID appended
    pub fn multiaddrs(&self) -> Vec<Multiaddr> {
        self.listen_addrs
            .iter()
            .map(|addr| {
                addr.clone()
                    .with_p2p(self.peer_id)
                    .expect("can add peer ID to multiaddr")
            })
            .collect()
    }

    /// Dial a peer at the given multiaddress
    pub async fn dial_peer(&self, addr: Multiaddr) -> Result<PeerId, String> {
        let (tx, rx) = oneshot::channel();
        self.command_tx
            .send(NodeCommand::DialPeer { addr, response: tx })
            .await
            .map_err(|_| "node command channel closed".to_string())?;
        rx.await.map_err(|_| "response channel closed".to_string())?
    }

    /// List all connected peers
    pub async fn list_peers(&self) -> Result<Vec<PeerInfo>, String> {
        let (tx, rx) = oneshot::channel();
        self.command_tx
            .send(NodeCommand::ListPeers { response: tx })
            .await
            .map_err(|_| "node command channel closed".to_string())?;
        rx.await.map_err(|_| "response channel closed".to_string())
    }

    /// Get node info
    pub async fn get_info(&self) -> Result<NodeInfo, String> {
        let (tx, rx) = oneshot::channel();
        self.command_tx
            .send(NodeCommand::GetInfo { response: tx })
            .await
            .map_err(|_| "node command channel closed".to_string())?;
        rx.await.map_err(|_| "response channel closed".to_string())
    }
}

const DEFAULT_MAX_PEER_COUNT: u32 = 50;

/// A message that can be sent between peers.
pub trait Message:
    serde::Serialize + for<'de> serde::Deserialize<'de> + Send + Sync + Clone + std::fmt::Debug
{
    fn protocol(&self) -> StreamProtocol;

    fn to_string(&self) -> eyre::Result<String> {
        serde_json::to_string(self).wrap_err("failed to serialize message to string")
    }

    fn from_str(s: &str) -> eyre::Result<Self>
    where
        Self: Sized,
    {
        serde_json::from_str(s).wrap_err("failed to deserialize message from string")
    }
}

/// The libp2p node.
///
/// The current behaviour of the node regarding messaging protocols is as follows:
/// - for each supported protocol, the node will accept incoming streams from remote peers on that protocol.
/// - when a new connection is established with a peer, the node will open outbound streams to that peer for each supported protocol.
/// - when a new outgoing message is received on `outgoing_message_rx`, the node will broadcast that message to all connected peers that have an outbound stream open for the message's protocol.
/// - incoming messages received on incoming streams are handled by `IncomingStreamsHandler`, which reads messages from the stream and sends them to a channel for processing by the consumer of this library.
///
/// Currently, there is no gossip implemented; messages are simply broadcast to connected peers.
pub struct Node<M> {
    /// The peer ID of this node.
    peer_id: PeerId,

    /// The multiaddresses this node is listening on.
    listen_addrs: Vec<libp2p::Multiaddr>,

    /// The libp2p swarm, which contains the state of the network
    /// and its behaviours.
    swarm: Swarm<Behaviour>,

    /// The multiaddresses of known peers to connect to on startup.
    known_peers: Vec<Multiaddr>,

    /// Receiver for outgoing messages to be sent to peers.
    outgoing_message_rx: mpsc::Receiver<M>,

    /// Receiver for node commands (peer management).
    command_rx: mpsc::Receiver<NodeCommand>,

    /// Handler for managing outgoing streams to peers.
    /// Used to determine what peers to broadcast to when a
    /// new outgoing message is received on `outgoing_message_rx`.
    outgoing_streams_handler: outgoing::StreamsHandler,

    /// Handlers for incoming streams (streams which remote peers have opened with us).
    incoming_streams_handlers: Vec<IncomingStreamsHandler<M>>,

    /// The protocols this node supports.
    protocols: Vec<StreamProtocol>,

    /// Cancellation token to shut down the node.
    cancellation_token: CancellationToken,
}

impl<M: Message + 'static> Node<M> {
    /// Returns the multiaddresses that this node is listening on, with the peer ID included.
    pub fn multiaddrs(&self) -> Vec<libp2p::Multiaddr> {
        self.listen_addrs
            .iter()
            .map(|addr| {
                addr.clone()
                    .with_p2p(self.peer_id)
                    .expect("can add peer ID to multiaddr")
            })
            .collect()
    }

    /// Runs the p2p node, dials known peers, and starts listening for incoming connections and messages.
    ///
    /// This function will run until the cancellation token is triggered.
    /// If an error occurs, it will be logged, but the node will continue running.
    pub async fn run(self) -> eyre::Result<()> {
        use libp2p::futures::StreamExt as _;

        let Node {
            peer_id,
            listen_addrs,
            mut swarm,
            known_peers,
            mut outgoing_message_rx,
            mut command_rx,
            mut outgoing_streams_handler,
            cancellation_token,
            incoming_streams_handlers,
            protocols,
        } = self;

        // Store listen addrs for later use in GetInfo
        let listen_addrs_clone = listen_addrs.clone();

        for addr in listen_addrs {
            swarm
                .listen_on(addr)
                .wrap_err("swarm failed to listen on multiaddr")?;
        }

        for mut address in known_peers {
            let peer_id = match address.pop() {
                Some(multiaddr::Protocol::P2p(peer_id)) => peer_id,
                _ => {
                    eyre::bail!("no peer ID for known peer");
                }
            };
            swarm.add_peer_address(peer_id, address.clone());
            swarm
                .dial(address)
                .wrap_err("swarm failed to dial known peer")?;
        }

        let handles = incoming_streams_handlers
            .into_iter()
            .map(|handler| tokio::spawn(handler.run()))
            .collect::<Vec<_>>();

        loop {
            tokio::select! {
                biased;
                _ = cancellation_token.cancelled() => {
                    debug!("cancellation token triggered, shutting down node");
                    handles.into_iter().for_each(|h| h.abort());
                    break Ok(());
                }
                Some(command) = command_rx.recv() => {
                    match command {
                        NodeCommand::DialPeer { addr, response } => {
                            let mut addr_clone = addr.clone();
                            let result = match addr_clone.pop() {
                                Some(multiaddr::Protocol::P2p(target_peer_id)) => {
                                    swarm.add_peer_address(target_peer_id, addr_clone.clone());
                                    match swarm.dial(addr_clone) {
                                        Ok(()) => {
                                            info!(target: "p2p", peer = %target_peer_id, "Dialing peer");
                                            Ok(target_peer_id)
                                        }
                                        Err(e) => Err(format!("dial failed: {e}")),
                                    }
                                }
                                _ => Err("multiaddr must end with /p2p/<peer_id>".to_string()),
                            };
                            let _ = response.send(result);
                        }
                        NodeCommand::ListPeers { response } => {
                            let peers: Vec<PeerInfo> = outgoing_streams_handler
                                .connected_peers()
                                .into_iter()
                                .map(|(peer_id, protocols)| PeerInfo {
                                    peer_id: peer_id.to_string(),
                                    protocols: protocols.into_iter().map(|p| p.to_string()).collect(),
                                })
                                .collect();
                            let _ = response.send(peers);
                        }
                        NodeCommand::GetInfo { response } => {
                            let info = NodeInfo {
                                peer_id: peer_id.to_string(),
                                listen_addrs: listen_addrs_clone
                                    .iter()
                                    .map(|a| a.clone().with_p2p(peer_id).unwrap().to_string())
                                    .collect(),
                                connected_peers: outgoing_streams_handler.peer_count(),
                            };
                            let _ = response.send(info);
                        }
                    }
                }
                Some(message) = outgoing_message_rx.recv() => {
                    let protocol = message.protocol();
                    debug!("p2p: received message to broadcast on protocol {protocol}");
                    if let Err(e) = outgoing_streams_handler.broadcast_message(message).await {
                        warn!("p2p: failed to broadcast message on protocol {protocol}: {e:?}");
                    } else {
                        debug!("p2p: successfully broadcast message on protocol {protocol}");
                    }
                }
                event = swarm.select_next_some() => {
                    match event {
                        SwarmEvent::NewListenAddr {
                            address,
                            ..
                        } => {
                            debug!("new listen address: {address}");
                        }
                        SwarmEvent::ExternalAddrConfirmed { address } => {
                            debug!("external address confirmed: {address}");
                        }
                        SwarmEvent::ConnectionEstablished {
                            peer_id,
                            connection_id,
                            ..
                        } => {
                            // when a new connection is established, open outbound streams for each protocol
                            // and add them to the outgoing streams handler.
                            info!(target: "p2p", peer = %peer_id, "Connection established with peer");
                            if !outgoing_streams_handler.has_peer(&peer_id) {
                                debug!("p2p: opening streams for {} protocols", protocols.len());
                                for protocol in &protocols {
                                        match swarm
                                        .behaviour_mut()
                                        .new_control()
                                        .open_stream(peer_id, protocol.clone())
                                        .await
                                    {
                                        Ok(stream) => { outgoing_streams_handler.insert_peer_and_stream(peer_id, protocol.clone(), stream);
                                            info!(target: "p2p", peer = %peer_id, protocol = %protocol, "Opened outbound stream");
                                        }
                                        Err(e) => {
                                            warn!("p2p: failed to open stream with peer {peer_id} on connection {connection_id}: {e:?}");
                                        }
                                    }
                                }
                            } else {
                                debug!("p2p: peer {peer_id} already has streams");
                            }
                        }
                        SwarmEvent::ConnectionClosed {
                            peer_id,
                            cause,
                            ..
                        } => {
                            info!(target: "p2p", peer = %peer_id, "Connection closed: {cause:?}");
                            outgoing_streams_handler.remove_peer(&peer_id);
                        }
                        SwarmEvent::Behaviour(event) => event.handle(&mut swarm),
                        _ => continue,
                    }
                },
            }
        }
    }
}

pub struct NodeBuildResult<M> {
    pub node: Node<M>,
    pub outgoing_message_tx: mpsc::Sender<M>,
    pub incoming_message_rxs: HashMap<StreamProtocol, mpsc::Receiver<M>>,
    /// Handle for peer management operations (dial, list peers, etc.)
    pub node_handle: NodeHandle,
}

pub struct NodeBuilder {
    port: Option<u16>,
    listen_addrs: Vec<libp2p::Multiaddr>,
    keypair_hex: Option<String>,
    known_peers: Vec<Multiaddr>,
    agent_version: Option<String>,
    protocols: Vec<StreamProtocol>,
    max_peer_count: Option<u32>,
    cancellation_token: Option<CancellationToken>,
}

impl Default for NodeBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl NodeBuilder {
    pub fn new() -> Self {
        Self {
            port: None,
            listen_addrs: Vec::new(),
            keypair_hex: None,
            known_peers: Vec::new(),
            agent_version: None,
            protocols: Vec::new(),
            max_peer_count: None,
            cancellation_token: None,
        }
    }

    pub fn with_port(mut self, port: u16) -> Self {
        self.port = Some(port);
        self
    }

    pub fn with_listen_addr(mut self, addr: libp2p::Multiaddr) -> Self {
        self.listen_addrs.push(addr);
        self
    }

    pub fn with_keypair_hex_string(mut self, keypair_hex: String) -> Self {
        self.keypair_hex = Some(keypair_hex);
        self
    }

    pub fn with_agent_version(mut self, agent_version: String) -> Self {
        self.agent_version = Some(agent_version);
        self
    }

    pub fn with_protocol(mut self, protocol: StreamProtocol) -> Self {
        self.protocols.push(protocol);
        self
    }

    pub fn with_cancellation_token(
        mut self,
        cancellation_token: tokio_util::sync::CancellationToken,
    ) -> Self {
        self.cancellation_token = Some(cancellation_token);
        self
    }

    pub fn with_max_peer_count(mut self, max_peer_count: u32) -> Self {
        self.max_peer_count = Some(max_peer_count);
        self
    }

    pub fn with_known_peers<I, T>(mut self, addresses: I) -> Self
    where
        I: IntoIterator<Item = T>,
        T: Into<Multiaddr>,
    {
        for address in addresses {
            self.known_peers.push(address.into());
        }
        self
    }

    pub fn try_build<M: Message + 'static>(self) -> eyre::Result<NodeBuildResult<M>> {
        let Self {
            port,
            listen_addrs,
            keypair_hex,
            known_peers,
            agent_version,
            protocols,
            max_peer_count,
            cancellation_token,
        } = self;

        // TODO: caller should be forced to provide this
        let cancellation_token = cancellation_token.unwrap_or_default();

        let Some(agent_version) = agent_version else {
            eyre::bail!("agent version must be set");
        };

        let keypair = match keypair_hex {
            Some(hex) => {
                let bytes = hex::decode(hex).wrap_err("failed to decode hex string")?;
                // Support both 32-byte secret key and 64-byte full keypair
                let keypair = if bytes.len() == 32 {
                    // 32 bytes = secret key only, derive keypair
                    let secret = ed25519::SecretKey::try_from_bytes(bytes)
                        .wrap_err("failed to create secret key from bytes")?;
                    ed25519::Keypair::from(secret)
                } else if bytes.len() == 64 {
                    // 64 bytes = full keypair (secret || public)
                    let mut keypair_bytes = bytes;
                    ed25519::Keypair::try_from_bytes(&mut keypair_bytes)
                        .wrap_err("failed to create keypair from bytes")?
                } else {
                    eyre::bail!(
                        "Invalid keypair length: expected 32 (secret) or 64 (keypair) bytes, got {}",
                        bytes.len()
                    );
                };
                Some(keypair.into())
            }
            None => None,
        };
        let keypair = keypair.unwrap_or(identity::Keypair::generate_ed25519());
        let peer_id = keypair.public().to_peer_id();

        let transport = create_transport(&keypair).wrap_err("failed to create transport")?;
        let max_peer_count = max_peer_count.unwrap_or(DEFAULT_MAX_PEER_COUNT);
        let mut behaviour = Behaviour::new(&keypair, agent_version, max_peer_count)
            .context("failed to create behaviour")?;
        let mut control = behaviour.new_control();

        let mut incoming_streams_handlers = Vec::new();
        let mut incoming_message_rxs = HashMap::new();
        for protocol in &protocols {
            let incoming_streams = control
                .accept(protocol.clone())
                .wrap_err("failed to subscribe to incoming streams for flashblocks protocol")?;
            let (incoming_streams_handler, message_rx) = IncomingStreamsHandler::new(
                protocol.clone(),
                incoming_streams,
                cancellation_token.clone(),
            );
            incoming_streams_handlers.push(incoming_streams_handler);
            incoming_message_rxs.insert(protocol.clone(), message_rx);
        }

        let swarm = libp2p::SwarmBuilder::with_existing_identity(keypair)
            .with_tokio()
            .with_other_transport(|_| transport)?
            .with_behaviour(|_| behaviour)?
            .with_swarm_config(|cfg| {
                cfg.with_idle_connection_timeout(Duration::from_secs(u64::MAX)) // don't disconnect from idle peers
            })
            .build();

        // disallow providing listen addresses that have a peer ID in them,
        // as we've specified the peer ID for this node above.
        let mut listen_addrs: Vec<Multiaddr> = listen_addrs
            .into_iter()
            .filter(|addr| {
                for protocol in addr.iter() {
                    if protocol == Protocol::P2p(peer_id) {
                        return false;
                    }
                }
                true
            })
            .collect();
        if listen_addrs.is_empty() {
            let port = port.unwrap_or(0);
            let listen_addr = format!("/ip4/0.0.0.0/tcp/{port}")
                .parse()
                .expect("can parse valid multiaddr");
            listen_addrs.push(listen_addr);
        }

        let (outgoing_message_tx, outgoing_message_rx) = tokio::sync::mpsc::channel(100);
        let (command_tx, command_rx) = tokio::sync::mpsc::channel(32);

        // Create the node handle for peer management
        let node_handle = NodeHandle {
            command_tx,
            peer_id,
            listen_addrs: listen_addrs.clone(),
        };

        Ok(NodeBuildResult {
            node: Node {
                peer_id,
                swarm,
                listen_addrs,
                known_peers,
                outgoing_message_rx,
                command_rx,
                outgoing_streams_handler: outgoing::StreamsHandler::new(),
                cancellation_token,
                incoming_streams_handlers,
                protocols,
            },
            outgoing_message_tx,
            incoming_message_rxs,
            node_handle,
        })
    }
}

struct IncomingStreamsHandler<M> {
    protocol: StreamProtocol,
    incoming: IncomingStreams,
    tx: mpsc::Sender<M>,
    cancellation_token: CancellationToken,
}

impl<M: Message + 'static> IncomingStreamsHandler<M> {
    fn new(
        protocol: StreamProtocol,
        incoming: IncomingStreams,
        cancellation_token: CancellationToken,
    ) -> (Self, mpsc::Receiver<M>) {
        const CHANNEL_SIZE: usize = 100;
        let (tx, rx) = mpsc::channel(CHANNEL_SIZE);
        (
            Self {
                protocol,
                incoming,
                tx,
                cancellation_token,
            },
            rx,
        )
    }

    async fn run(self) {
        use futures::StreamExt as _;

        let Self {
            protocol,
            mut incoming,
            tx,
            cancellation_token,
        } = self;
        let mut handle_stream_futures = futures::stream::FuturesUnordered::new();

        loop {
            tokio::select! {
                _ = cancellation_token.cancelled() => {
                    debug!("cancellation token triggered, shutting down incoming streams handler for protocol {protocol}");
                    return;
                }
                Some((from, stream)) = incoming.next() => {
                    debug!("new incoming stream on protocol {protocol} from peer {from}");
                    handle_stream_futures.push(tokio::spawn(handle_incoming_stream(from, stream, tx.clone())));
                }
                Some(res) = handle_stream_futures.next() => {
                    match res {
                        Ok(Ok(())) => {}
                        Ok(Err(e)) => {
                            warn!("error handling incoming stream: {e:?}");
                        }
                        Err(e) => {
                            warn!("task handling incoming stream panicked: {e:?}");
                        }
                    }
                }
            }
        }
    }
}

async fn handle_incoming_stream<M: Message>(
    peer_id: PeerId,
    stream: libp2p::Stream,
    payload_tx: mpsc::Sender<M>,
) -> eyre::Result<()> {
    use futures::StreamExt as _;
    use tokio_util::{
        codec::{FramedRead, LinesCodec},
        compat::FuturesAsyncReadCompatExt as _,
    };

    let codec = LinesCodec::new();
    let mut reader = FramedRead::new(stream.compat(), codec);

    while let Some(res) = reader.next().await {
        match res {
            Ok(str) => {
                let payload = M::from_str(&str).wrap_err("failed to decode stream message")?;
                debug!("got message from peer {peer_id}: {payload:?}");
                let _ = payload_tx.send(payload).await;
            }
            Err(e) => {
                return Err(e).wrap_err(format!("failed to read from stream of peer {peer_id}"));
            }
        }
    }

    Ok(())
}

fn create_transport(
    keypair: &identity::Keypair,
) -> eyre::Result<libp2p::core::transport::Boxed<(PeerId, libp2p::core::muxing::StreamMuxerBox)>> {
    let transport = tcp::tokio::Transport::new(tcp::Config::default())
        .upgrade(libp2p::core::upgrade::Version::V1)
        .authenticate(noise::Config::new(keypair)?)
        .multiplex(yamux::Config::default())
        .timeout(Duration::from_secs(20))
        .boxed();

    Ok(transport)
}

#[cfg(test)]
mod test {
    use super::*;

    const TEST_AGENT_VERSION: &str = "test/1.0.0";
    const TEST_PROTOCOL: StreamProtocol = StreamProtocol::new("/test/1.0.0");

    #[derive(Debug, PartialEq, Eq, Clone)]
    struct TestMessage {
        content: String,
    }

    impl Message for TestMessage {
        fn protocol(&self) -> StreamProtocol {
            TEST_PROTOCOL
        }
    }

    impl serde::Serialize for TestMessage {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: serde::Serializer,
        {
            serializer.serialize_str(&self.content)
        }
    }

    impl<'de> serde::Deserialize<'de> for TestMessage {
        fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: serde::Deserializer<'de>,
        {
            let s = String::deserialize(deserializer)?;
            Ok(TestMessage { content: s })
        }
    }

    #[tokio::test]
    async fn two_nodes_can_connect_and_message() {
        let NodeBuildResult {
            node: node1,
            outgoing_message_tx: _,
            incoming_message_rxs: mut rx1,
        } = NodeBuilder::new()
            .with_listen_addr("/ip4/127.0.0.1/tcp/9000".parse().unwrap())
            .with_agent_version(TEST_AGENT_VERSION.to_string())
            .with_protocol(TEST_PROTOCOL)
            .try_build::<TestMessage>()
            .unwrap();
        let NodeBuildResult {
            node: node2,
            outgoing_message_tx: tx2,
            incoming_message_rxs: _,
        } = NodeBuilder::new()
            .with_known_peers(node1.multiaddrs())
            .with_protocol(TEST_PROTOCOL)
            .with_listen_addr("/ip4/127.0.0.1/tcp/9001".parse().unwrap())
            .with_agent_version(TEST_AGENT_VERSION.to_string())
            .try_build::<TestMessage>()
            .unwrap();

        tokio::spawn(async move { node1.run().await });
        tokio::spawn(async move { node2.run().await });
        // sleep to allow nodes to connect
        tokio::time::sleep(Duration::from_secs(2)).await;

        let message = TestMessage {
            content: "message".to_string(),
        };
        tx2.send(message.clone()).await.unwrap();

        let recv_message: TestMessage = rx1.remove(&TEST_PROTOCOL).unwrap().recv().await.unwrap();
        assert_eq!(recv_message, message);
    }
}
