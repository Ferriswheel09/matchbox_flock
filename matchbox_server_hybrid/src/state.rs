use axum::extract::ws::Message;
use matchbox_protocol::{PeerId, JsonPeerEvent};
use matchbox_signaling::{
    common_logic::{StateObj, SignalingChannel, try_send},
    SignalingError, SignalingState,
};
use std::collections::{HashMap, HashSet};
use tracing::{error, info};

/// Enum representing whether a peer is a child or super peer (and associated data).
#[derive(Debug)]
pub enum PeerRole {
    /// Child peer: stores the ID of its parent super peer, if any.
    Child {
        parent_id: Option<PeerId>,
    },
    /// Super peer: stores a set of child peers.
    Super {
        children: HashSet<PeerId>,
    },
}

/// A single “Peer” type that can be either a child or super peer.
#[derive(Debug)]
pub struct Peer {
    /// The peer’s unique ID.
    pub id: PeerId,
    /// The peer’s role (child or super).
    pub role: PeerRole,
    /// Channel to communicate with the signaling server.
    pub signaling_channel: SignalingChannel,
}

impl Peer {
    /// Create a new Peer as a Child (default role).
    pub fn new_child(id: PeerId, signaling_channel: SignalingChannel) -> Self {
        Peer {
            id,
            role: PeerRole::Child { parent_id: None },
            signaling_channel,
        }
    }

    /// Create a new Peer as a Super Peer.
    pub fn new_super(id: PeerId, signaling_channel: SignalingChannel) -> Self {
        Peer {
            id,
            role: PeerRole::Super {
                children: HashSet::new(),
            },
            signaling_channel,
        }
    }
    
    /// Returns `true` if this peer is a super peer.
    pub fn is_super(&self) -> bool {
        matches!(self.role, PeerRole::Super { .. })
    }

    /// Returns `true` if this peer is a child peer.
    pub fn is_child(&self) -> bool {
        matches!(self.role, PeerRole::Child { .. })
    }
}

/// Signaling server state for hybrid topologies
#[derive(Default, Debug, Clone)]
pub struct HybridState {
    pub peers: StateObj<HashMap<PeerId, Peer>>,
}
impl SignalingState for HybridState {}  

impl HybridState {
    // TODO: implement functions to be called in state machine logic

    /// Get number of super peers connected
    pub fn get_num_super_peers(&self) -> usize {
        let peers = self.peers.lock().unwrap();
        peers
            .values()
            .filter(|peer| matches!(peer.role, PeerRole::Super { .. }))
            .count()
    }

    /// Get number of child peers connected
    pub fn get_num_child_peers(&self) -> usize {
        let peers = self.peers.lock().unwrap();
        peers
            .values()
            .filter(|peer| matches!(peer.role, PeerRole::Child { .. }))
            .count()
    }

    /// Check if a given `PeerId` corresponds to a super peer
    pub fn is_super_peer(&self, peer_id: &PeerId) -> bool {
        let peers = self.peers.lock().unwrap();
        if let Some(peer) = peers.get(peer_id) {
            matches!(peer.role, PeerRole::Super { .. })
        } else {
            false
        }
    }

    /// Get all super peer IDs
    pub fn get_super_peer_ids(&self) -> Vec<PeerId> {
        self.peers
            .lock()
            .unwrap()
            .values()
            .filter(|peer| peer.is_super())
            .map(|peer| peer.id)
            .collect()
    }

    /// Get all child peer IDs
    pub fn get_child_peer_ids(&self) -> Vec<PeerId> {
        self.peers
            .lock()
            .unwrap()
            .values()
            .filter(|peer| peer.is_child())
            .map(|peer| peer.id)
            .collect()
    }

    /// Returns super peer with the least number of children then lowest ID
    pub fn find_least_loaded_super_peer(&self) -> Option<PeerId> {
        let peers = self.peers.lock().unwrap();
        peers
            .values()
            // Filter only super peers
            .filter(|peer| matches!(peer.role, PeerRole::Super { .. }))
            // Among super peers, find the one with the smallest (children.len(), id)
            .min_by_key(|peer| {
                match &peer.role {
                    PeerRole::Super { children } => (children.len(), peer.id),
                    _ => (usize::MAX, peer.id), 
                }
            })
            // Return the selected peer’s ID
            .map(|peer| peer.id)
    }

    /// Add a new super peer
    pub fn add_super_peer(&mut self, peer: PeerId, sender: SignalingChannel) {
        // Alert all super peers of new super peer
        let event = Message::Text(JsonPeerEvent::NewPeer(peer).to_string());
        // Safety: Lock must be scoped/dropped to ensure no deadlock with loop
        let super_peers = { self.get_super_peer_ids()};

        super_peers.iter().for_each(|peer_id| {
            if let Err(e) = self.try_send_to_peer(*peer_id, event.clone()) {
                error!("error informing {peer_id} of new super peer {peer}: {e:?}");
            }
        });
        // Safety: All prior locks in this method must be freed prior to this call
        self.peers.lock().as_mut().unwrap().insert(peer, Peer::new_super(peer, sender));
    }

    /// Add child peer
    pub fn add_child_peer(&mut self, peer: PeerId, sender: SignalingChannel) {
        self.peers.lock()
                        .as_mut()
                        .unwrap()
                        .insert(peer, Peer::new_child(peer, sender));
    }

    /// Connect child to super peer
    pub fn connect_child(&mut self, child_id: PeerId, super_id: PeerId) -> Result<(), SignalingError> {
        {
            let mut peers = self.peers.lock().unwrap();
            
            {
                // Find the super peer in the map.
                let super_peer = peers
                    .get_mut(&super_id)
                    .ok_or(SignalingError::UnknownPeer)?;

                // Ensure the peer is actually a Super peer, then add the child to its set.
                match &mut super_peer.role {
                    PeerRole::Super { children } => {
                        children.insert(child_id);
                    }
                    _ => {
                        error!("Peer {super_id} is not a super peer");
                        return Err(SignalingError::UnknownPeer); // IncorrectRole would be a better error
                    }
                }
            }
            {
                // Find the child peer in the map.
                let child_peer = peers
                    .get_mut(&child_id)
                    .ok_or(SignalingError::UnknownPeer)?;

                // Ensure the peer is actually a Child, then set its parent_id.
                match &mut child_peer.role {
                    PeerRole::Child { parent_id } => {
                        *parent_id = Some(super_id);
                    }
                    _ => {
                        error!("Peer {child_id} is not a child peer");
                        return Err(SignalingError::UnknownPeer); // IncorrectRole would be a better error
                    }
                }
            }
        }
        // Safety: All prior locks in this method must be freed prior to this call
        let event = Message::Text(JsonPeerEvent::NewPeer(child_id).to_string());
        self.try_send_to_peer(super_id, event)
    }   
    
    /// Disconnect child peer from super peer
    pub fn remove_child_peer(&mut self, child_id: PeerId) {
        // Store the parent ID outside the lock, so we can notify them after dropping the lock.
        let mut parent_id: Option<PeerId> = None;
        {
            // Lock the peers map and remove the child
            let mut peers = self.peers.lock().unwrap();

            // Remove the child from the map entirely
            if let Some(child_peer) = peers.remove(&child_id) {
                // Check if this peer was actually a Child
                match child_peer.role {
                    PeerRole::Child { parent_id: Some(pid) } => {
                        // Remove this child from the parent's 'children' set
                        if let Some(parent_peer) = peers.get_mut(&pid) {
                            if let PeerRole::Super { children } = &mut parent_peer.role {
                                children.remove(&child_id);
                            }
                        }
                        // Parent will be notified after releasing the lock
                        parent_id = Some(pid);
                    }
                    PeerRole::Child { parent_id: None } => {
                        // It was a child with no parent; nothing else to do
                        info!("Removed child peer {child_id} with no parent");
                    }
                    PeerRole::Super { .. } => {
                        // The function name is remove_child_peer, but we found a Super?
                        // Could log an error or handle differently:
                        error!("Peer {child_id} was actually a super peer, not a child");
                    }
                }
            } else {
                // The child_id wasn't in the map
                error!("Peer {child_id} was not found in peers map");
                return;
            }
        } // <-- The lock is dropped here

        // Now notify the parent (if any) without holding the lock
        if let Some(pid) = parent_id {
            let event = Message::Text(JsonPeerEvent::PeerLeft(child_id).to_string());
            match self.try_send_to_peer(pid, event) {
                Ok(()) => info!("Notified parent {pid} of child peer {child_id} removal"),
                Err(e) => error!("Failed to notify parent {pid} of child removal: {e:?}"),
            }
        }
    }

    pub fn remove_super_peer(&mut self, super_id: PeerId) {
        // We will gather the children of the removed super peer here.
        let mut former_children: HashSet<PeerId> = HashSet::new();
        // We'll also gather the IDs of the other super peers to notify.
        let mut other_super_ids: Vec<PeerId> = Vec::new();

        {
            // 1) Lock the peers map and remove this super peer.
            let mut peers_map = self.peers.lock().unwrap();

            let maybe_super_peer = peers_map.remove(&super_id);
            if let Some(removed_peer) = maybe_super_peer {
                match removed_peer.role {
                    // If this really was a super peer, capture its children.
                    PeerRole::Super { children } => {
                        former_children = children;

                        // Gather all other super peer IDs (to tell them that `super_id` left).
                        other_super_ids = peers_map
                            .values()
                            .filter(|p| matches!(p.role, PeerRole::Super { .. }))
                            .map(|p| p.id)
                            .collect();
                    }
                    // If we discovered it wasn’t actually a super peer, log an error (or handle differently).
                    _ => {
                        error!("Peer {super_id} was not actually a super peer!");
                        // Possibly re‐insert it if that was unexpected:
                        // peers_map.insert(super_id, removed_peer);
                        return;
                    }
                }
            } else {
                error!("Tried removing unknown super peer {super_id}");
                return;
            }
        } // <-- lock is dropped here

        // 2) Notify other super peers that `super_id` left.
        let event = Message::Text(JsonPeerEvent::PeerLeft(super_id).to_string());
        for sp_id in &other_super_ids {
            if let Err(e) = self.try_send_to_peer(*sp_id, event.clone()) {
                error!("Failed to send PeerLeft({super_id}) to super peer {sp_id}: {e:?}");
            }
        }

        // 3) Notify the children that their super peer is gone.
        for &child_id in &former_children {
            if let Err(e) = self.try_send_to_peer(child_id, event.clone()) {
                error!("Failed to send PeerLeft({super_id}) to child {child_id}: {e:?}");
            }
        }

        // 4) (Optional) “Promote” one of the children to be the new super peer.
        //    You can pick the smallest ID, or any other selection strategy.
        if let Some(&recruit_id) = former_children.iter().min() {
            // Turn `recruit_id` into a super peer in place, then re‐attach the other children to it.
            // We do so in a new scope, so the lock is short‐lived.
            {
                let mut peers_map = self.peers.lock().unwrap();

                // If the recruit still exists in the map, update it.
                if let Some(recruit) = peers_map.get_mut(&recruit_id) {
                    // If it was a child, clear its parent and set it to super.
                    match &mut recruit.role {
                        PeerRole::Child { parent_id } => {
                            *parent_id = None; // no parent now
                        }
                        // If it’s already a super, or something else, we might handle differently
                        _ => {}
                    }
                    // Now make it a super with an empty children set, 
                    // or you might directly insert the “former_children minus itself.”
                    recruit.role = PeerRole::Super {
                        children: HashSet::new(),
                    };
                } else {
                    error!("Recruit peer {recruit_id} was not found in the map");
                }
            } // lock dropped

            info!("Promoted peer {recruit_id} to super peer");

            // 5) Re‐attach the other children to the new super peer
            for &child_id in &former_children {
                if child_id != recruit_id {
                    if let Err(e) = self.connect_child(child_id, recruit_id) {
                        error!("Failed to connect child {child_id} to new super {recruit_id}: {e:?}");
                    }
                }
            }
        }
    }

    /// Send signaling message to peer
    pub fn try_send_to_peer(&self, peer: PeerId, message: Message) -> Result<(), SignalingError> {
        self.peers
            .lock()
            .as_mut()
            .unwrap()
            .get(&peer)
            .ok_or_else(|| SignalingError::UnknownPeer)
            .and_then(|sender| try_send(&sender.signaling_channel, message))      
    }

    // demote_super_peer(&mut self, peer: &PeerId)

    // promote_child_peer(&mut self, peer: &PeerId, children: Vec<PeerId>)

    // 
}
