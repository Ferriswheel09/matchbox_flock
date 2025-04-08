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

/// A single Peer type that can be either a child or super peer.
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
    
    /// Returns true if this peer is a super peer.
    pub fn is_super(&self) -> bool {
        matches!(self.role, PeerRole::Super { .. })
    }
}

/// Signaling server state for hybrid topologies
#[derive(Default, Debug, Clone)]
pub struct HybridState {
    pub peers: StateObj<HashMap<PeerId, Peer>>,
}
impl SignalingState for HybridState {}  

impl HybridState {

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

    /// Check if a new super peer should be added based on the ratio of child to super peers
    pub fn should_add_super_peer(&self, peer_ratio: usize) -> bool {
        let peers = self.peers.lock().unwrap();

        let (mut child_count, mut super_count) = (0usize, 0usize);

        for peer in peers.values() {
            match &peer.role {
                PeerRole::Child { .. } => child_count += 1,
                PeerRole::Super { .. } => super_count += 1,
            }
        }

        if super_count == 0 {
            return true;
        }

        let ratio = child_count as f64 / super_count as f64;
        ratio >= peer_ratio as f64
    }

    /// Returns super peer with the least number of children then lowest ID
    pub fn find_least_loaded_super_peer(&self) -> Option<PeerId> {
        let peers = self.peers.lock().unwrap();
        peers
            .values()
            .filter(|peer| matches!(peer.role, PeerRole::Super { .. }))
            .min_by_key(|peer| {
                match &peer.role {
                    PeerRole::Super { children } => (children.len(), peer.id),
                    _ => (usize::MAX, peer.id), 
                }
            })
            .map(|peer| peer.id)
    }

    /// Add a new super peer
    pub fn add_super_peer(&mut self, peer: PeerId, sender: SignalingChannel) {
        let event = Message::Text(JsonPeerEvent::NewPeer(peer).to_string());
        let super_peers = { self.get_super_peer_ids()};

        super_peers.iter().for_each(|peer_id| {
            if let Err(e) = self.try_send_to_peer(*peer_id, event.clone()) {
                error!("error informing {peer_id} of new super peer {peer}: {e:?}");
            }
        });
        
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
        } // <-- The lock is dropped here

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
                        info!("Removed child peer {child_id} with no parent");
                    }
                    PeerRole::Super { .. } => {
                        peers.insert(child_id, child_peer);
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
        let former_children: HashSet<PeerId>;
        let other_super_ids: Vec<PeerId>;
        {
            let mut peers_map = self.peers.lock().unwrap();

            if let Some(removed_peer) = peers_map.remove(&super_id) {
                match removed_peer.role {
                    PeerRole::Super { children } => {
                        former_children = children;
                        other_super_ids = peers_map
                            .values()
                            .filter(|p| matches!(p.role, PeerRole::Super { .. }))
                            .map(|p| p.id)
                            .collect();
                    }
                    _ => {
                        error!("Peer {super_id} was not actually a super peer!");
                        peers_map.insert(super_id, removed_peer);
                        return;
                    }
                }
            } else {
                error!("Tried removing unknown super peer {super_id}");
                return;
            }
        } // <-- lock is dropped here

        // Notify other super peers that `super_id` left.
        let event = Message::Text(JsonPeerEvent::PeerLeft(super_id).to_string());
        for sp_id in &other_super_ids {
            if let Err(e) = self.try_send_to_peer(*sp_id, event.clone()) {
                error!("Failed to send PeerLeft({super_id}) to super peer {sp_id}: {e:?}");
            }
        }

        // Notify the children that their super peer is gone.
        for &child_id in &former_children {
            if let Err(e) = self.try_send_to_peer(child_id, event.clone()) {
                error!("Failed to send PeerLeft({super_id}) to child {child_id}: {e:?}");
            }
        }

        // Promote one of the children to be the new super peer.
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

            // Re‐attach the other children to the new super peer
            for &child_id in &former_children {
                if child_id != recruit_id {
                    if let Err(e) = self.connect_child(child_id, recruit_id) {
                        error!("Failed to connect child {child_id} to new super {recruit_id}: {e:?}");
                    }
                }
            }
        }
    }

    /// Transfers child with min id from giver to receiever super peers
    pub fn move_child(&mut self, giver_id: PeerId, receiver_id: PeerId){

        let child_id = {

            if let Some(giver) = self.peers.lock().unwrap().get_mut(&giver_id) {
                match &mut giver.role {
                    PeerRole::Super { children } => {
                        let cid = children.iter().min().cloned();
                        if cid.is_some() {
                            children.remove(&cid.unwrap());
                            cid.unwrap()
                        }
                        else {
                            error!("peer {giver_id} has no children to give");
                            return
                        }
                    }
            
                    _ => {
                            error!("trying to move child from {giver_id} but they are not a super peer");
                            return
                    }
                }
            }
            else {
                error!("{giver_id} does not exist in the network");
                return
            }
        };

        let event = Message::Text(JsonPeerEvent::PeerLeft(child_id).to_string());
        match self.try_send_to_peer(giver_id, event) {
            Ok(()) => info!("Notified parent {giver_id} of child peer {child_id} removal"),
            Err(e) => error!("Failed to notify parent {giver_id} of child removal: {e:?}"),
        }
    
        let event = Message::Text(JsonPeerEvent::PeerLeft(giver_id).to_string());
        match self.try_send_to_peer(child_id, event) {
            Ok(()) => info!("Notified child {child_id} of parent {giver_id} disconnection"),
            Err(e) => error!("Failed to notify child {child_id} of parent disconnection: {e:?}"),
        }
    
        match self.connect_child(child_id, receiver_id) {
            Ok(()) => info!("Connected child {child_id} to parent {giver_id}"),
            Err(e) => error!("Failed to notify parent {giver_id} of child connection: {e:?}"),
        }   
    }

    pub fn load_balance(&mut self) {

        // Get list of super peers and their child counts
        let mut super_info: Vec<(PeerId, usize)> = {
            let peers = self.peers.lock().unwrap();
            peers
                .values()
                .filter_map(|peer| {
                    if let PeerRole::Super { children } = &peer.role {
                        Some((peer.id, children.len()))
                    } else {
                        None
                    }
                })
                .collect()
        };

        // If there are less than 2 super peers, we can't balance
        if super_info.len() < 2 {
            return;
        }

        // Sort super peers by number of children
        super_info.sort_by_key(|&(_id, child_count)| child_count);
        
        // Calculate the average number of children per super peer
        let total_children: usize = super_info.iter().map(|(_, c)| c).sum();
        let num_supers = super_info.len();
        let average = total_children / num_supers;
        let remainder = total_children % num_supers;

        let mut num_moves = 0;

        let mut target_info: Vec<(PeerId, usize, usize)> = Vec::new();

        for n in 0..super_info.len() {
            if n < remainder {
                target_info.push((super_info[n].0, super_info[n].1, average + 1)); 
            }
            else {
                target_info.push((super_info[n].0, super_info[n].1, average)); 
            }
        }

        let mut overloaded_idx: Vec<usize> = Vec::new();
        let mut underloaded_idx: Vec<usize> = Vec::new();

        for n in 0..target_info.len() {
            if target_info[n].1 > target_info[n].2 {
                overloaded_idx.push(n);
            }
            else if target_info[n].1 < target_info[n].2{
                underloaded_idx.push(n);
            }
        }

        let mut i:usize= 0;
        let mut j:usize = 0;
        while i < overloaded_idx.len() && j < underloaded_idx.len() {
            if target_info[overloaded_idx[i]].1 <= target_info[overloaded_idx[i]].2 {
                i += 1;
                continue;
            }
            if target_info[underloaded_idx[j]].1 >= target_info[underloaded_idx[j]].2 {
                j += 1;
                continue;
            }

            target_info[overloaded_idx[i]].1 -= 1;
            target_info[underloaded_idx[j]].1 += 1;
            self.move_child(target_info[overloaded_idx[i]].0, target_info[underloaded_idx[j]].0);
            num_moves += 1;
        }

        info!("{num_moves} reassignments made in load balancing");

    }
    // TODO: Logic for super peer assignment according to bandwidth
    //-------------------------------------------------------------------
    // demote_super_peer(&mut self, peer: &PeerId)
    // promote_child_peer(&mut self, peer: &PeerId, children: Vec<PeerId>)
    // lowest_bandwidth_super_peer(&self)
    // highest_bandwidth_child_peer(&self)
    //-------------------------------------------------------------------

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
}
