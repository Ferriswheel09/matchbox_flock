use futures::{select, FutureExt};
use futures_timer::Delay;
use log::info;
use matchbox_socket::{PeerState, WebRtcSocketBuilder};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const CHANNEL_ID: usize = 0;

#[cfg(target_arch = "wasm32")]
fn main() {
    // Setup logging for browser
    console_error_panic_hook::set_once();
    console_log::init_with_level(log::Level::Debug).unwrap();

    wasm_bindgen_futures::spawn_local(async_main());
}

#[cfg(not(target_arch = "wasm32"))]
#[tokio::main]
async fn main() {
    // Setup logging for native
    use tracing_subscriber::prelude::*;
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "full_mesh=info,matchbox_socket=info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    async_main().await
}

async fn async_main() {
    info!("Connecting to matchbox");

    let (mut socket, loop_fut) = WebRtcSocketBuilder::new("ws://localhost:3536/")
        .add_reliable_channel()
        .build();

    let loop_fut = loop_fut.fuse();
    futures::pin_mut!(loop_fut);

    let timeout = Delay::new(Duration::from_millis(100));
    futures::pin_mut!(timeout);


    loop {
        // Handle peer state changes
        for (peer, state) in socket.update_peers() {
            match state {
                PeerState::Connected => {
                    info!("Peer joined: {peer}");
                    let timestamp = current_millis();
                    let packet = format!("ping:{}", timestamp).into_bytes().into_boxed_slice();
                    socket.channel_mut(CHANNEL_ID).send(packet, peer);
                }
                PeerState::Disconnected => {
                    info!("Peer left: {peer}");
                }
            }
        }

        // Handle incoming messages
        for (peer, packet) in socket.channel_mut(CHANNEL_ID).receive() {
            let message = String::from_utf8_lossy(&packet);
            if let Some(ts) = message.strip_prefix("ping:") {
                if let Ok(sent_time) = ts.parse::<u128>() {
                    let now = current_millis();
                    info!("[FullMesh] {} ms latency from peer {}", now - sent_time, peer);
                }
            } else {
                info!("Message from {peer}: {message:?}");
            }
        }

        select! {
            _ = (&mut timeout).fuse() => {
                timeout.reset(Duration::from_millis(100));
            }
            _ = &mut loop_fut => {
                break;
            }
        }
    }
}

fn current_millis() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis()
}
