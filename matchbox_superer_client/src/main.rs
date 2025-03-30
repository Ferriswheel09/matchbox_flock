use futures::{select, FutureExt};
use futures_timer::Delay;
use log::info;
use matchbox_socket::{PeerId, PeerState, WebRtcSocket, WebRtcSocketBuilder};
use std::collections::{HashMap, HashSet};
use std::time::Duration;

const CHANNEL_ID: usize = 0;
const HANDSHAKE_RETRY_INTERVAL_MS: u64 = 1000; // Retry every 1 second
const MAX_HANDSHAKE_ATTEMPTS: u8 = 5; // Maximum retry attempts

#[derive(Debug)]
enum PeerRole {
    Super,
    Child,
}

struct HandshakeState {
    attempts: u8,
    last_attempt: std::time::Instant,
    acknowledged: bool,
}

impl HandshakeState {
    fn new() -> Self {
        Self {
            attempts: 0,
            last_attempt: std::time::Instant::now(),
            acknowledged: false,
        }
    }

    fn should_retry(&self) -> bool {
        !self.acknowledged
            && self.attempts < MAX_HANDSHAKE_ATTEMPTS
            && self.last_attempt.elapsed() >= Duration::from_millis(HANDSHAKE_RETRY_INTERVAL_MS)
    }

    fn record_attempt(&mut self) {
        self.attempts += 1;
        self.last_attempt = std::time::Instant::now();
    }

    fn acknowledge(&mut self) {
        self.acknowledged = true;
    }
}

struct NetworkState {
    super_peers: HashMap<PeerId, String>, // Map peer id to any additional info
    child_peers: HashMap<PeerId, String>,
    peer_handshake_states: HashMap<PeerId, HandshakeState>,
}

impl NetworkState {
    fn new() -> Self {
        Self {
            super_peers: HashMap::new(),
            child_peers: HashMap::new(),
            peer_handshake_states: HashMap::new(),
        }
    }

    fn add_super_peer(&mut self, peer: PeerId, info: String) {
        self.super_peers.insert(peer, info);
    }

    fn add_child_peer(&mut self, peer: PeerId, info: String) {
        self.child_peers.insert(peer, info);
    }

    fn remove_peer(&mut self, peer: PeerId) {
        self.super_peers.remove(&peer);
        self.child_peers.remove(&peer);
        self.peer_handshake_states.remove(&peer);
    }

    fn print_state(&self) {
        info!(
            "Network State -> Super Peers: {:?} | Child Peers: {:?}",
            self.super_peers, self.child_peers
        );
    }

    fn get_peers_to_retry(&self) -> Vec<PeerId> {
        self.peer_handshake_states
            .iter()
            .filter(|(_, state)| state.should_retry())
            .map(|(peer_id, _)| *peer_id)
            .collect()
    }
}

#[cfg(target_arch = "wasm32")]
fn main() {
    // Setup logging for wasm
    console_error_panic_hook::set_once();
    console_log::init_with_level(log::Level::Debug).unwrap();
    wasm_bindgen_futures::spawn_local(async_main());
}

#[cfg(not(target_arch = "wasm32"))]
#[tokio::main]
async fn main() {
    // Setup logging for native
    use tracing_subscriber::prelude::*;
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "simple_example=info,matchbox_socket=info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    async_main().await
}

async fn async_main() {
    info!("Connecting to matchbox");

    // Build the socket (optionally, add your TURN/stun config here)
    let (mut socket, loop_fut) = WebRtcSocketBuilder::new("ws://localhost:3536/")
        .add_reliable_channel()
        .build();

    let loop_fut = loop_fut.fuse();
    futures::pin_mut!(loop_fut);

    // Track whether the socket has determined its role.
    let mut initialized = false;
    // Create a network state instance to track connected peers.
    let mut network_state = NetworkState::new();

    // Store the parent peer ID when we're a child.
    let mut parent_id: Option<PeerId> = None;
    // Store our own role.
    let mut self_role: Option<PeerRole> = None;

    // Used for maintaining the loop timeout.
    let timeout = Delay::new(Duration::from_millis(100));
    futures::pin_mut!(timeout);

    // Cache of messages to prevent duplicate forwarding.
    let mut seen_messages: HashSet<String> = HashSet::new();

    loop {
        // Determine our own role once on start.
        if !initialized {
            if let Some(super_peer) = socket.super_peer() {
                info!("Socket created as super peer: {:?}", super_peer);
                self_role = Some(PeerRole::Super);
                initialized = true;
            } else if let Some(parent) = socket.parent_peer() {
                info!("Socket created as child peer (parent: {:?})", parent);
                parent_id = Some(parent);
                self_role = Some(PeerRole::Child);
                initialized = true;
            }
        }

        // Handle new peer connections/disconnections.
        for (peer, state) in socket.update_peers() {
            match state {
                PeerState::Connected => {
                    info!("Peer joined: {}", peer);

                    // Initialize handshake state for the new peer.
                    network_state
                        .peer_handshake_states
                        .insert(peer, HandshakeState::new());

                    // For super peers, send handshake to any new peer.
                    if let Some(PeerRole::Super) = self_role {
                        send_handshake(&mut socket, &mut network_state, peer, "super_handshake");
                    }
                    // For child peers, check if this is our parent.
                    else if let Some(parent) = parent_id {
                        if peer == parent {
                            info!("Parent peer connection established");
                            send_handshake(&mut socket, &mut network_state, peer, "child_handshake");
                        }
                    }
                }
                PeerState::Disconnected => {
                    info!("Peer left: {}", peer);
                    network_state.remove_peer(peer);
                    network_state.print_state();
                }
            }
        }

        if let Some(my_peer_id) = socket.id() {
            match self_role {
                Some(PeerRole::Super) => {
                    // Send a hello message to all other super peers.
                    for super_peer in network_state.super_peers.keys() {
                        let hello_msg = format!("hello_super_peer|{}", my_peer_id);
                        let packet = hello_msg.as_bytes().to_vec().into_boxed_slice();
                        socket.channel_mut(CHANNEL_ID).send(packet, *super_peer);
                    }
                    // Additionally, send a message to all my child peers.
                    for child_peer in network_state.child_peers.keys() {
                        let hello_msg = format!("hello_super_peer|{}", my_peer_id);
                        let packet = hello_msg.as_bytes().to_vec().into_boxed_slice();
                        socket.channel_mut(CHANNEL_ID).send(packet, *child_peer);
                    }
                }
                Some(PeerRole::Child) => {
                    // Ensure parent has acknowledged before sending a hello message.
                    if let Some(parent) = parent_id {
                        if let Some(state) = network_state.peer_handshake_states.get(&parent) {
                            if state.acknowledged {
                                let hello_msg = format!("hello_parent|{}", my_peer_id);
                                let packet = hello_msg.as_bytes().to_vec().into_boxed_slice();
                                socket.channel_mut(CHANNEL_ID).send(packet, parent);
                            } else {
                                info!(
                                    "Waiting for parent {} to acknowledge before sending hello message",
                                    parent
                                );
                            }
                        }
                    }
                }
                None => {}
            }
        }

        // Process incoming messages.
        for (peer, packet) in socket.channel_mut(CHANNEL_ID).receive() {
            let message = String::from_utf8_lossy(&packet).to_string();

            info!("Message from {}: {}", peer, message);

            // Process handshake messages.
            match message.as_str() {
                "super_handshake" => {
                    if let Some(PeerRole::Super) = self_role {
                        network_state.add_super_peer(peer, "OtherSuperPeer".to_string());
                        info!("Discovered another super peer: {}", peer);

                        let ack_msg = "super_handshake_ack";
                        info!("Sending super peer acknowledgment to {}: {}", peer, ack_msg);
                        let packet = ack_msg.as_bytes().to_vec().into_boxed_slice();
                        socket.channel_mut(CHANNEL_ID).send(packet, peer);
                    } else {
                        network_state.add_super_peer(peer, "SuperPeerInfo".to_string());
                        info!("Received super peer handshake from {}", peer);

                        let ack_msg = "super_handshake_ack";
                        info!("Sending acknowledgment to super peer {}: {}", peer, ack_msg);
                        let packet = ack_msg.as_bytes().to_vec().into_boxed_slice();
                        socket.channel_mut(CHANNEL_ID).send(packet, peer);
                    }
                    network_state.print_state();
                }
                "super_handshake_ack" => {
                    if let Some(state) = network_state.peer_handshake_states.get_mut(&peer) {
                        state.acknowledge();
                        info!("Super peer handshake acknowledged by {}", peer);
                    }
                    if let Some(PeerRole::Super) = self_role {
                        network_state.add_super_peer(peer, "OtherSuperPeer".to_string());
                        info!("Confirmed another super peer: {}", peer);
                        network_state.print_state();
                    }
                }
                "child_handshake" => {
                    network_state.add_child_peer(peer, "ChildPeerInfo".to_string());
                    info!("Received child peer handshake from {}", peer);

                    let ack_msg = "child_handshake_ack";
                    info!("Sending acknowledgement to child {}: {}", peer, ack_msg);
                    let packet = ack_msg.as_bytes().to_vec().into_boxed_slice();
                    socket.channel_mut(CHANNEL_ID).send(packet, peer);

                    network_state.print_state();
                }
                "child_handshake_ack" => {
                    if let Some(state) = network_state.peer_handshake_states.get_mut(&peer) {
                        state.acknowledge();
                        info!("Child handshake acknowledged by parent {}", peer);
                    }
                }
                _ => {
                    // Non-handshake messages get processed here.
                    info!("Received message from {}: {}", peer, message);

                    // Only super peers forward messages.
                    if let Some(PeerRole::Super) = self_role {
                        // Forward non-handshake messages appropriately:
                        // Check if the sender is a child peer.
                        if network_state.child_peers.contains_key(&peer) {
                            // Forward to all child peers and super peers (except the sender).
                            for child_peer in network_state.child_peers.keys() {
                                if *child_peer != peer {
                                    socket.channel_mut(CHANNEL_ID)
                                        .send(packet.clone(), *child_peer);
                                }
                            }
                            for super_peer in network_state.super_peers.keys() {
                                if *super_peer != peer {
                                    socket.channel_mut(CHANNEL_ID)
                                        .send(packet.clone(), *super_peer);
                                }
                            }
                        }
                        // Otherwise, if the sender is a super peer.
                        else if network_state.super_peers.contains_key(&peer) {
                            // Forward only to child peers.
                            for child_peer in network_state.child_peers.keys() {
                                socket.channel_mut(CHANNEL_ID)
                                    .send(packet.clone(), *child_peer);
                            }
                        }
                    }
                }
            }
        }

        // Maintain the loop with a timeout.
        select! {
            _ = (&mut timeout).fuse() => {
                timeout.reset(Duration::from_millis(500));

                // Check for peers that need handshake retries.
                let peers_to_retry = network_state.get_peers_to_retry();
                for peer in peers_to_retry {
                    match self_role {
                        Some(PeerRole::Super) => {
                            send_handshake(&mut socket, &mut network_state, peer, "super_handshake");
                        },
                        Some(PeerRole::Child) => {
                            if let Some(parent) = parent_id {
                                if peer == parent {
                                    send_handshake(&mut socket, &mut network_state, peer, "child_handshake");
                                }
                            }
                        },
                        None => {}
                    }
                }
            }
            _ = &mut loop_fut => {
                break;
            }
        }
    }
}

// Helper function to send handshakes and update the handshake state.
fn send_handshake(
    socket: &mut WebRtcSocket,
    network_state: &mut NetworkState,
    peer: PeerId,
    handshake_msg: &str,
) {
    if let Some(state) = network_state.peer_handshake_states.get_mut(&peer) {
        info!(
            "Sending handshake to {}: {} (attempt {})",
            peer,
            handshake_msg,
            state.attempts + 1
        );
        let packet = handshake_msg.as_bytes().to_vec().into_boxed_slice();
        socket.channel_mut(CHANNEL_ID).send(packet, peer);
        state.record_attempt();
    }
}
