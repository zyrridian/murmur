use chat_core::{generate_identity, global_topic, setup_swarm, ChatBehaviourEvent};
use chat_protocol::ChatMessage;
use chrono::Local;
use crossterm::{
    event::{EventStream, KeyCode, KeyEventKind, KeyModifiers},
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    ExecutableCommand,
};
use futures::StreamExt;
use libp2p::{gossipsub, mdns, swarm::SwarmEvent};
use ratatui::{
    prelude::*,
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
    Terminal,
};
use std::{collections::HashMap, error::Error};
use tokio::fs::{File, OpenOptions};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let (local_key, local_peer_id) = generate_identity();
    let username = std::env::args().nth(1).unwrap_or_else(|| "Anonymous".to_string());
    let history_filename = format!("{}_history.txt", username);

    let mut swarm = setup_swarm(local_key, local_peer_id)?;
    swarm.listen_on("/ip4/0.0.0.0/tcp/0".parse()?)?;

    let topic = global_topic();

    let mut messages: Vec<String> = Vec::new();
    let mut input = String::new();
    let mut crossterm_events = EventStream::new();
    let mut list_state = ListState::default();
    let mut connected_peers: HashMap<String, String> = HashMap::new();

    if let Ok(mut file) = File::open(&history_filename).await {
        let mut contents = String::new();
        if file.read_to_string(&mut contents).await.is_ok() {
            for line in contents.lines() {
                messages.push(line.to_string());
            }
        }
    }

    messages.push(format!("Starting decentralized chat node..."));
    messages.push(format!("My unique Peer ID is: {}", local_peer_id));
    messages.push(format!("Logged in as: {}", username));

    let mut history_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&history_filename)
        .await?;

    enable_raw_mode()?;
    std::io::stdout().execute(EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(std::io::stdout()))?;

    loop {
        terminal.draw(|f| {
            let main_chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(75), Constraint::Percentage(25)].as_ref())
                .split(f.area());

            let chat_chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(1), Constraint::Length(3)].as_ref())
                .split(main_chunks[0]);

            let messages_list: Vec<ListItem> = messages.iter().map(|m| ListItem::new(m.as_str())).collect();
            let messages_widget = List::new(messages_list).block(Block::default().borders(Borders::ALL).title("Chat History"));

            if !messages.is_empty() {
                list_state.select(Some(messages.len() - 1));
            }
            f.render_stateful_widget(messages_widget, chat_chunks[0], &mut list_state);

            let input_widget = Paragraph::new(input.as_str()).block(
                Block::default().borders(Borders::ALL).title("Type a message (Press Esc or Ctrl+C to quit)"),
            );
            f.render_widget(input_widget, chat_chunks[1]);

            let mut active_peers: Vec<String> = connected_peers.values().cloned().collect();
            active_peers.sort();

            let peers_list: Vec<ListItem> = active_peers.iter().map(|p| ListItem::new(p.as_str())).collect();
            let peers_widget = List::new(peers_list).block(Block::default().borders(Borders::ALL).title("Online Peers"));
            f.render_widget(peers_widget, main_chunks[1]);
        })?;

        tokio::select! {
            Some(Ok(crossterm::event::Event::Key(key))) = crossterm_events.next() => {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => break,
                        KeyCode::Enter => {
                            let line = input.clone();
                            input.clear();
                            if line.is_empty() { continue; }

                            if line.starts_with("/ip4/") {
                                match line.parse::<libp2p::Multiaddr>() {
                                    Ok(addr) => {
                                        if let Err(e) = swarm.dial(addr.clone()) {
                                            messages.push(format!("Failed to dial {}: {:?}", addr, e));
                                        } else {
                                            messages.push(format!("Dialing {}...", addr));
                                        }
                                    }
                                    Err(e) => messages.push(format!("Invalid address format: {:?}", e)),
                                }
                            } else {
                                let chat_msg = ChatMessage {
                                    sender: username.clone(),
                                    content: line.clone(),
                                };
                                let json_bytes = serde_json::to_vec(&chat_msg).expect("Failed to serialize");

                                if let Err(e) = swarm.behaviour_mut().gossipsub.publish(topic.clone(), json_bytes) {
                                    messages.push(format!("Failed to publish: {:?}", e));
                                } else {
                                    let now = Local::now().format("%H:%M:%S").to_string();
                                    let display_msg = format!("[{}] {}: {}", now, username, line);
                                    messages.push(display_msg.clone());
                                    let _ = history_file.write_all(format!("{}\n", display_msg).as_bytes()).await;
                                }
                            }
                        }
                        KeyCode::Char(c) => input.push(c),
                        KeyCode::Backspace => { input.pop(); }
                        KeyCode::Esc => break,
                        _ => {}
                    }
                }
            }

            event = swarm.select_next_some() => match event {
                SwarmEvent::NewListenAddr { address, .. } => {
                    messages.push(format!("Node listening on: {}", address));
                }
                SwarmEvent::Behaviour(ChatBehaviourEvent::Mdns(mdns::Event::Discovered(list))) => {
                    for (peer_id, _) in list {
                        swarm.behaviour_mut().gossipsub.add_explicit_peer(&peer_id);
                        let short_id = format!("Unknown ({})", &peer_id.to_string()[..8]);
                        connected_peers.insert(peer_id.to_string(), short_id);
                    }
                }
                SwarmEvent::Behaviour(ChatBehaviourEvent::Mdns(mdns::Event::Expired(list))) => {
                    for (peer_id, _) in list {
                        swarm.behaviour_mut().gossipsub.remove_explicit_peer(&peer_id);
                        connected_peers.remove(&peer_id.to_string());
                    }
                }
                SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                    swarm.behaviour_mut().gossipsub.add_explicit_peer(&peer_id);
                    let short_id = format!("Unknown ({})", &peer_id.to_string()[..8]);
                    connected_peers.insert(peer_id.to_string(), short_id);
                }
                SwarmEvent::ConnectionClosed { peer_id, .. } => {
                    swarm.behaviour_mut().gossipsub.remove_explicit_peer(&peer_id);
                    connected_peers.remove(&peer_id.to_string());
                }
                SwarmEvent::Behaviour(ChatBehaviourEvent::Gossipsub(gossipsub::Event::Message {
                    propagation_source: peer_id,
                    message,
                    ..
                })) => {
                    if let Ok(chat_msg) = serde_json::from_slice::<ChatMessage>(&message.data) {
                        let now = Local::now().format("%H:%M:%S").to_string();
                        let display_msg = format!("[{}] {}: {}", now, chat_msg.sender, chat_msg.content);
                        messages.push(display_msg.clone());
                        let _ = history_file.write_all(format!("{}\n", display_msg).as_bytes()).await;
                        connected_peers.insert(peer_id.to_string(), chat_msg.sender);
                    } else {
                        messages.push(format!("Unknown Format: '{}'", String::from_utf8_lossy(&message.data)));
                    }
                }
                _ => {}
            }
        }
    }

    disable_raw_mode()?;
    std::io::stdout().execute(LeaveAlternateScreen)?;

    Ok(())
}