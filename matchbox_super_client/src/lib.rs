use futures::{select, FutureExt};
use futures_timer::Delay;
use log::info;
use matchbox_socket::{
    Packet, PeerId, PeerState, RtcIceServerConfig, WebRtcSocket, WebRtcSocketBuilder,
};
use serde::{Deserialize, Serialize};
use serde_json;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;
use uuid::Uuid;
use wasm_bindgen::prelude::*;

#[derive(Serialize, Deserialize, Debug, Clone)]
struct Position {
    #[serde(skip_serializing_if = "Option::is_none")]
    peer_id: Option<String>,
    x: i32,
    y: i32,
    x_velocity: i32,
    y_velocity: i32,
}

#[derive(Serialize, Deserialize, Debug)]
enum Message {
    PositionUpdate(Position),
    Forward(Position),
    PeerList(Vec<String>),
}

#[wasm_bindgen]
pub struct Client {
    // Identity
    peer_id: Option<PeerId>,
    is_super: bool,

    // Positioning
    position: Rc<RefCell<Position>>,
    peer_positions: Rc<RefCell<HashMap<PeerId, (i32, i32, i32, i32)>>>,

    // Networking
    known_super_peers: Rc<RefCell<Vec<PeerId>>>, // Super-peers track other super-peers
    socket: Option<WebRtcSocket>,
    super_peer: Option<PeerId>, // Each client has a designated super-peer if they are a regular peer
    connected_peers: Rc<RefCell<Vec<PeerId>>>,
}

#[wasm_bindgen]
impl Client {
    #[wasm_bindgen(constructor)]
    pub fn new(is_super: bool) -> Client {
        Client {
            peer_id: None,
            is_super,
            socket: None,
            super_peer: None,
            connected_peers: Rc::new(RefCell::new(Vec::new())),
            known_super_peers: Rc::new(RefCell::new(Vec::new())),
            peer_positions: Rc::new(RefCell::new(HashMap::new())),
            position: Rc::new(RefCell::new(Position {
                peer_id: None,
                x: 10,
                y: 20,
                x_velocity: 0,
                y_velocity: 0,
            })),
        }
    }

    // The below section will be adding all the methods from our original library
    // Will try to keep it as similar as possible to avoid rewriting API code in frontend

    // update_id: Update the current clients ID
    #[wasm_bindgen]
    pub fn update_id(&mut self, new_id: String) {
        match uuid::Uuid::parse_str(&new_id) {
            Ok(uuid) => {
                self.peer_id = Some(PeerId(uuid));
            }
            Err(err) => {
                eprintln!("Failed to parse UUID: {}", err);
            }
        }
    }

    // update_position: Update the current clients Position
    #[wasm_bindgen]
    pub fn update_position(&mut self, x: i32, y: i32, x_velocity: i32, y_velocity: i32) {
        let mut position = self.position.borrow_mut();
        position.x = x;
        position.y = y;
        position.x_velocity = x_velocity;
        position.y_velocity = y_velocity;
    }

    // get_position_velocity: Return the position and velocity of the current client as a string
    #[wasm_bindgen(getter)]
    pub fn get_position_velocity(&self) -> String {
        match &self.peer_id {
            Some(id) => format!(
                "Player {} is at position: X={}, Y={}, X Velocity={}, Y Velocity={}",
                id,
                self.position.borrow().x,
                self.position.borrow().y,
                self.position.borrow().x_velocity,
                self.position.borrow().y_velocity
            ),
            _ => "Error getting Position Velocity".to_string(),
        }
    }

    // Below are helper methods for the getters of the different position values
    #[wasm_bindgen(getter)]
    pub fn x(&self) -> i32 {
        self.position.borrow().x
    }

    #[wasm_bindgen(getter)]
    pub fn y(&self) -> i32 {
        self.position.borrow().y
    }

    #[wasm_bindgen(getter)]
    pub fn x_velocity(&self) -> i32 {
        self.position.borrow().x_velocity
    }

    #[wasm_bindgen(getter)]
    pub fn y_velocity(&self) -> i32 {
        self.position.borrow().y_velocity
    }

    // Need additional helper methods for state (debugging)
    #[wasm_bindgen(getter)]
    pub fn is_super_peer(&self) -> bool {
        self.is_super
    }

    // get_all_positions: Returns the list of all peers and all of their positions
    // Necessary in order to get current state of all players (as well as if any dropped)
    #[wasm_bindgen]
    pub fn get_all_positions(&self) -> JsValue {
        let peer_positions = self.peer_positions.borrow();
        let positions: Vec<Position> = peer_positions
            .iter()
            .map(|(peer_id, &(x, y, x_velocity, y_velocity))| Position {
                peer_id: Some(peer_id.to_string()),
                x,
                y,
                x_velocity,
                y_velocity,
            })
            .collect();

        JsValue::from_serde(&positions).unwrap_or(JsValue::from_str("[]"))
    }

    /// Gets the client's current peer ID as a string.
    #[wasm_bindgen(getter)]
    pub fn peer_id_string(&self) -> String {
        match &self.peer_id {
            Some(id) => id.0.to_string(),
            None => "Not set".to_string(),
        }
    }

    /// Gets the number of connected peers.
    #[wasm_bindgen(getter)]
    pub fn connected_peer_count(&self) -> usize {
        self.connected_peers.borrow().len()
    }

    /// Gets the number of known super peers (only relevant for super peers).
    #[wasm_bindgen(getter)]
    pub fn known_super_peer_count(&self) -> usize {
        if self.is_super {
            self.known_super_peers.borrow().len()
        } else {
            0
        }
    }

    // Broadcast method that, if the client is a super peer
    // Will send all messages to all super peers in the network
    // Who will then transmit all messages to all clients
    // TODO: Will need an abstraction before passing messages to all peers
    #[wasm_bindgen]
    pub fn broadcast_message(&mut self, packet: Packet) {
        if self.is_super {
            // Need to fix this relating to the channels
            for peer in self.known_super_peers.borrow().iter() {
                if let Some(socket) = &mut self.socket {
                    socket.channel_mut(0).send(packet.clone(), *peer);
                }
            }
            info!("Forwarded position update to super-peers.");
        }
    }

    // Sending method that the client can use to either
    // a. send to their correspondent super peer or
    // b. broadcast to the network if they are a regular peer
    #[wasm_bindgen]
    pub fn send(&mut self, packet: Packet) {
        if self.is_super {
            self.broadcast_message(packet);
        } else {
            if let Some(socket) = &mut self.socket {
                if let Some(super_peer) = &self.super_peer {
                    socket
                        .channel_mut(0)
                        .send(packet.clone(), super_peer.clone());
                }
            }
        }
    }

    // Method that will send all peer information to another super peer
    // This will be useful if a new super peer joins the network
    #[wasm_bindgen]
    pub fn send_super_peer_list(&mut self) {
        if self.is_super {
            // Get the known super peers from the client to pass to the new connecting super peer
            let peer_list: Vec<String> = self
                .known_super_peers
                .borrow()
                .iter()
                .map(|peer| peer.0.to_string()) // Extract Uuid from PeerId
                .collect();
            let msg = Message::PeerList(peer_list);
            let packet = serde_json::to_vec(&msg).unwrap().into_boxed_slice();

            for peer in self.connected_peers.borrow().iter() {
                if let Some(socket) = &mut self.socket {
                    socket.channel_mut(0).send(packet.clone(), *peer);
                }
            }
            info!("Super-peer sent peer list.");
        }
    }

    // Helper function that is necessary for new super peers entering the network
    // where given the list of super peers, add them to our local struct
    // TODO: What will happen if a peer gets promoted?
    // Maybe we send the peer list to every other peer for redundancy to limit the overhead
    #[wasm_bindgen]
    pub fn add_super_peer_list(&mut self, peer_list: Packet) {
        // Check to make sure that the peer is super (shouldn't add to list otherwise)
        if self.is_super {
            let message: Message =
                serde_json::from_slice(&peer_list).expect("Couldn't convert message to peer list");
            match message {
                Message::PeerList(peers) => {
                    let mut peer_ref = self.known_super_peers.borrow_mut();
                    for peer in peers {
                        peer_ref.push(PeerId(
                            Uuid::parse_str(&peer).expect("Couldn't conver to UUID"),
                        ));
                    }
                }
                _ => panic!("Expected peer list "),
            }
        }
    }

    // Start method for the server
    #[wasm_bindgen]
    pub fn start(&mut self) -> Result<(), JsValue> {
        console_error_panic_hook::set_once();
        console_log::init_with_level(log::Level::Debug).unwrap();

        info!("Starting P2P client...");

        let position_ref = self.position.clone();
        let connected_peers_ref = self.connected_peers.clone();
        let peer_positions_ref = self.peer_positions.clone();
        let known_super_peers_ref = self.known_super_peers.clone();
        let is_super = self.is_super;
        let peer_id_ref = Rc::new(RefCell::new(self.peer_id.clone()));
        let super_peer_ref = Rc::new(RefCell::new(self.super_peer.clone()));

        // let turn_server = matchbox_socket::RtcIceServerConfig {
        //     urls: vec![
        //         "stun:184.72.189.6:3478".to_string(),
        //         "turn:184.72.189.6:3478".to_string(),
        //     ],
        //     username: Some("youruser".to_string()),
        //     credential: Some("yourpassword".to_string()),
        // };

        // Spawn a future that will run the loop
        wasm_bindgen_futures::spawn_local(async move {
            // Use the provided room URL or default to the AWS server
            let url = "ws://localhost:3536".to_string();
            
            let (mut socket, loop_fut) = WebRtcSocketBuilder::new(&url)
                // .ice_server(turn_server)
                .add_unreliable_channel()
                .build();

            let loop_fut = loop_fut.fuse();
            futures::pin_mut!(loop_fut);

            let timeout = Delay::new(Duration::from_millis(100));
            futures::pin_mut!(timeout);

            info!("P2P client started. Is super peer: {}", is_super);

            loop {
                // Update peer ID if not set
                if peer_id_ref.borrow().is_none() {
                    if let Some(socket_peer_id) = socket.id() {
                        *peer_id_ref.borrow_mut() = Some(socket_peer_id);
                        position_ref.borrow_mut().peer_id = Some(socket_peer_id.0.to_string());
                        info!("Client ID set to: {}", socket_peer_id);
                    }
                }

                // Process peer updates
                for (peer, state) in socket.update_peers() {
                    let mut connected_peers = connected_peers_ref.borrow_mut();
                    let mut peer_positions = peer_positions_ref.borrow_mut();

                    match state {
                        PeerState::Connected => {
                            info!("Peer joined: {peer}");
                            connected_peers.push(peer);

                            // If this is a super peer, add the new peer to known super peers
                            if is_super {
                                let mut super_peers = known_super_peers_ref.borrow_mut();
                                super_peers.push(peer);

                                // Send the current list of super peers to the new peer
                                let peer_list: Vec<String> =
                                    super_peers.iter().map(|p| p.0.to_string()).collect();

                                let msg = Message::PeerList(peer_list);
                                if let Ok(json_data) = serde_json::to_vec(&msg) {
                                    let packet = json_data.into_boxed_slice();
                                    socket.channel_mut(0).send(packet, peer);
                                }
                            } else if !is_super && super_peer_ref.borrow().is_none() {
                                // Regular peer without a super-peer assigns the first one it sees
                                *super_peer_ref.borrow_mut() = Some(peer);
                                info!("Assigned super peer: {peer}");
                            }
                        }
                        PeerState::Disconnected => {
                            connected_peers.retain(|&p| p != peer);
                            peer_positions.remove(&peer);

                            // If super peer, remove from known super peers
                            if is_super {
                                let mut super_peers = known_super_peers_ref.borrow_mut();
                                super_peers.retain(|&p| p != peer);
                            }

                            // If the disconnected peer was this client's super peer, clear it
                            if let Some(current_super_peer) = *super_peer_ref.borrow() {
                                if current_super_peer == peer {
                                    *super_peer_ref.borrow_mut() = None;
                                    info!("Super peer disconnected, looking for new super peer");
                                }
                            }

                            info!("Peer left: {peer}");
                        }
                    }
                }

                // Send position update to peers
                {
                    let position = position_ref.borrow();
                    let connected_peers = connected_peers_ref.borrow();

                    // Create a Position object with current state
                    let position_data = Position {
                        peer_id: peer_id_ref.borrow().as_ref().map(|id| id.0.to_string()),
                        x: position.x,
                        y: position.y,
                        x_velocity: position.x_velocity,
                        y_velocity: position.y_velocity,
                    };

                    // Create a message
                    let message = Message::PositionUpdate(position_data);

                    // Serialize to JSON
                    if let Ok(json_data) = serde_json::to_vec(&message) {
                        let packet = json_data.into_boxed_slice();

                        // Broadcast or send to super peer based on client type
                        if is_super {
                            // Super peers broadcast to all peers
                            for peer in connected_peers.iter() {
                                socket.channel_mut(0).send(packet.clone(), *peer);
                            }
                        } else if let Some(super_peer) = *super_peer_ref.borrow() {
                            // Regular peers only send to their super peer
                            socket.channel_mut(0).send(packet.clone(), super_peer);
                        }
                    }
                }

                // Receive data from peers
                for (peer, packet) in socket.channel_mut(0).receive() {
                    if let Ok(message) = serde_json::from_slice::<Message>(&packet) {
                        match message {
                            Message::PositionUpdate(position_data) => {
                                // Store the peer's position
                                let mut peer_positions = peer_positions_ref.borrow_mut();
                                peer_positions.insert(
                                    peer,
                                    (
                                        position_data.x,
                                        position_data.y,
                                        position_data.x_velocity,
                                        position_data.y_velocity,
                                    ),
                                );

                                // If super peer, forward to all other peers
                                if is_super {
                                    let forward_message = Message::Forward(position_data);
                                    if let Ok(forward_data) = serde_json::to_vec(&forward_message) {
                                        let forward_packet = forward_data.into_boxed_slice();
                                        for other_peer in connected_peers_ref.borrow().iter() {
                                            if *other_peer != peer {
                                                socket
                                                    .channel_mut(0)
                                                    .send(forward_packet.clone(), *other_peer);
                                            }
                                        }
                                    }
                                }
                            }
                            Message::Forward(position_data) => {
                                // Store forwarded position data
                                if let Some(peer_id_str) = &position_data.peer_id {
                                    if let Ok(uuid) = Uuid::parse_str(peer_id_str) {
                                        let peer_id = PeerId(uuid);
                                        let mut peer_positions = peer_positions_ref.borrow_mut();
                                        peer_positions.insert(
                                            peer_id,
                                            (
                                                position_data.x,
                                                position_data.y,
                                                position_data.x_velocity,
                                                position_data.y_velocity,
                                            ),
                                        );
                                    }
                                }
                            }
                            Message::PeerList(peers) => {
                                // Only process peer list if we're a super peer
                                if is_super {
                                    let mut super_peers = known_super_peers_ref.borrow_mut();
                                    for peer_str in peers {
                                        if let Ok(uuid) = Uuid::parse_str(&peer_str) {
                                            let peer_id = PeerId(uuid);
                                            if !super_peers.contains(&peer_id) {
                                                super_peers.push(peer_id);
                                            }
                                        }
                                    }
                                    info!(
                                        "Updated super peers list, now has {} entries",
                                        super_peers.len()
                                    );
                                }
                            }
                        }
                    }
                }

                select! {
                    _ = (&mut timeout).fuse() => {
                        timeout.reset(Duration::from_millis(100));
                    }
                    _ = &mut loop_fut => {
                        info!("Loop future finished.");
                        break;
                    }
                }
            }

            // We should update the original client with the new values
            // but since we're in an async context, we can't directly modify self
        });

        Ok(())
    }
}
