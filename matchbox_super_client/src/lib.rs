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

#[derive(Serialize, Deserialize)]
struct Position {
    peer_id: Option<String>,
    x: i32,
    y: i32,
    x_velocity: i32,
    y_velocity: i32,
}

#[derive(Serialize, Deserialize)]
enum Message {
    PositionUpdate(Position),
    PeerList(Vec<String>),
    SuperPeer(String),
    ChildPeer(String),
}

#[wasm_bindgen]
pub struct Client {
    // Identity
    peer_id: Option<PeerId>,
    is_super: Option<bool>,

    // Positioning
    position: Rc<RefCell<Position>>,
    peer_positions: Rc<RefCell<HashMap<PeerId, (i32, i32, i32, i32)>>>,

    // Hash map
    // Networking
    known_super_peers: Rc<RefCell<Vec<PeerId>>>, // Super-peers track other super-peers
    known_children_peers: Rc<RefCell<Vec<PeerId>>>, // Super-peers track their children
    parent_peer: Option<PeerId>, // Each client has a designated super-peer if they are a regular peer
    connected_peers: Rc<RefCell<Vec<PeerId>>>,
}

#[wasm_bindgen]
impl Client {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Client {
        Client {
            peer_id: None,
            is_super: None,
            parent_peer: None,
            connected_peers: Rc::new(RefCell::new(Vec::new())),
            known_super_peers: Rc::new(RefCell::new(Vec::new())),
            known_children_peers: Rc::new(RefCell::new(Vec::new())),
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

    // Start method for the server
    #[wasm_bindgen]
    pub fn start(&mut self) -> Result<(), JsValue> {
        console_error_panic_hook::set_once();
        console_log::init_with_level(log::Level::Debug).unwrap();

        info!("Starting P2P client...");

        // Create references to the struct's fields
        let position_ref = self.position.clone();
        let connected_peers_ref = self.connected_peers.clone();
        let peer_positions_ref = self.peer_positions.clone();
        let known_super_peers_ref = self.known_super_peers.clone();
        let known_children_peers_ref = self.known_children_peers.clone();
        let peer_id_ref = Rc::new(RefCell::new(self.peer_id.clone()));
        let parent_peer_ref = Rc::new(RefCell::new(self.parent_peer.clone()));
        let is_super_ref = Rc::new(RefCell::new(self.is_super.clone()));

        wasm_bindgen_futures::spawn_local(async move {
            let url = "ws://localhost:3536".to_string();
            let (mut socket, loop_fut) = WebRtcSocketBuilder::new(&url)
                .add_unreliable_channel()
                .build();

            let loop_fut = loop_fut.fuse();
            futures::pin_mut!(loop_fut);
            let timeout = Delay::new(Duration::from_millis(100));
            futures::pin_mut!(timeout);
            let mut role_assigned = false;

            loop {
                // Set peer ID if not already set
                if peer_id_ref.borrow().is_none() {
                    if let Some(socket_peer_id) = socket.id() {
                        *peer_id_ref.borrow_mut() = Some(socket_peer_id);
                        position_ref.borrow_mut().peer_id = Some(socket_peer_id.0.to_string());
                        info!("Client ID set to: {}", socket_peer_id);
                    }
                }

                // Handle peer connection and disconnection
                for (peer, state) in socket.update_peers() {
                    let mut connected_peers = connected_peers_ref.borrow_mut();
                    let mut super_peers = known_super_peers_ref.borrow_mut();
                    let mut child_peers = known_children_peers_ref.borrow_mut();
                    let mut peer_positions = peer_positions_ref.borrow_mut();

                    match state {
                        PeerState::Connected => {
                            info!("Peer joined: {peer}");
                            connected_peers.push(peer);
                        }
                        PeerState::Disconnected => {
                            connected_peers.retain(|&p| p != peer);
                            child_peers.retain(|&p| p != peer);
                            super_peers.retain(|&p| p != peer);
                            peer_positions.remove(&peer);
                            info!("Peer left: {peer}");
                        }
                    }
                }

                // Assign role (super peer or child peer)
                if !role_assigned && peer_id_ref.borrow().is_some() {
                    if let Some(is_super_peer) = socket.super_peer() {
                        *is_super_ref.borrow_mut() = Some(is_super_peer);
                        info!("Role assigned: Super peer = {}", is_super_peer);
                        role_assigned = true;

                        // If super peer, announce to the network
                        if is_super_peer {
                            if let Some(my_id) = *peer_id_ref.borrow() {
                                let announcement = Message::SuperPeer(my_id.0.to_string());
                                if let Ok(json_data) = serde_json::to_vec(&announcement) {
                                    let packet = json_data.into_boxed_slice();
                                    for peer in connected_peers_ref.borrow().iter() {
                                        socket.channel_mut(0).try_send(packet.clone(), *peer);
                                    }
                                }
                            }
                        }
                    } else if let Some(parent) = socket.parent_peer() {
                        *parent_peer_ref.borrow_mut() = Some(parent);
                        info!("Role assigned: Child peer with parent = {}", parent);
                        role_assigned = true;

                        // Register with parent super peer
                        if let Some(my_id) = *peer_id_ref.borrow() {
                            let registration = Message::ChildPeer(my_id.0.to_string());
                            if let Ok(json_data) = serde_json::to_vec(&registration) {
                                let packet = json_data.into_boxed_slice();
                                socket.channel_mut(0).try_send(packet, parent);
                                info!("Message sent to parent");
                            }
                        }
                    }
                }

                // Send position updates based on peer type
                {
                    let position = position_ref.borrow();
                    let is_super = is_super_ref.borrow().unwrap_or(false);

                    let position_data = Position {
                        peer_id: peer_id_ref.borrow().as_ref().map(|id| id.0.to_string()),
                        x: position.x,
                        y: position.y,
                        x_velocity: position.x_velocity,
                        y_velocity: position.y_velocity,
                    };

                    let message = Message::PositionUpdate(position_data);

                    if let Ok(json_data) = serde_json::to_vec(&message) {
                        let packet = json_data.into_boxed_slice();

                        if is_super {
                            // Super peer broadcasts to all connected peers
                            for peer in connected_peers_ref.borrow().iter() {
                                socket.channel_mut(0).try_send(packet.clone(), *peer);
                            }
                        } else if let Some(parent_peer) = *parent_peer_ref.borrow() {
                            // Child peer sends to its parent super peer
                            socket.channel_mut(0).try_send(packet.clone(), parent_peer);
                        }
                    }
                }

                // Process incoming messages
                for (peer, packet) in socket.channel_mut(0).receive() {
                    if let Ok(message) = serde_json::from_slice::<Message>(&packet) {
                        match message {
                            Message::ChildPeer(child_id) => {
                                // Super peer handles child peer registration
                                if is_super_ref.borrow().unwrap_or(false) {
                                    if let Ok(uuid) = Uuid::parse_str(&child_id) {
                                        let child_peer_id = PeerId(uuid);
                                        let mut children_peers =
                                            known_children_peers_ref.borrow_mut();
                                        if !children_peers.contains(&child_peer_id) {
                                            children_peers.push(child_peer_id);
                                            info!("Added child peer: {}", child_id);
                                        }
                                    }
                                }
                            }
                            Message::SuperPeer(super_id) => {
                                // Super peers share their list of known super peers
                                if let Ok(uuid) = Uuid::parse_str(&super_id) {
                                    let super_peer_id = PeerId(uuid);
                                    let mut super_peers = known_super_peers_ref.borrow_mut();

                                    if !super_peers.contains(&super_peer_id) {
                                        super_peers.push(super_peer_id);
                                        info!("Added super peer: {}", super_id);
                                    }

                                    let peer_list: Vec<String> =
                                        super_peers.iter().map(|p| p.0.to_string()).collect();

                                    let msg = Message::PeerList(peer_list);
                                    if let Ok(json_data) = serde_json::to_vec(&msg) {
                                        let packet = json_data.into_boxed_slice();
                                        socket.channel_mut(0).send(packet, peer);
                                    }
                                }
                            }
                            Message::PositionUpdate(position_data) => {
                                // Update peer positions
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

                                // Super peer forwards position to other peers
                                if is_super_ref.borrow().unwrap_or(false) {
                                    let forward_message = Message::PositionUpdate(position_data);
                                    if let Ok(forward_data) = serde_json::to_vec(&forward_message) {
                                        let forward_packet = forward_data.into_boxed_slice();

                                        // Send to all child peers
                                        for child_peer in known_children_peers_ref.borrow().iter() {
                                            socket
                                                .channel_mut(0)
                                                .send(forward_packet.clone(), *child_peer);
                                        }

                                        // Send to other super peers (excluding source)
                                        for other_super_peer in
                                            known_super_peers_ref.borrow().iter()
                                        {
                                            if *other_super_peer != peer {
                                                socket.channel_mut(0).send(
                                                    forward_packet.clone(),
                                                    *other_super_peer,
                                                );
                                            }
                                        }
                                    }
                                }
                            }
                            Message::PeerList(peers) => {
                                // Update known super peers list
                                let mut super_peers = known_super_peers_ref.borrow_mut();
                                for peer_str in peers {
                                    if let Ok(uuid) = Uuid::parse_str(&peer_str) {
                                        let peer_id = PeerId(uuid);
                                        if !super_peers.contains(&peer_id) {
                                            super_peers.push(peer_id);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                // Timeout and loop management
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
        });

        Ok(())
    }
}
