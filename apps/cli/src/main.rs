use chat_core::{ChatBehaviourEvent, generate_identity, global_topic, setup_swarm};
use chat_protocol::ChatMessage;
use chrono::Local;
use crossterm::{
    ExecutableCommand,
    event::{EventStream, KeyCode, KeyEventKind, KeyModifiers},
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use futures::StreamExt;
use libp2p::{PeerId, gossipsub, kad, mdns, swarm::SwarmEvent};
use ratatui::{
    Terminal,
    prelude::*,
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
};
use std::{collections::HashMap, error::Error};
use tokio::sync::mpsc;

use sqlx::{Row, sqlite::SqlitePoolOptions};
// use std::str::FromStr;

enum NetworkCommand {
    Publish(String),
    Dial(String),
    FindPeer(PeerId),
}

enum NetworkEvent {
    Log(String),
    PeerConnected(String, String),
    PeerDisconnected(String),
    MessageReceived(String, String),
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let (local_key, local_peer_id) = generate_identity();
    let username = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "Anonymous".to_string());

    let db_url = format!("sqlite://{}_murmur.db?mode=rwc", username);
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect(&db_url)
        .await
        .expect("Failed to connect to SQLite database");

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS messages (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            timestamp TEXT NOT NULL,
            sender TEXT NOT NULL,
            content TEXT NOT NULL
        );",
    )
    .execute(&pool)
    .await
    .expect("Failed to create database schema");

    let mut swarm = setup_swarm(local_key, local_peer_id)?;
    swarm.listen_on("/ip4/0.0.0.0/tcp/0".parse()?)?;
    let topic = global_topic();

    let (cmd_tx, mut cmd_rx) = mpsc::channel::<NetworkCommand>(100);
    let (event_tx, mut event_rx) = mpsc::channel::<NetworkEvent>(100);

    let username_net = username.clone();

    // --- TASK 1: THE NETWORK ENGINE ---
    tokio::spawn(async move {
        loop {
            tokio::select! {
                Some(cmd) = cmd_rx.recv() => match cmd {
                    NetworkCommand::Publish(line) => {
                        let msg = ChatMessage { sender: username_net.clone(), content: line.clone() };
                        if let Ok(bytes) = serde_json::to_vec(&msg) {
                            if let Err(e) = swarm.behaviour_mut().gossipsub.publish(topic.clone(), bytes) {
                                let _ = event_tx.send(NetworkEvent::Log(format!("Publish error: {e:?}"))).await;
                            } else {
                                let _ = event_tx.send(NetworkEvent::MessageReceived(username_net.clone(), line)).await;
                            }
                        }
                    }
                    NetworkCommand::Dial(addr) => {
                        if let Ok(multiaddr) = addr.parse::<libp2p::Multiaddr>() {
                            if let Err(e) = swarm.dial(multiaddr) {
                                let _ = event_tx.send(NetworkEvent::Log(format!("Dial error: {e:?}"))).await;
                            } else {
                                let _ = event_tx.send(NetworkEvent::Log(format!("Dialing {addr}..."))).await;
                            }
                        }
                    }
                    NetworkCommand::FindPeer(peer_id) => {
                        let _ = event_tx.send(NetworkEvent::Log(format!("Searching DHT for {}...", peer_id))).await;
                        swarm.behaviour_mut().kademlia.get_closest_peers(peer_id);
                    }
                },
                event = swarm.select_next_some() => match event {
                    SwarmEvent::NewListenAddr { address, .. } => {
                        let _ = event_tx.send(NetworkEvent::Log(format!("Listening on {address}"))).await;
                    }
                    SwarmEvent::Behaviour(ChatBehaviourEvent::Mdns(mdns::Event::Discovered(list))) => {
                        for (peer, addr) in list {
                            swarm.behaviour_mut().gossipsub.add_explicit_peer(&peer);
                            swarm.behaviour_mut().kademlia.add_address(&peer, addr);
                            let _ = event_tx.send(NetworkEvent::PeerConnected(peer.to_string(), format!("Unknown ({})", &peer.to_string()[..8]))).await;
                        }
                    }
                    SwarmEvent::Behaviour(ChatBehaviourEvent::Mdns(mdns::Event::Expired(list))) => {
                        for (peer, _) in list {
                            swarm.behaviour_mut().gossipsub.remove_explicit_peer(&peer);
                            let _ = event_tx.send(NetworkEvent::PeerDisconnected(peer.to_string())).await;
                        }
                    }
                    SwarmEvent::ConnectionEstablished { peer_id, endpoint, .. } => {
                        swarm.behaviour_mut().gossipsub.add_explicit_peer(&peer_id);
                        if endpoint.is_dialer() {
                            swarm.behaviour_mut().kademlia.add_address(&peer_id, endpoint.get_remote_address().clone());
                        }
                        let _ = event_tx.send(NetworkEvent::PeerConnected(peer_id.to_string(), format!("Unknown ({})", &peer_id.to_string()[..8]))).await;
                    }
                    SwarmEvent::ConnectionClosed { peer_id, .. } => {
                        swarm.behaviour_mut().gossipsub.remove_explicit_peer(&peer_id);
                        let _ = event_tx.send(NetworkEvent::PeerDisconnected(peer_id.to_string())).await;
                    }
                    SwarmEvent::Behaviour(ChatBehaviourEvent::Gossipsub(gossipsub::Event::Message { propagation_source, message, .. })) => {
                        if let Ok(chat_msg) = serde_json::from_slice::<ChatMessage>(&message.data) {
                            let _ = event_tx.send(NetworkEvent::MessageReceived(chat_msg.sender.clone(), chat_msg.content)).await;
                            let _ = event_tx.send(NetworkEvent::PeerConnected(propagation_source.to_string(), chat_msg.sender)).await;
                        }
                    }
                    SwarmEvent::Behaviour(ChatBehaviourEvent::Kademlia(kad::Event::OutboundQueryProgressed {
                        result: kad::QueryResult::GetClosestPeers(Ok(ok)),
                        ..
                    })) => {
                        for peer_info in ok.peers {
                            let _ = event_tx
                                .send(NetworkEvent::Log(format!(
                                    "DHT found peer nearby: {}",
                                    peer_info.peer_id
                                )))
                                .await;

                            for addr in peer_info.addrs {
                                swarm
                                    .behaviour_mut()
                                    .kademlia
                                    .add_address(&peer_info.peer_id, addr.clone());
                                let _ = swarm.dial(addr);
                            }
                        }
                    }

                    _ => {}
                }
            }
        }
    });

    // --- TASK 2: THE UI ENGINE ---
    let mut messages: Vec<String> = Vec::new();
    let mut input = String::new();
    let mut crossterm_events = EventStream::new();
    let mut list_state = ListState::default();
    let mut connected_peers: HashMap<String, String> = HashMap::new();

    let rows = sqlx::query("SELECT timestamp, sender, content FROM messages ORDER BY id ASC")
        .fetch_all(&pool)
        .await
        .unwrap_or_default();

    for row in rows {
        let ts: String = row.get("timestamp");
        let snd: String = row.get("sender");
        let msg: String = row.get("content");
        messages.push(format!("[{ts}] {snd}: {msg}"));
    }

    messages.push("Starting decentralized chat node...".to_string());
    messages.push(format!("My unique Peer ID is: {local_peer_id}"));
    messages.push(format!("Logged in as: {username}"));

    // let mut history_file = OpenOptions::new()
    //     .create(true)
    //     .append(true)
    //     .open(&history_filename)
    //     .await?;

    enable_raw_mode()?;
    std::io::stdout().execute(EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(std::io::stdout()))?;

    loop {
        terminal.draw(|f| {
            let main_chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(75), Constraint::Percentage(25)])
                .split(f.area());

            let chat_chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(1), Constraint::Length(3)])
                .split(main_chunks[0]);

            let messages_list: Vec<ListItem> =
                messages.iter().map(|m| ListItem::new(m.as_str())).collect();
            let messages_widget = List::new(messages_list)
                .block(Block::default().borders(Borders::ALL).title("Chat History"));

            if !messages.is_empty() {
                list_state.select(Some(messages.len() - 1));
            }
            f.render_stateful_widget(messages_widget, chat_chunks[0], &mut list_state);

            let input_widget = Paragraph::new(input.as_str()).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("Type a message (Press Esc or Ctrl+C to quit)"),
            );
            f.render_widget(input_widget, chat_chunks[1]);

            let mut active_peers: Vec<String> = connected_peers.values().cloned().collect();
            active_peers.sort();

            let peers_list: Vec<ListItem> = active_peers
                .iter()
                .map(|p| ListItem::new(p.as_str()))
                .collect();
            let peers_widget = List::new(peers_list)
                .block(Block::default().borders(Borders::ALL).title("Online Peers"));
            f.render_widget(peers_widget, main_chunks[1]);
        })?;

        tokio::select! {
            Some(Ok(event)) = crossterm_events.next() => {
                match event {
                    crossterm::event::Event::Key(key) => {
                        if key.kind == KeyEventKind::Press {
                            match key.code {
                                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => break,
                                KeyCode::Enter => {
                                    let line = input.trim().to_string();
                                    input.clear();
                                    if line.is_empty() { continue; }

                                    if line.starts_with("/ip4/") {
                                        let _ = cmd_tx.send(NetworkCommand::Dial(line)).await;
                                    } else if line.starts_with("/find ") {
                                        let peer_str = line.trim_start_matches("/find ").trim();
                                        match peer_str.parse::<PeerId>() {
                                            Ok(peer_id) => {
                                                let _ = cmd_tx.send(NetworkCommand::FindPeer(peer_id)).await;
                                            }
                                            Err(_) => messages.push("Invalid Peer ID format.".to_string()),
                                        }
                                    } else {
                                        let _ = cmd_tx.send(NetworkCommand::Publish(line.clone())).await;
                                        let now = Local::now().format("%H:%M:%S").to_string();
                                        let display_msg = format!("[{now}] {username}: {line}");
                                        messages.push(display_msg);

                                        let _ = sqlx::query("INSERT INTO messages (timestamp, sender, content) VALUES (?, ?, ?)")
                                            .bind(&now)
                                            .bind(&username)
                                            .bind(&line)
                                            .execute(&pool)
                                            .await;
                                    }
                                }
                                KeyCode::Char(c) => input.push(c),
                                KeyCode::Backspace => { input.pop(); }
                                KeyCode::Esc => break,
                                _ => {}
                            }
                        }
                    }
                    // Focus events are ignored but keep the loop alive
                    crossterm::event::Event::FocusLost => {
                        // No action needed; raw mode stays enabled.
                    }
                    crossterm::event::Event::FocusGained => {
                        // No action needed.
                    }
                    _ => {}
                }
            }
            Some(event) = event_rx.recv() => match event {
                NetworkEvent::Log(msg) => {
                    messages.push(format!("[System] {msg}"));
                }
                NetworkEvent::MessageReceived(sender, content) => {
                    let now = Local::now().format("%H:%M:%S").to_string();
                    // Skip echo of our own published messages to avoid duplicate lines
                    if sender != username {
                        let display_msg = format!("[{now}] {sender}: {content}");
                        messages.push(display_msg);

                        let _ = sqlx::query("INSERT INTO messages (timestamp, sender, content) VALUES (?, ?, ?)")
                            .bind(&now)
                            .bind(&sender)
                            .bind(&content)
                            .execute(&pool)
                            .await;
                    }
                }
                NetworkEvent::PeerConnected(peer_id, name) => {
                    connected_peers.insert(peer_id, name);
                }
                NetworkEvent::PeerDisconnected(peer_id) => {
                    connected_peers.remove(&peer_id);
                }
            }
        }
    }

    disable_raw_mode()?;
    std::io::stdout().execute(LeaveAlternateScreen)?;

    Ok(())
}
