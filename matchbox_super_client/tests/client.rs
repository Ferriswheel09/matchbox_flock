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




#[cfg(test)]
mod tests {
    use log::info;
    use matchbox_socket::Packet;
    use matchbox_socket::{PeerId, WebRtcSocket};
    use serde::{Deserialize, Serialize};
    use serde_json;
    use uuid::Uuid;
    

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