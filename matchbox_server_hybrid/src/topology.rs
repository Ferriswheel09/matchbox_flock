
use crate::state::HybridState;
use async_trait::async_trait;
use axum::extract::ws::Message;
use futures::StreamExt;
use matchbox_protocol::{JsonPeerEvent, PeerRequest};
use matchbox_signaling::{
    common_logic::parse_request, ClientRequestError, NoCallbacks, SignalingTopology, WsStateMeta, 
};
use tracing::{error, info, warn};

pub const PEER_RATIO: usize = 4;
    
/// A hybrid network topology
#[derive(Debug, Default)]
pub struct HybridTopology;

#[async_trait]
impl SignalingTopology<NoCallbacks, HybridState> for HybridTopology{
    async fn state_machine(upgrade: WsStateMeta<NoCallbacks, HybridState>) {
        let WsStateMeta {
            peer_id,
            sender,
            mut receiver,
            mut state,
            ..
        } = upgrade;

        if state.should_add_super_peer(PEER_RATIO) {
            //TODO: Implement logic to assign super peers based off bandwidth
            //-----------------------------------------------------------------------------------
            // If an existing child have higher bandwidth than new peer
            //      Make that child a new super peer and make new peer a child peer, load balance
            // Else
            //      Make new peer a super peer, load balance
            //-----------------------------------------------------------------------------------
            state.add_super_peer(peer_id, sender.clone());
            state.load_balance();
            info!("Added super peer {peer_id}");
        } else {
            //TODO: Implement logic to handle new child peers based off bandwidth
            //-----------------------------------------------------------------------------------
            // If the new peer a better candidate for super peer than existing super peers
            //      Replace worst existing super peer with new peer, transfer children to new peer
            //      Add old super peer as child of least loaded super peer
            // Else
            //      Add new peer as  child of least loaded super peer
            //-----------------------------------------------------------------------------------
            state.add_child_peer(peer_id, sender.clone());
            if let Some(parent) = state.find_least_loaded_super_peer() {
                match state.connect_child(peer_id, parent) {
                    Ok(_) => {
                        info!("Connected CHILD:{peer_id} to PARENT:{parent}");
                        state.connect_child_parent(peer_id, parent, sender.clone());
                    }
                    Err(e) => {
                        error!("error sending peer {peer_id} to super: {e:?}");
                        return;
                    }
                }
            }
            else {
                error!("error finding super peer");
            }
        }

         // The state machine for the data channel established for this websocket.
         while let Some(request) = receiver.next().await {
            let request = match parse_request(request) {
                Ok(request) => request,
                Err(e) => {
                    match e {
                        ClientRequestError::Axum(_) => {
                            // Most likely a ConnectionReset or similar.
                            warn!("Unrecoverable error with {peer_id}: {e:?}");
                        }
                        ClientRequestError::Close => {
                            info!("Connection closed by {peer_id}");
                        }
                        ClientRequestError::Json(_) | ClientRequestError::UnsupportedType(_) => {
                            error!("Error with request: {e:?}");
                            continue; // Recoverable error
                        }
                    };
                    if state.is_super_peer(&peer_id){
                        state.remove_super_peer(peer_id);

                    } else {
                        state.remove_child_peer(peer_id);
                    }
                    return;
                }
            };

            match request {
                PeerRequest::Signal { receiver, data } => {
                    let event = Message::Text(
                        JsonPeerEvent::Signal {
                            sender: peer_id,
                            data,
                        }
                        .to_string(),
                    );
                    if let Err(e) = {
                        state.try_send_to_peer(receiver, event)
                    } {
                        error!("error sending signal event: {e:?}");
                    }
                }
                // TODO: Add custom hybrid events
                // PeerRequest::GetChildren
                // PeerRequest::UpdateBandwidth

                PeerRequest::KeepAlive => {
                    // Do nothing. KeepAlive packets are used to protect against idle websocket
                    // connections getting automatically disconnected, common for reverse proxies.
                }
            }
        }
        
        // If we have reached here the peer is leaving the network
        if state.is_super_peer(&peer_id) {
            // TODO: Implement dynamic super peer removal taking bandwidth into consideration
            //-----------------------------------------------------------------------------------
            // If child peer to super peer ratio low
            //      Remove super peer and load balance
            // Else 
            //      Find child peer with highest bandwidth
            //      Make that child peer a super peer
            //      Transfer children of leaving super peer to new super peer
            //      Remove leaving super peer
            //-----------------------------------------------------------------------------------
            state.remove_super_peer(peer_id);
        } else {
            state.remove_child_peer(peer_id);
        }
        
    }
}

#[cfg(test)]
mod tests {
    use futures::StreamExt;
    use matchbox_protocol::{JsonPeerEvent, PeerId}; 
    use matchbox_signaling::{SignalingServer, SignalingServerBuilder};
    use std::{net::Ipv4Addr, str::FromStr};
    use tokio::net::TcpStream;
    use tokio_tungstenite::{tungstenite::Message, MaybeTlsStream, WebSocketStream};
    use tracing::info;

    use crate::{state::HybridState, topology::PEER_RATIO};

    use super::HybridTopology;

    /// Helper to build hybrid signaling server
    fn app() -> SignalingServer {
        let state = HybridState::default();
        SignalingServerBuilder::new((Ipv4Addr::LOCALHOST, 0), HybridTopology, state.clone())
            .on_connection_request(|connection| {
                info!("Connecting: {connection:?}");
                Ok(true) // Allow all connections
            })
            .on_id_assignment(|(socket, id)| info!("{socket} received {id}"))
            .cors()
            .trace()
            .build()
    }

    /// Helper to take the next PeerEvent from a stream
    async fn recv_peer_event(
        client: &mut WebSocketStream<MaybeTlsStream<TcpStream>>,
    ) -> JsonPeerEvent {
        let message: Message = client.next().await.unwrap().unwrap();
        JsonPeerEvent::from_str(&message.to_string()).expect("json peer event")
    }

    /// Helper to extract PeerId when expecting an Id assignment
    fn get_peer_id(peer_event: JsonPeerEvent) -> PeerId {
        if let JsonPeerEvent::IdAssigned(id) = peer_event {
            id
        } else {
            panic!("Peer_event was not IdAssigned: {peer_event:?}");
        }
    }
    /// Testing peer can connect to signaling server
    #[tokio::test]
    async fn ws_connect() {
        let mut server = app();
        let addr = server.bind().unwrap();
        tokio::spawn(server.serve());

        tokio_tungstenite::connect_async(format!("ws://{addr}/room_a"))
            .await
            .expect("handshake");
    }

    /// Testing peer is assigned an id upon initial connection
    #[tokio::test]
    async fn uuid_assigned() {
        let mut server = app();        
        let addr = server.bind().unwrap();
        tokio::spawn(server.serve());

        let (mut peer, _response) = tokio_tungstenite::connect_async(format!("ws://{addr}/room_a"))
            .await
            .unwrap();

        let id_assigned_event = recv_peer_event(&mut peer).await;

        assert!(matches!(id_assigned_event, JsonPeerEvent::IdAssigned(..)));
    }

    /// Testing a super peer can make up to PEER_RATIO connections with connecting child peers
    #[tokio::test]
    async fn super_peer_gets_peer_ratio_children() {
        let mut server = app(); 
        let addr = server.bind().unwrap();
        tokio::spawn(server.serve());

        let (mut super_peer, _response) = tokio_tungstenite::connect_async(format!("ws://{addr}/room_a"))
            .await
            .unwrap();
        let _super_peer_uuid = get_peer_id(recv_peer_event(&mut super_peer).await);

        let mut child_peers: Vec<(PeerId, WebSocketStream<MaybeTlsStream<TcpStream>>)> = Vec::new();

        for _ in 0..PEER_RATIO {
            let (mut child_peer,  _response) =  tokio_tungstenite::connect_async(format!("ws://{addr}/room_a"))
                .await
                .unwrap();
            let child_peer_uuid = get_peer_id(recv_peer_event(&mut child_peer).await);

            child_peers.push((child_peer_uuid, child_peer));
        }

            
        let mut received_new_peers: Vec<PeerId> = Vec::new();
        while received_new_peers.len() < PEER_RATIO {
            let event = recv_peer_event(&mut super_peer).await;

            match event {
                JsonPeerEvent::NewPeer(peer_id) => received_new_peers.push(peer_id),
                _ => assert!(false),
            }
        }

        let expected: std::collections::HashSet<_> = child_peers.into_iter().map(|(id, _)| id).collect();
        let actual: std::collections::HashSet<_> = received_new_peers.into_iter().collect();

        assert_eq!(expected, actual, "Super peer did not receive all child's NewPeer events");
    }

    /// Testing that upon the connection of a second super peer, existing children in the network are spread evenly
    #[tokio::test]
    async fn new_super_peer_then_rebalance() {
        let mut server = app(); 
        let addr = server.bind().unwrap();
        tokio::spawn(server.serve());

        let (mut super_peer_a, _response) = tokio_tungstenite::connect_async(format!("ws://{addr}/room_a"))
            .await
            .unwrap();
        let _super_peer_a_uuid = get_peer_id(recv_peer_event(&mut super_peer_a).await);

        let mut child_peers: Vec<WebSocketStream<MaybeTlsStream<TcpStream>>> = Vec::new();

        for _ in 0..PEER_RATIO {
            let (mut child_peer,  _response) =  tokio_tungstenite::connect_async(format!("ws://{addr}/room_a"))
                .await
                .unwrap();
            let _child_peer_uuid = get_peer_id(recv_peer_event(&mut child_peer).await);

            child_peers.push(child_peer);
        }

            
        let mut received_new_peers: Vec<PeerId> = Vec::new();
        while received_new_peers.len() < PEER_RATIO {
            let event = recv_peer_event(&mut super_peer_a).await;

            match event {
                JsonPeerEvent::NewPeer(peer_id) => received_new_peers.push(peer_id),
                _ => assert!(false),
            } 
        }

        let (mut super_peer_b, _response) = tokio_tungstenite::connect_async(format!("ws://{addr}/room_a"))
            .await
            .unwrap();
        let super_peer_b_uuid = get_peer_id(recv_peer_event(&mut super_peer_b).await);

        let mut former_children: Vec<PeerId> = Vec::new();
        while former_children.len() < PEER_RATIO / 2 {
            let event = recv_peer_event(&mut super_peer_a).await;

            match event {
                JsonPeerEvent::PeerLeft(peer_id) => former_children.push(peer_id),
                JsonPeerEvent::NewPeer(_) => assert_eq!(event, JsonPeerEvent::NewPeer(super_peer_b_uuid)),
                _ => assert!(false),
            }
        }
 
        let mut new_children: Vec<PeerId> = Vec::new();
        while new_children.len() < PEER_RATIO / 2 {
            let event = recv_peer_event(&mut super_peer_b).await;
            match event {
                JsonPeerEvent::NewPeer(peer_id) => new_children.push(peer_id),
                _ => assert!(false),
            }
        }

        let super_peer_a_left_children: std::collections::HashSet<_> = former_children.into_iter().collect();
        let super_peer_b_joined_children: std::collections::HashSet<_> = new_children.into_iter().collect();

        assert_eq!(super_peer_a_left_children, super_peer_b_joined_children, "Children were not transfered correctly");
    }

    /// Testing that a child can smoothly disconnect from the network
    #[tokio::test]
    async fn child_peer_disconnecting() {
        let mut server = app(); 
        let addr = server.bind().unwrap();
        tokio::spawn(server.serve());

        let (mut super_peer, _response) = tokio_tungstenite::connect_async(format!("ws://{addr}/room_a"))
            .await
            .unwrap();
        let _super_peer_uuid = get_peer_id(recv_peer_event(&mut super_peer).await);

        let (mut child_peer,  _response) =  tokio_tungstenite::connect_async(format!("ws://{addr}/room_a"))
            .await
            .unwrap();
        let child_peer_uuid = get_peer_id(recv_peer_event(&mut child_peer).await);

        let event = recv_peer_event(&mut super_peer).await;

        assert_eq!(event, JsonPeerEvent::NewPeer(child_peer_uuid));

        let _ = child_peer.close(None).await;

        let event = recv_peer_event(&mut super_peer).await;

        assert_eq!(event, JsonPeerEvent::PeerLeft(child_peer_uuid));
    }

    /// Testing that a super peer can smoothly disconnect from the network and have one of its children be promoted to take its place
    #[tokio::test]
    async fn super_peer_disconnecting() {
        let mut server = app(); 
        let addr = server.bind().unwrap();
        tokio::spawn(server.serve());

        let (mut super_peer, _response) = tokio_tungstenite::connect_async(format!("ws://{addr}/room_a"))
            .await
            .unwrap();
        let super_peer_uuid = get_peer_id(recv_peer_event(&mut super_peer).await);

        let mut child_peers: Vec<(PeerId, WebSocketStream<MaybeTlsStream<TcpStream>>)> = Vec::new();

        for _ in 0..PEER_RATIO {
            let (mut child_peer,  _response) =  tokio_tungstenite::connect_async(format!("ws://{addr}/room_a"))
                .await
                .unwrap();
            let child_peer_uuid = get_peer_id(recv_peer_event(&mut child_peer).await);

            child_peers.push((child_peer_uuid, child_peer));
        }
 
        for _ in 0..PEER_RATIO {
            let event = recv_peer_event(&mut super_peer).await;

            match event {
                JsonPeerEvent::NewPeer(_) => {} ,
                _ => assert!(false),
            }
        }

        let _ = super_peer.close(None).await;

        for i in 0..PEER_RATIO {
            let event = recv_peer_event(&mut child_peers[i].1).await;

            match event {
                JsonPeerEvent::PeerLeft(id) => assert_eq!(id, super_peer_uuid) ,
                _ => assert!(false),
            }
        }

        let mut new_children: Vec<PeerId> = Vec::new();
        let mut new_super_peer = child_peers[0].0;
        let mut index = 0;
        for i in 1..PEER_RATIO {
            if child_peers[i].0 < new_super_peer{
                new_super_peer = child_peers[i].0;
                index = i;
            }
        }

        let mut new_super_peer = child_peers.remove(index);
        while new_children.len() < PEER_RATIO - 1 {
            let event = recv_peer_event(&mut new_super_peer.1).await;

            match event {
                JsonPeerEvent::NewPeer(id) => new_children.push(id),
                _ => assert!(false),
            }
        }

        let expected: std::collections::HashSet<_> = child_peers.into_iter().map(|(id, _)| id).collect();
        let actual: std::collections::HashSet<_> = new_children.into_iter().collect();

        assert_eq!(expected, actual);
    }
}

