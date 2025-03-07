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
struct PeerPosition {
    peer_id: String,
    x: u32,
    y: u32,
}

#[derive(Serialize, Deserialize, Debug)]
enum Message {
    PositionUpdate(PeerPosition),
    Forward(PeerPosition),
    PeerList(Vec<String>),
}

#[wasm_bindgen]
pub struct Client {
    peer_id: Option<PeerId>,
    is_super: bool,
    socket: Option<WebRtcSocket>,
    super_peer: Option<PeerId>, // Each client has a designated super-peer
    connected_peers: Rc<RefCell<Vec<PeerId>>>,
    peer_positions: Rc<RefCell<HashMap<PeerId, (u32, u32)>>>,
    known_super_peers: Rc<RefCell<Vec<PeerId>>>, // Super-peers track other super-peers
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
            peer_positions: Rc::new(RefCell::new(HashMap::new())),
            connected_peers: Rc::new(RefCell::new(Vec::new())),
            known_super_peers: Rc::new(RefCell::new(Vec::new())),
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
}
