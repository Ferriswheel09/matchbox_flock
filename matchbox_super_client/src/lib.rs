use futures::{select, FutureExt};
use futures_timer::Delay;
use js_sys::Date;
use log::info;
use matchbox_socket::{PeerId, PeerState, WebRtcSocket, WebRtcSocketBuilder};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;
use url::Url;
use uuid::Uuid;
use wasm_bindgen::prelude::wasm_bindgen;
use wasm_bindgen::JsValue;
use wasm_bindgen_futures::js_sys;

const CHANNEL_ID: usize = 0;
const HANDSHAKE_RETRY_INTERVAL_MS: u64 = 1000; // Retry every 1 second
const MAX_HANDSHAKE_ATTEMPTS: u8 = 5; // Maximum retry attempts

#[derive(Debug, Serialize, Deserialize)]
enum PeerRole {
    Super,
    Child,
}

#[derive(Serialize, Deserialize)]
struct HandshakeState {
    attempts: u8,
    acknowledged: bool,
}

#[wasm_bindgen]
pub struct Client {
    // All fields are now wrapped in Rc<RefCell<...>>.
    info: Rc<RefCell<String>>,
    url: Rc<RefCell<String>>,
    peer_info: Rc<RefCell<HashMap<PeerId, String>>>,
    super_peers: Rc<RefCell<HashMap<PeerId, String>>>, // Map peer id to any additional info
    child_peers: Rc<RefCell<HashMap<PeerId, String>>>,
    peer_handshake_states: Rc<RefCell<HashMap<PeerId, HandshakeState>>>,
    last_seen: Rc<RefCell<HashMap<PeerId, f64>>>,
}

#[wasm_bindgen]
impl Client {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self {
            info: Rc::new(RefCell::new(String::new())),
            url: Rc::new(RefCell::new(String::new())),
            peer_info: Rc::new(RefCell::new(HashMap::new())),
            super_peers: Rc::new(RefCell::new(HashMap::new())),
            child_peers: Rc::new(RefCell::new(HashMap::new())),
            peer_handshake_states: Rc::new(RefCell::new(HashMap::new())),
            last_seen: Rc::new(RefCell::new(HashMap::new())),
        }
    }

    #[wasm_bindgen]
    pub fn set_url(&mut self, url: String) {
        *self.url.borrow_mut() = url;
    }

    #[wasm_bindgen]
    pub fn set_info(&mut self, info: String) {
        // Update the RefCell content for info
        *self.info.borrow_mut() = info;
    }

    #[wasm_bindgen(getter)]
    pub fn info(&self) -> String {
        self.info.borrow().clone()
    }

    #[wasm_bindgen]
    pub fn add_peer_info(&mut self, peer_id: String, info: String) {
        let peer_uuid = Uuid::parse_str(&peer_id).expect("Not a valid Peer ID");
        let peer_id = PeerId(peer_uuid);
        self.peer_info.borrow_mut().insert(peer_id, info.clone());
    }

    #[wasm_bindgen]
    pub fn getAllInfo(&self) -> js_sys::Map {
        let result = js_sys::Map::new();
        for (peer_id, info) in self.peer_info.borrow().iter() {
            let peer_id_str = peer_id.to_string();
            let peer_id_js = wasm_bindgen::JsValue::from_str(&peer_id_str);
            let info_js = wasm_bindgen::JsValue::from_str(info);
            result.set(&peer_id_js, &info_js);
        }
        result
    }

    #[wasm_bindgen]
    pub fn add_super_peer(&mut self, peer: String, info: String) {
        let peer_uuid = Uuid::parse_str(&peer).expect("Invalid UUID format");
        let peer_id = PeerId(peer_uuid);
        self.super_peers.borrow_mut().insert(peer_id, info.clone());
    }

    #[wasm_bindgen]
    pub fn add_child_peer(&mut self, peer: String, info: String) {
        let peer_uuid = Uuid::parse_str(&peer).expect("Invalid UUID format");
        let peer_id = PeerId(peer_uuid);
        self.child_peers.borrow_mut().insert(peer_id, info.clone());
    }

    #[wasm_bindgen]
    pub fn remove_peer(&mut self, peer: String) {
        let peer_uuid = Uuid::parse_str(&peer).expect("Invalid UUID format");
        let peer_id = PeerId(peer_uuid);

        self.super_peers.borrow_mut().remove(&peer_id);
        self.child_peers.borrow_mut().remove(&peer_id);
        self.peer_handshake_states.borrow_mut().remove(&peer_id);
        self.peer_info.borrow_mut().remove(&peer_id);
    }

    // Get peers to retry the handshake (peers with less than MAX_HANDSHAKE_ATTEMPTS and not acknowledged)
    #[wasm_bindgen]
    pub fn get_peers_to_retry(&self) -> Vec<String> {
        self.peer_handshake_states
            .borrow()
            .iter()
            .filter(|(_, state)| state.attempts < MAX_HANDSHAKE_ATTEMPTS && !state.acknowledged)
            .map(|(peer_id, _)| peer_id.to_string())
            .collect()
    }

    // Modified helper to take the handshake state map from self.
    fn send_handshake(
        socket: &mut WebRtcSocket,
        handshake_states: &Rc<RefCell<HashMap<PeerId, HandshakeState>>>,
        peer: String,
        handshake_msg: &str,
    ) {
        let peer_uuid = Uuid::parse_str(&peer).expect("Invalid UUID format");
        let peer_id = PeerId(peer_uuid);

        if let Some(state) = handshake_states.borrow_mut().get_mut(&peer_id) {
            info!(
                "Sending handshake to {}: {} (attempt {})",
                peer,
                handshake_msg,
                state.attempts + 1
            );
            let packet = handshake_msg.as_bytes().to_vec().into_boxed_slice();
            socket.channel_mut(CHANNEL_ID).send(packet, peer_id);
            state.attempts += 1;
        }
    }

    #[wasm_bindgen]
    pub fn start(&mut self) -> Result<(), wasm_bindgen::JsValue> {
        console_error_panic_hook::set_once();
        console_log::init_with_level(log::Level::Debug).unwrap();

        let url_rc = self.url.clone();
        let url = url_rc.borrow().trim().to_string();

        // If URL is not defined, bail out early
        if url.is_empty() {
            info!("Signaling server URL not set, skipping connection.");
            return Ok(());
        }

        let parsed_url =
            url::Url::parse(&url).map_err(|e| JsValue::from_str(&format!("Invalid URL: {}", e)))?;

        let host = parsed_url
            .host_str()
            .ok_or_else(|| JsValue::from_str("No host found in URL"))?
            .to_string();

        let turn_port = 3478;
        let turn_url = format!("{}:{}", host, turn_port);

        info!("Connecting to matchbox");

        // Clone self’s inner Rc values to use within the async block.
        let info_rc = self.info.clone();
        let peer_info_rc = self.peer_info.clone();
        let super_peers_rc = self.super_peers.clone();
        let child_peers_rc = self.child_peers.clone();
        let handshake_states_rc = self.peer_handshake_states.clone();
        let last_seen_rc = self.last_seen.clone();

        wasm_bindgen_futures::spawn_local(async move {
            let signaling_url = url_rc.borrow().clone();

            let parsed_url = Url::parse(&signaling_url)
                .map_err(|e| JsValue::from_str(&format!("Invalid URL: {}", e)))
                .unwrap(); // you can handle this better if needed

            let host = parsed_url
                .host_str()
                .expect("No host in signaling URL")
                .to_string();

            // Optional ICE server setup
            let mut builder = WebRtcSocketBuilder::new(&signaling_url)
                .add_reliable_channel()
                .reconnect_attempts(Some(5));

            if host != "localhost" && host != "127.0.0.1" {
                let turn_port = 3478;
                let ice_host = format!("{}:{}", host, turn_port);

                let turn_server = matchbox_socket::RtcIceServerConfig {
                    urls: vec![format!("stun:{}", ice_host), format!("turn:{}", ice_host)],
                    username: Some("youruser".to_string()),
                    credential: Some("yourpassword".to_string()),
                };

                builder = builder.ice_server(turn_server);
            } else {
                info!("Running locally; no TURN/STUN server needed");
            }

            let (mut socket, loop_fut) = builder.build();

            let mut loop_fut = loop_fut.fuse();
            futures::pin_mut!(loop_fut);

            let mut initialized = false;
            let mut parent_id: Option<PeerId> = None;
            let mut self_role: Option<PeerRole> = None;

            let mut timeout = Delay::new(Duration::from_millis(100));
            futures::pin_mut!(timeout);

            loop {
                // Determine our role once on start.
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
                            handshake_states_rc.borrow_mut().insert(
                                peer,
                                HandshakeState {
                                    attempts: 0,
                                    acknowledged: false,
                                },
                            );

                            if let Some(PeerRole::Super) = self_role {
                                Self::send_handshake(
                                    &mut socket,
                                    &handshake_states_rc,
                                    peer.to_string(),
                                    "super_handshake",
                                );
                            } else if let Some(parent) = parent_id {
                                if peer == parent {
                                    info!("Parent peer connection established");
                                    Self::send_handshake(
                                        &mut socket,
                                        &handshake_states_rc,
                                        peer.to_string(),
                                        "child_handshake",
                                    );
                                }
                            }
                        }
                        PeerState::Disconnected => {
                            info!("Peer left: {}", peer);
                            // Remove from all client maps.
                            super_peers_rc.borrow_mut().remove(&peer);
                            child_peers_rc.borrow_mut().remove(&peer);
                            handshake_states_rc.borrow_mut().remove(&peer);
                            peer_info_rc.borrow_mut().remove(&peer);
                            if Some(peer) == parent_id {
                                initialized = false;
                            }
                        }
                    }
                }

                // Send our info based on our role.
                if let Some(my_peer_id) = socket.id() {
                    match self_role {
                        Some(PeerRole::Super) => {
                            for peer in super_peers_rc.borrow().keys() {
                                if let Some(state) = handshake_states_rc.borrow().get(peer) {
                                    if state.acknowledged {
                                        let info_msg =
                                            format!("info|{}|{}", my_peer_id, info_rc.borrow());
                                        let packet =
                                            info_msg.as_bytes().to_vec().into_boxed_slice();
                                        socket.channel_mut(CHANNEL_ID).send(packet, *peer);
                                    }
                                }
                            }
                            for peer in child_peers_rc.borrow().keys() {
                                if let Some(state) = handshake_states_rc.borrow().get(peer) {
                                    if state.acknowledged {
                                        let info_msg =
                                            format!("info|{}|{}", my_peer_id, info_rc.borrow());
                                        let packet =
                                            info_msg.as_bytes().to_vec().into_boxed_slice();
                                        socket.channel_mut(CHANNEL_ID).send(packet, *peer);
                                    }
                                }
                            }
                        }
                        Some(PeerRole::Child) => {
                            if let Some(parent) = parent_id {
                                if let Some(state) = handshake_states_rc.borrow().get(&parent) {
                                    if state.acknowledged {
                                        let info_msg =
                                            format!("info|{}|{}", my_peer_id, info_rc.borrow());
                                        let packet =
                                            info_msg.as_bytes().to_vec().into_boxed_slice();
                                        socket.channel_mut(CHANNEL_ID).send(packet, parent);
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

                    if message.starts_with("info|") {
                        let parts: Vec<&str> = message.splitn(3, '|').collect();
                        if parts.len() == 3 {
                            let sender_id_str = parts[1];
                            let actual_info = parts[2].to_string();
                            if let Ok(sender_uuid) = Uuid::parse_str(sender_id_str) {
                                let sender_id = PeerId(sender_uuid);
                                peer_info_rc.borrow_mut().insert(sender_id, actual_info);
                                last_seen_rc.borrow_mut().insert(sender_id, Date::now());

                                if let Some(PeerRole::Super) = self_role {
                                    for child_peer in child_peers_rc.borrow().keys() {
                                        if *child_peer != peer {
                                            socket
                                                .channel_mut(CHANNEL_ID)
                                                .send(packet.clone(), *child_peer);
                                        }
                                    }
                                    for super_peer in super_peers_rc.borrow().keys() {
                                        if *super_peer != peer {
                                            socket
                                                .channel_mut(CHANNEL_ID)
                                                .send(packet.clone(), *super_peer);
                                        }
                                    }
                                }
                            } else {
                                info!("Error parsing sender ID in info message: invalid UUID");
                            }
                        } else {
                            info!("Received malformed info message: {}", message);
                        }
                    } else if message == "super_handshake" {
                        if let Some(PeerRole::Super) = self_role {
                            super_peers_rc
                                .borrow_mut()
                                .insert(peer, "OtherSuperPeer".to_string());
                            info!("Discovered another super peer: {}", peer);

                            let ack_msg = "super_handshake_ack";
                            info!("Sending super peer acknowledgment to {}: {}", peer, ack_msg);
                            let packet = ack_msg.as_bytes().to_vec().into_boxed_slice();
                            socket.channel_mut(CHANNEL_ID).send(packet, peer);
                        } else {
                            super_peers_rc
                                .borrow_mut()
                                .insert(peer, "SuperPeerInfo".to_string());
                            info!("Received super peer handshake from {}", peer);

                            let ack_msg = "super_handshake_ack";
                            info!("Sending acknowledgment to super peer {}: {}", peer, ack_msg);
                            let packet = ack_msg.as_bytes().to_vec().into_boxed_slice();
                            socket.channel_mut(CHANNEL_ID).send(packet, peer);
                        }
                    } else if message == "super_handshake_ack" {
                        if let Some(PeerRole::Super) = self_role {
                            if let Some(mut states) = handshake_states_rc.try_borrow_mut().ok() {
                                if let Some(state) = states.get_mut(&peer) {
                                    state.acknowledged = true;
                                    info!("Super peer handshake acknowledged by {}", peer);
                                    if let Some(my_peer_id) = socket.id() {
                                        let info_msg =
                                            format!("info|{}|{}", my_peer_id, info_rc.borrow());
                                        let packet =
                                            info_msg.as_bytes().to_vec().into_boxed_slice();
                                        socket.channel_mut(CHANNEL_ID).send(packet, peer);
                                        info!(
                                            "Sent info after super peer handshake ack: {}",
                                            info_msg
                                        );
                                    }
                                }
                            }
                            super_peers_rc
                                .borrow_mut()
                                .insert(peer, "OtherSuperPeer".to_string());
                            info!("Confirmed another super peer: {}", peer);
                        }
                    } else if message == "child_handshake" {
                        child_peers_rc
                            .borrow_mut()
                            .insert(peer, "ChildPeerInfo".to_string());
                        info!("Received child peer handshake from {}", peer);

                        let ack_msg = "child_handshake_ack";
                        info!("Sending acknowledgement to child {}: {}", peer, ack_msg);
                        let packet = ack_msg.as_bytes().to_vec().into_boxed_slice();
                        socket.channel_mut(CHANNEL_ID).send(packet, peer);
                    } else if message == "child_handshake_ack" {
                        if let Some(mut states) = handshake_states_rc.try_borrow_mut().ok() {
                            if let Some(state) = states.get_mut(&peer) {
                                state.acknowledged = true;
                                info!("Child handshake acknowledged by parent {}", peer);
                                if let Some(my_peer_id) = socket.id() {
                                    let info_msg =
                                        format!("info|{}|{}", my_peer_id, info_rc.borrow());
                                    let packet = info_msg.as_bytes().to_vec().into_boxed_slice();
                                    socket.channel_mut(CHANNEL_ID).send(packet, peer);
                                    info!("Sent info after child handshake ack: {}", info_msg);
                                }
                            }
                        }
                    } else {
                        if let Some(PeerRole::Super) = self_role {
                            if child_peers_rc.borrow().contains_key(&peer) {
                                for child_peer in child_peers_rc.borrow().keys() {
                                    if *child_peer != peer {
                                        socket
                                            .channel_mut(CHANNEL_ID)
                                            .send(packet.clone(), *child_peer);
                                    }
                                }
                                for super_peer in super_peers_rc.borrow().keys() {
                                    if *super_peer != peer {
                                        socket
                                            .channel_mut(CHANNEL_ID)
                                            .send(packet.clone(), *super_peer);
                                    }
                                }
                            } else if super_peers_rc.borrow().contains_key(&peer) {
                                for child_peer in child_peers_rc.borrow().keys() {
                                    socket
                                        .channel_mut(CHANNEL_ID)
                                        .send(packet.clone(), *child_peer);
                                }
                            }
                        }
                    }
                }

                // Maintain the loop with a timeout.
                select! {
                    _ = (&mut timeout).fuse() => {
                        timeout.reset(Duration::from_millis(100));
                        let peers_to_retry: Vec<String> = handshake_states_rc.borrow()
                            .iter()
                            .filter(|(_, state)| state.attempts < MAX_HANDSHAKE_ATTEMPTS && !state.acknowledged)
                            .map(|(peer_id, _)| peer_id.to_string())
                            .collect();
                        for peer in peers_to_retry {
                            match self_role {
                                Some(PeerRole::Super) => {
                                    Self::send_handshake(
                                        &mut socket,
                                        &handshake_states_rc,
                                        peer,
                                        "super_handshake",
                                    );
                                },
                                Some(PeerRole::Child) => {
                                    if let Some(parent) = parent_id {
                                        if peer == parent.to_string() {
                                            Self::send_handshake(
                                                &mut socket,
                                                &handshake_states_rc,
                                                peer,
                                                "child_handshake",
                                            );
                                        }
                                    }
                                },
                                None => {}
                            }
                        }
                        let now = Date::now();
                        let mut to_remove = vec![];

                        for (peer_id, last_seen_time) in last_seen_rc.borrow().iter() {
                            if now - *last_seen_time > 5000.0 {
                                to_remove.push(*peer_id);
                            }
                        }

                        for peer_id in to_remove {
                            super_peers_rc.borrow_mut().remove(&peer_id);
                            child_peers_rc.borrow_mut().remove(&peer_id);
                            peer_info_rc.borrow_mut().remove(&peer_id);
                            handshake_states_rc.borrow_mut().remove(&peer_id);
                            last_seen_rc.borrow_mut().remove(&peer_id);
                        }
                    }
                    _ = &mut loop_fut => {
                        break;
                    }
                }
            }
        });

        Ok(())
    }
}
