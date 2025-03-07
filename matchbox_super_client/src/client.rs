use log::info;
use matchbox_socket::Packet;
use matchbox_socket::{PeerId, WebRtcSocket};
use serde::{Deserialize, Serialize};
use serde_json;
use std::cell::RefCell;
use std::collections::HashMap;
use std::collections::VecDeque;
use std::rc::Rc;
use uuid::Uuid;
use wasm_bindgen::prelude::*;

/// Mock WebRtcSocket that captures sent messages instead of sending them.
struct MockWebRtcSocket {
    sent_messages: Rc<RefCell<VecDeque<(PeerId, Packet)>>>,
}

impl MockWebRtcSocket {
    fn new() -> Self {
        Self {
            sent_messages: Rc::new(RefCell::new(VecDeque::new())),
        }
    }

    /// Mocks the behavior of sending a message to a peer.
    fn send<T: Into<Packet>>(&self, peer: PeerId, data: T) {
        self.sent_messages
            .borrow_mut()
            .push_back((peer, data.into()));
    }

    /// Retrieves the last sent message (for testing).
    fn get_last_message(&self) -> Option<(PeerId, Packet)> {
        self.sent_messages.borrow_mut().pop_back()
    }

    /// Retrieves all sent messages.
    fn get_all_messages(&self) -> Vec<(PeerId, Packet)> {
        self.sent_messages.borrow().clone().into()
    }
}

pub trait SocketSender {
    fn send(&self, peer: PeerId, data: Packet);
}

impl SocketSender for MockWebRtcSocket {
    fn send(&self, peer: PeerId, data: Packet) {
        self.send(peer, data);
    }
}

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
    pub fn update_position(&mut self, x: i32, y: i32, x_velocity: i32, y_velocity: i32) {
        let mut position = self.position.borrow_mut();
        position.x = x;
        position.y = y;
        position.x_velocity = x_velocity;
        position.y_velocity = y_velocity;
    }

    // get_position_velocity: Return the position and velocity of the current client as a string
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
    fn broadcast_message<T: SocketSender>(&mut self, socket: &T, packet: Packet) {
        if self.is_super {
            // Need to fix this relating to the channels
            for peer in self.known_super_peers.borrow().iter() {
                // socket.channel_mut(0).send(packet.clone(), *peer);
                socket.send(*peer, packet.clone());
            }
            info!("Forwarded position update to super-peers.");
        }
    }

    // Sending method that the client can use to either
    // a. send to their correspondent super peer or
    // b. broadcast to the network if they are a regular peer
    fn send<T: SocketSender>(&mut self, socket: &T, packet: Packet) {
        if self.is_super {
            self.broadcast_message(socket, packet);
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
    // TODO: Change this to only send super peer information. Other super peers don't care about other regular peers
    fn send_super_peer_list<T: SocketSender>(&self, socket: &T) {
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
                socket.send(*peer, packet.clone());
            }
            info!("Super-peer sent peer list.");
        }
    }

    // Helper function that is necessary for new super peers entering the network
    // where given the list of super peers, add them to our local struct
    // TODO: What will happen if a peer gets promoted?
    // Maybe we send the peer list to every other peer for redundancy to limit the overhead
    fn add_super_peer_list(&mut self, peer_list: Packet) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_send_peer_list() {
        println!("Running Send Peer List");
        let client = Client::new(true); // Super-peer
        let mock_socket = MockWebRtcSocket::new();

        // Add known super-peers and connected peers
        let peer1 = PeerId(Uuid::new_v4());
        let peer2 = PeerId(Uuid::new_v4());
        let connected_peer = PeerId(Uuid::new_v4());

        client.known_super_peers.borrow_mut().push(peer1);
        client.known_super_peers.borrow_mut().push(peer2);
        client.connected_peers.borrow_mut().push(connected_peer);

        println!("Client is super-peer: {}", client.is_super);
        println!("Known super peers: {:?}", client.known_super_peers.borrow());
        println!("Connected peers: {:?}", client.connected_peers.borrow());

        // Call send_peer_list()
        client.send_super_peer_list(&mock_socket);

        // Retrieve last sent message
        let (peer, data) = mock_socket.get_last_message().expect("No message was sent");

        // Print last message (for debugging purposes)
        println!("Sent raw data to {}: {:?}", peer, data);

        // Deserialize the message
        let received_msg: Message = serde_json::from_slice(&data).expect("Failed to deserialize");

        // Print deserialized message content
        println!("Received message: {:?}", received_msg);

        // Validate that the message is a PeerList
        match received_msg {
            Message::PeerList(peers) => {
                assert_eq!(peers.len(), 2);
                assert!(peers.contains(&peer1.0.to_string()));
                assert!(peers.contains(&peer2.0.to_string()));
            }
            _ => panic!("Expected PeerList message"),
        }

        // Ensure the message was sent to the correct peer
        assert_eq!(peer, connected_peer);
    }

    #[test]
    fn test_broadcast_message() {
        // Create a super peer client
        println!("Running Test Broadcast Message");

        let mut client = Client::new(true);

        // Create a mock socket that we can inspect
        let mock_socket = MockWebRtcSocket::new();
        // Add known super-peers
        let peer1 = PeerId(Uuid::new_v4());
        let peer2 = PeerId(Uuid::new_v4());

        client.known_super_peers.borrow_mut().push(peer1);
        client.known_super_peers.borrow_mut().push(peer2);

        // Create a test packet
        let test_data = vec![1, 2, 3, 4].into_boxed_slice();
        let packet: Packet = test_data;
        println!("Client is super-peer: {}", client.is_super);
        println!("Known super peers: {:?}", client.known_super_peers.borrow());

        // Broadcast Message
        client.broadcast_message(&mock_socket, packet);

        // Check that messages were sent to all known super-peers
        let sent_messages = mock_socket.get_all_messages();
        assert_eq!(sent_messages.len(), 2);

        // Verify recipients match our expected super-peers
        let recipients: Vec<PeerId> = sent_messages.iter().map(|(peer, _)| *peer).collect();
        assert!(recipients.contains(&peer1));
        assert!(recipients.contains(&peer2));

        // Verify the packet content was correctly sent
        for (_, data) in sent_messages {
            assert_eq!(data, [1, 2, 3, 4].into());
        }
    }

    #[test]
    fn test_add_super_peers() {
        println!("Running Adding Super Peer to Local List...");

        let send_client = Client::new(true);
        let mut recv_client = Client::new(true);

        // Create a mock socket that we will inspect for message
        let mock_socket = MockWebRtcSocket::new();

        // Add known super-peers to the "sending" client
        let peer1 = PeerId(Uuid::new_v4());
        let peer2 = PeerId(Uuid::new_v4());
        let connected_peer = PeerId(Uuid::new_v4());
        send_client.known_super_peers.borrow_mut().push(peer1);
        send_client.known_super_peers.borrow_mut().push(peer2);
        send_client.connected_peers.borrow_mut().push(connected_peer);

        // Create a test packet that will have (hypothetically) been sent to our client
        send_client.send_super_peer_list(&mock_socket);

        // Retrieve last sent message
        let (peer, data) = mock_socket.get_last_message().expect("No message was sent");

        // Print last message (for debugging purposes)
        println!("Sent raw data to {}: {:?}", peer, data);

        // After sending client hypothetically sent the packet, ensure receiving client adds the clients to their list
        recv_client.add_super_peer_list(data);

        // Assert that both clients have the same super-peer list contents
        let send_peers = send_client.known_super_peers.borrow().clone();
        let recv_peers = recv_client.known_super_peers.borrow().clone();
        assert!(
            send_peers.len() == recv_peers.len()
                && send_peers.iter().all(|p| recv_peers.contains(p))
        );
    }

    // Rest of the tests for the basic functionality in the previous demo
    #[test]
    #[test]
    fn test_constructor() {
        // Test regular peer
        let client = Client::new(false);
        assert_eq!(client.is_super_peer(), false);
        assert_eq!(client.peer_id, None);
        assert_eq!(client.position.borrow().x, 10);
        assert_eq!(client.position.borrow().y, 20);
        assert_eq!(client.position.borrow().x_velocity, 0);
        assert_eq!(client.position.borrow().y_velocity, 0);
        assert_eq!(client.connected_peer_count(), 0);
        assert_eq!(client.known_super_peer_count(), 0);
        
        // Test super peer
        let super_client = Client::new(true);
        assert_eq!(super_client.is_super_peer(), true);
        assert_eq!(super_client.peer_id, None);
    }

    #[test]
    fn test_update_id() {
        let mut client = Client::new(false);
        
        // Test valid UUID
        let valid_uuid = Uuid::new_v4().to_string();
        client.update_id(valid_uuid.clone());
        assert_eq!(client.peer_id_string(), valid_uuid);
        
        // Test invalid UUID
        client.update_id("not-a-valid-uuid".to_string());
        // ID should remain unchanged
        assert_eq!(client.peer_id_string(), valid_uuid);
    }

    #[test]
    fn test_update_position() {
        let mut client = Client::new(false);
        
        // Update position and verify
        client.update_position(100, 200, 5, -3);
        assert_eq!(client.x(), 100);
        assert_eq!(client.y(), 200);
        assert_eq!(client.x_velocity(), 5);
        assert_eq!(client.y_velocity(), -3);
        
        // Update again and check changes
        client.update_position(150, 250, 0, 0);
        assert_eq!(client.x(), 150);
        assert_eq!(client.y(), 250);
        assert_eq!(client.x_velocity(), 0);
        assert_eq!(client.y_velocity(), 0);
    }

    #[test]
    fn test_position_getters() {
        let mut client = Client::new(false);
        client.update_position(123, 456, 7, 8);
        
        // Test individual getters
        assert_eq!(client.x(), 123);
        assert_eq!(client.y(), 456);
        assert_eq!(client.x_velocity(), 7);
        assert_eq!(client.y_velocity(), 8);
    }

    #[test]
    fn test_get_position_velocity() {
        let mut client = Client::new(false);
        
        // Initially the peer_id is None, so it should return an error message
        assert_eq!(client.get_position_velocity(), "Error getting Position Velocity");
        
        // Set a peer ID
        let uuid = Uuid::new_v4().to_string();
        client.update_id(uuid.clone());
        
        // Update position and verify the formatted string
        client.update_position(42, 99, 3, -1);
        let expected_format = format!(
            "Player {} is at position: X=42, Y=99, X Velocity=3, Y Velocity=-1",
            uuid
        );
        assert_eq!(client.get_position_velocity(), expected_format);
    }

    #[test]
    fn test_is_super_peer() {
        let regular_client = Client::new(false);
        let super_client = Client::new(true);
        
        assert_eq!(regular_client.is_super_peer(), false);
        assert_eq!(super_client.is_super_peer(), true);
    }

    // TODO: Need a better test for get_all_positions
    // #[test]
    // fn test_get_all_positions() {
    //     let client = Client::new(false);
        
    //     // Initially, there should be no positions
    //     let positions_js = client.get_all_positions();
    //     let positions: Vec<Position> = serde_json::from_str(&positions_js.as_string().unwrap()).unwrap_or_else(|_| Vec::new());
    //     assert_eq!(positions.len(), 0);
        
    //     // Add some peer positions
    //     let peer1 = PeerId(Uuid::new_v4());
    //     let peer2 = PeerId(Uuid::new_v4());
        
    //     client.peer_positions.borrow_mut().insert(peer1, (10, 20, 1, 2));
    //     client.peer_positions.borrow_mut().insert(peer2, (30, 40, 3, 4));
        
    //     // Get positions and verify
    //     let positions_js = client.get_all_positions();
    //     let positions: Vec<Position> = serde_json::from_str(&positions_js.as_string().unwrap()).unwrap_or_else(|_| Vec::new());
        
    //     assert_eq!(positions.len(), 2);
        
    //     // Check if both peers are included (order might be different)
    //     let contains_peer1 = positions.iter().any(|p| {
    //         p.peer_id.as_ref().map_or(false, |id| id == peer1.0.to_string().as_str()) &&
    //         p.x == 10 && p.y == 20 && p.x_velocity == 1 && p.y_velocity == 2
    //     });
        
    //     let contains_peer2 = positions.iter().any(|p| {
    //         p.peer_id.as_ref().map_or(false, |id| id == peer2.0.to_string().as_str()) &&
    //         p.x == 30 && p.y == 40 && p.x_velocity == 3 && p.y_velocity == 4
    //     });
        
    //     assert!(contains_peer1);
    //     assert!(contains_peer2);
    // }

    #[test]
    fn test_peer_id_string() {
        let mut client = Client::new(false);
        
        // Initially, peer_id is None
        assert_eq!(client.peer_id_string(), "Not set");
        
        // Set a valid peer ID
        let uuid = Uuid::new_v4().to_string();
        client.update_id(uuid.clone());
        assert_eq!(client.peer_id_string(), uuid);
    }

    #[test]
    fn test_connected_peer_count() {
        let client = Client::new(false);
        
        // Initially, there are no connected peers
        assert_eq!(client.connected_peer_count(), 0);
        
        // Add some connected peers
        let peer1 = PeerId(Uuid::new_v4());
        let peer2 = PeerId(Uuid::new_v4());
        let peer3 = PeerId(Uuid::new_v4());
        
        client.connected_peers.borrow_mut().push(peer1);
        assert_eq!(client.connected_peer_count(), 1);
        
        client.connected_peers.borrow_mut().push(peer2);
        client.connected_peers.borrow_mut().push(peer3);
        assert_eq!(client.connected_peer_count(), 3);
    }

    #[test]
    fn test_known_super_peer_count() {
        // For regular peer, it should always return 0
        let regular_client = Client::new(false);
        let peer1 = PeerId(Uuid::new_v4());
        regular_client.known_super_peers.borrow_mut().push(peer1);
        assert_eq!(regular_client.known_super_peer_count(), 0);
        
        // For super peer, it should reflect the actual count
        let super_client = Client::new(true);
        assert_eq!(super_client.known_super_peer_count(), 0);
        
        let peer2 = PeerId(Uuid::new_v4());
        let peer3 = PeerId(Uuid::new_v4());
        
        super_client.known_super_peers.borrow_mut().push(peer2);
        assert_eq!(super_client.known_super_peer_count(), 1);
        
        super_client.known_super_peers.borrow_mut().push(peer3);
        assert_eq!(super_client.known_super_peer_count(), 2);
    }
}
