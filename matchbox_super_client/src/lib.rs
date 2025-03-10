use log::info;
use matchbox_socket::Packet;
use matchbox_socket::{PeerId, WebRtcSocket};
use serde::{Deserialize, Serialize};
use serde_json;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use uuid::Uuid;
use wasm_bindgen::prelude::*;

#[derive(Serialize, Deserialize, Debug, Clone)]
struct Position {
    #[serde(skip_serializing_if = "Option::is_none")]
    peer_id: Option<String>,
    x: i32,
    y: i32,
    x_velocity: i32, 
    y_velocity: i32
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
                y_velocity: 0
            }))
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
            },
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
    pub fn get_position_velocity(&self) -> String{
        match &self.peer_id {
            Some(id) => format!(
                "Player {} is at position: X={}, Y={}, X Velocity={}, Y Velocity={}",
                id, self.position.borrow().x, self.position.borrow().y, self.position.borrow().x_velocity, self.position.borrow().y_velocity
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
                y_velocity
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
                if let Some(socket) = &mut self.socket{
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
                if let Some(socket) = &mut self.socket{
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
}


