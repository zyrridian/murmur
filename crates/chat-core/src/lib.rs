use libp2p::{
    gossipsub, identity, mdns, swarm::NetworkBehaviour, PeerId, Swarm, SwarmBuilder,
};
use std::{
    collections::hash_map::DefaultHasher,
    error::Error,
    hash::{Hash, Hasher},
    time::Duration,
};

#[derive(NetworkBehaviour)]
pub struct ChatBehaviour {
    pub gossipsub: gossipsub::Behaviour<gossipsub::IdentityTransform, gossipsub::AllowAllSubscriptionFilter>,
    pub mdns: mdns::tokio::Behaviour,
}

pub fn generate_identity() -> (identity::Keypair, PeerId) {
    let local_key = identity::Keypair::generate_ed25519();
    let local_peer_id = PeerId::from(local_key.public());
    (local_key, local_peer_id)
}

pub fn setup_swarm(
    local_key: identity::Keypair,
    local_peer_id: PeerId,
) -> Result<Swarm<ChatBehaviour>, Box<dyn Error>> {
    let message_id_fn = |message: &gossipsub::Message| {
        let mut s = DefaultHasher::new();
        message.data.hash(&mut s);
        message.source.hash(&mut s);
        message.sequence_number.hash(&mut s);
        gossipsub::MessageId::from(s.finish().to_string())
    };

    let gossipsub_config = gossipsub::ConfigBuilder::default()
        .heartbeat_interval(Duration::from_secs(10))
        .validation_mode(gossipsub::ValidationMode::Strict)
        .message_id_fn(message_id_fn)
        .build()
        .expect("Valid config");

    let mut gossipsub = gossipsub::Behaviour::<
        gossipsub::IdentityTransform,
        gossipsub::AllowAllSubscriptionFilter,
    >::new(
        gossipsub::MessageAuthenticity::Signed(local_key.clone()),
        gossipsub_config,
    )
    .expect("Correct configuration");

    let topic = gossipsub::IdentTopic::new("global-chat");
    gossipsub.subscribe(&topic)?;

    let mdns = mdns::tokio::Behaviour::new(mdns::Config::default(), local_peer_id)?;
    let behaviour = ChatBehaviour { gossipsub, mdns };

    let swarm = SwarmBuilder::with_existing_identity(local_key)
        .with_tokio()
        .with_tcp(
            libp2p::tcp::Config::default(),
            libp2p::noise::Config::new,
            libp2p::yamux::Config::default,
        )?
        .with_behaviour(|_| behaviour)?
        .with_swarm_config(|cfg| {
            cfg.with_idle_connection_timeout(Duration::from_secs(60))
        })
        .build();

    Ok(swarm)
}

pub fn global_topic() -> gossipsub::IdentTopic {
    gossipsub::IdentTopic::new("global-chat")
}