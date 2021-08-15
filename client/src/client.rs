use std::net::SocketAddr;

use byteorder::{BigEndian, ReadBytesExt, WriteBytesExt};

use naia_client_socket::{ClientSocket, ClientSocketTrait, MessageSender};

pub use naia_shared::{
    ConnectionConfig, HostTickManager, Instant, LocalComponentKey, LocalEntityKey, LocalObjectKey,
    LocalReplicateKey, ManagerType, Manifest, PacketReader, PacketType, PawnKey, ProtocolType,
    Replicate, SequenceIterator, SharedConfig, StandardHeader, Timer, Timestamp,
};

use super::{
    client_config::ClientConfig,
    connection_state::{ConnectionState, ConnectionState::AwaitingChallengeResponse},
    error::NaiaClientError,
    event::Event,
    replicate_action::ReplicateAction,
    server_connection::ServerConnection,
    tick_manager::TickManager,
    Packet,
};

/// Client can send/receive events to/from a server, and has a pool of in-scope
/// replicates that are synced with the server
#[derive(Debug)]
pub struct Client<T: ProtocolType> {
    manifest: Manifest<T>,
    server_address: SocketAddr,
    connection_config: ConnectionConfig,
    socket: Box<dyn ClientSocketTrait>,
    sender: MessageSender,
    server_connection: Option<ServerConnection<T>>,
    pre_connection_timestamp: Option<Timestamp>,
    pre_connection_digest: Option<Box<[u8]>>,
    handshake_timer: Timer,
    connection_replicate: ConnectionState,
    auth_event: Option<T>,
    tick_manager: TickManager,
}

impl<T: ProtocolType> Client<T> {
    /// Create a new client, given the server's address, a shared manifest, an
    /// optional Config, and an optional Authentication event
    pub fn new(
        manifest: Manifest<T>,
        client_config: Option<ClientConfig>,
        shared_config: SharedConfig,
        auth: Option<T>,
    ) -> Self {
        let client_config = match client_config {
            Some(config) => config,
            None => ClientConfig::default(),
        };

        let server_address = client_config.server_address;

        let connection_config = ConnectionConfig::new(
            client_config.disconnection_timeout_duration,
            client_config.heartbeat_interval,
            client_config.ping_interval,
            client_config.rtt_sample_size,
        );

        let mut client_socket = ClientSocket::connect(server_address);
        if let Some(config) = shared_config.link_condition_config {
            client_socket = client_socket.with_link_conditioner(&config);
        }

        let mut handshake_timer = Timer::new(client_config.send_handshake_interval);
        handshake_timer.ring_manual();
        let message_sender = client_socket.get_sender();

        Client {
            server_address,
            manifest,
            socket: client_socket,
            sender: message_sender,
            connection_config,
            handshake_timer,
            server_connection: None,
            pre_connection_timestamp: None,
            pre_connection_digest: None,
            connection_replicate: AwaitingChallengeResponse,
            auth_event: auth,
            tick_manager: TickManager::new(shared_config.tick_interval),
        }
    }

    /// Must call this regularly (preferably at the beginning of every draw
    /// frame), in a loop until it returns None.
    /// Retrieves incoming events/updates, and performs updates to maintain the
    /// connection.
    pub fn receive(&mut self) -> Option<Result<Event<T>, NaiaClientError>> {
        // send ticks, handshakes, heartbeats, pings, timeout if need be
        match &mut self.server_connection {
            Some(connection) => {
                // process replays
                connection.process_replays();
                // receive event
                if let Some(event) = connection.get_incoming_event() {
                    return Some(Ok(Event::Message(event)));
                }
                // receive replicate action
                while let Some(action) = connection.get_incoming_replicate_action() {
                    let event_opt: Option<Event<T>> = {
                        match action {
                            ReplicateAction::CreateObject(local_key) => {
                                Some(Event::CreateObject(local_key))
                            }
                            ReplicateAction::DeleteObject(local_key, replicate) => {
                                Some(Event::DeleteObject(local_key, replicate.clone()))
                            }
                            ReplicateAction::UpdateObject(local_key) => {
                                Some(Event::UpdateObject(local_key))
                            }
                            ReplicateAction::AssignPawn(local_key) => {
                                Some(Event::AssignPawn(local_key))
                            }
                            ReplicateAction::UnassignPawn(local_key) => {
                                Some(Event::UnassignPawn(local_key))
                            }
                            ReplicateAction::ResetPawn(local_key) => {
                                Some(Event::ResetPawn(local_key))
                            }
                            ReplicateAction::CreateEntity(local_key, component_list) => {
                                Some(Event::CreateEntity(local_key, component_list))
                            }
                            ReplicateAction::DeleteEntity(local_key) => {
                                Some(Event::DeleteEntity(local_key))
                            }
                            ReplicateAction::AssignPawnEntity(local_key) => {
                                Some(Event::AssignPawnEntity(local_key))
                            }
                            ReplicateAction::UnassignPawnEntity(local_key) => {
                                Some(Event::UnassignPawnEntity(local_key))
                            }
                            ReplicateAction::ResetPawnEntity(local_key) => {
                                Some(Event::ResetPawnEntity(local_key))
                            }
                            ReplicateAction::AddComponent(entity_key, component_key) => {
                                Some(Event::AddComponent(entity_key, component_key))
                            }
                            ReplicateAction::UpdateComponent(entity_key, component_key) => {
                                Some(Event::UpdateComponent(entity_key, component_key))
                            }
                            ReplicateAction::RemoveComponent(
                                entity_key,
                                component_key,
                                component,
                            ) => Some(Event::RemoveComponent(
                                entity_key,
                                component_key,
                                component.clone(),
                            )),
                        }
                    };
                    match event_opt {
                        Some(event) => {
                            return Some(Ok(event));
                        }
                        None => {
                            continue;
                        }
                    }
                }
                // receive replay command
                if let Some((pawn_key, command)) = connection.get_incoming_replay() {
                    match pawn_key {
                        PawnKey::Object(object_key) => {
                            return Some(Ok(Event::ReplayCommand(
                                object_key,
                                command.as_ref().to_protocol(),
                            )));
                        }
                        PawnKey::Entity(entity_key) => {
                            return Some(Ok(Event::ReplayCommandEntity(
                                entity_key,
                                command.as_ref().to_protocol(),
                            )));
                        }
                    }
                }
                // receive command
                if let Some((pawn_key, command)) = connection.get_incoming_command() {
                    match pawn_key {
                        PawnKey::Object(object_key) => {
                            return Some(Ok(Event::NewCommand(
                                object_key,
                                command.as_ref().to_protocol(),
                            )));
                        }
                        PawnKey::Entity(entity_key) => {
                            return Some(Ok(Event::NewCommandEntity(
                                entity_key,
                                command.as_ref().to_protocol(),
                            )));
                        }
                    }
                }
                // update current tick
                // apply updates on tick boundary
                if connection.frame_begin(&self.manifest, &mut self.tick_manager) {
                    return Some(Ok(Event::Tick));
                }
                // drop connection if necessary
                if connection.should_drop() {
                    self.server_connection = None;
                    self.pre_connection_timestamp = None;
                    self.pre_connection_digest = None;
                    self.connection_replicate = AwaitingChallengeResponse;
                    return Some(Ok(Event::Disconnection));
                } else {
                    // send heartbeats
                    if connection.should_send_heartbeat() {
                        Client::internal_send_with_connection(
                            self.tick_manager.get_client_tick(),
                            &mut self.sender,
                            connection,
                            PacketType::Heartbeat,
                            Packet::empty(),
                        );
                    }
                    // send pings
                    if connection.should_send_ping() {
                        let ping_payload = connection.get_ping_payload();
                        Client::internal_send_with_connection(
                            self.tick_manager.get_client_tick(),
                            &mut self.sender,
                            connection,
                            PacketType::Ping,
                            ping_payload,
                        );
                    }
                    // send a packet
                    while let Some(payload) = connection
                        .get_outgoing_packet(self.tick_manager.get_client_tick(), &self.manifest)
                    {
                        self.sender
                            .send(Packet::new_raw(payload))
                            .expect("send failed!");
                        connection.mark_sent();
                    }
                }
            }
            None => {
                if self.handshake_timer.ringing() {
                    match self.connection_replicate {
                        ConnectionState::AwaitingChallengeResponse => {
                            if self.pre_connection_timestamp.is_none() {
                                self.pre_connection_timestamp = Some(Timestamp::now());
                            }

                            let mut timestamp_bytes = Vec::new();
                            self.pre_connection_timestamp
                                .as_mut()
                                .unwrap()
                                .write(&mut timestamp_bytes);
                            Client::<T>::internal_send_connectionless(
                                &mut self.sender,
                                PacketType::ClientChallengeRequest,
                                Packet::new(timestamp_bytes),
                            );
                        }
                        ConnectionState::AwaitingConnectResponse => {
                            // write timestamp & digest into payload
                            let mut payload_bytes = Vec::new();
                            self.pre_connection_timestamp
                                .as_mut()
                                .unwrap()
                                .write(&mut payload_bytes);
                            for digest_byte in self.pre_connection_digest.as_ref().unwrap().as_ref()
                            {
                                payload_bytes.push(*digest_byte);
                            }
                            // write auth event replicate if there is one
                            if let Some(auth_event) = &mut self.auth_event {
                                let type_id = auth_event.get_type_id();
                                let naia_id = self.manifest.get_naia_id(&type_id); // get naia id
                                payload_bytes.write_u16::<BigEndian>(naia_id).unwrap(); // write naia id
                                auth_event.write(&mut payload_bytes);
                            }
                            Client::<T>::internal_send_connectionless(
                                &mut self.sender,
                                PacketType::ClientConnectRequest,
                                Packet::new(payload_bytes),
                            );
                        }
                        _ => {}
                    }

                    self.handshake_timer.reset();
                }
            }
        }

        // receive from socket
        loop {
            match self.socket.receive() {
                Ok(event) => {
                    if let Some(packet) = event {
                        let server_connection_wrapper = self.server_connection.as_mut();

                        if let Some(server_connection) = server_connection_wrapper {
                            server_connection.mark_heard();

                            let (header, payload) = StandardHeader::read(packet.payload());
                            server_connection
                                .process_incoming_header(&header, &mut self.tick_manager);

                            match header.packet_type() {
                                PacketType::Data => {
                                    server_connection.buffer_data_packet(
                                        header.host_tick(),
                                        header.local_packet_index(),
                                        &payload,
                                    );
                                    continue;
                                }
                                PacketType::Heartbeat => {
                                    continue;
                                }
                                PacketType::Pong => {
                                    server_connection.process_pong(&payload);
                                    continue;
                                }
                                _ => {}
                            }
                        } else {
                            let (header, payload) = StandardHeader::read(packet.payload());
                            match header.packet_type() {
                                PacketType::ServerChallengeResponse => {
                                    if self.connection_replicate
                                        == ConnectionState::AwaitingChallengeResponse
                                    {
                                        if let Some(my_timestamp) = self.pre_connection_timestamp {
                                            let mut reader = PacketReader::new(&payload);
                                            let server_tick = reader
                                                .get_cursor()
                                                .read_u16::<BigEndian>()
                                                .unwrap();
                                            let payload_timestamp = Timestamp::read(&mut reader);

                                            if my_timestamp == payload_timestamp {
                                                let mut digest_bytes: Vec<u8> = Vec::new();
                                                for _ in 0..32 {
                                                    digest_bytes.push(reader.read_u8());
                                                }
                                                self.pre_connection_digest =
                                                    Some(digest_bytes.into_boxed_slice());

                                                self.tick_manager.set_initial_tick(server_tick);

                                                self.connection_replicate =
                                                    ConnectionState::AwaitingConnectResponse;
                                            }
                                        }
                                    }

                                    continue;
                                }
                                PacketType::ServerConnectResponse => {
                                    let server_connection = ServerConnection::new(
                                        self.server_address,
                                        &self.connection_config,
                                    );

                                    self.server_connection = Some(server_connection);
                                    self.connection_replicate = ConnectionState::Connected;
                                    return Some(Ok(Event::Connection));
                                }
                                _ => {}
                            }
                        }
                    } else {
                        break;
                    }
                }
                Err(error) => {
                    return Some(Err(NaiaClientError::Wrapped(Box::new(error))));
                }
            }
        }

        return None;
    }

    /// Queues up an Message to be sent to the Server
    pub fn send_message(&mut self, message: &impl Replicate<T>, guaranteed_delivery: bool) {
        if let Some(connection) = &mut self.server_connection {
            connection.queue_message(message, guaranteed_delivery);
        }
    }

    /// Queues up a Pawn Object Command to be sent to the Server
    pub fn send_command(&mut self, pawn_object_key: &LocalObjectKey, command: &impl Replicate<T>) {
        if let Some(connection) = &mut self.server_connection {
            connection.replicate_queue_command(pawn_object_key, command);
        }
    }

    /// Queues up a Pawn Entity Command to be sent to the Server
    pub fn entity_send_command(
        &mut self,
        pawn_entity_key: &LocalEntityKey,
        command: &impl Replicate<T>,
    ) {
        if let Some(connection) = &mut self.server_connection {
            connection.entity_queue_command(pawn_entity_key, command);
        }
    }

    /// Get the address currently associated with the Server
    pub fn server_address(&self) -> SocketAddr {
        return self.server_address;
    }

    /// Return whether or not a connection has been established with the Server
    pub fn has_connection(&self) -> bool {
        return self.server_connection.is_some();
    }

    // objects

    /// Get a reference to an Object currently in scope for the Client, given
    /// that Object's Key
    pub fn get_object(&self, key: &LocalObjectKey) -> Option<&T> {
        if let Some(connection) = &self.server_connection {
            return connection.get_object(key);
        }
        return None;
    }

    /// Get whether or not the Object currently in scope for the Client, given
    /// that Object's Key
    pub fn has_object(&self, key: &LocalObjectKey) -> bool {
        if let Some(connection) = &self.server_connection {
            return connection.has_object(key);
        }
        return false;
    }

    /// Component-themed alias for `get_object`
    pub fn get_component(&self, key: &LocalComponentKey) -> Option<&T> {
        return self.get_object(key);
    }

    /// Get whether or not the Component currently in scope for the Client,
    /// given that Component's Key
    pub fn has_component(&self, key: &LocalComponentKey) -> bool {
        if let Some(connection) = &self.server_connection {
            return connection.has_component(key);
        }
        return false;
    }

    /// Return an iterator to the collection of keys to all Replicates tracked
    /// by the Client
    pub fn object_keys(&self) -> Option<Vec<LocalObjectKey>> {
        if let Some(connection) = &self.server_connection {
            return Some(connection.object_keys());
        }
        return None;
    }

    /// Return an iterator to the collection of keys to all Components tracked
    /// by the Client
    pub fn component_keys(&self) -> Option<Vec<LocalComponentKey>> {
        if let Some(connection) = &self.server_connection {
            return Some(connection.component_keys());
        }
        return None;
    }

    // pawns

    /// Get a reference to a Pawn
    pub fn get_pawn(&self, key: &LocalObjectKey) -> Option<&T> {
        if let Some(connection) = &self.server_connection {
            return connection.get_pawn(key);
        }
        return None;
    }

    /// Get a reference to a Pawn, used for setting it's replicate
    pub fn get_pawn_mut(&mut self, key: &LocalObjectKey) -> Option<&T> {
        if let Some(connection) = self.server_connection.as_mut() {
            return connection.get_pawn_mut(key);
        }
        return None;
    }

    /// Return an iterator to the collection of keys to all Pawns tracked by
    /// the Client
    pub fn pawn_keys(&self) -> Option<Vec<LocalObjectKey>> {
        if let Some(connection) = &self.server_connection {
            return Some(
                connection
                    .pawn_keys()
                    .cloned()
                    .collect::<Vec<LocalObjectKey>>(),
            );
        }
        return None;
    }

    // entities

    /// Get whether or not the Entity currently in scope for the Client, given
    /// that Entity's Key
    pub fn has_entity(&self, key: &LocalEntityKey) -> bool {
        if let Some(connection) = &self.server_connection {
            return connection.has_entity(key);
        }
        return false;
    }

    // connection metrics

    /// Gets the average Round Trip Time measured to the Server
    pub fn get_rtt(&self) -> f32 {
        return self.server_connection.as_ref().unwrap().get_rtt();
    }

    /// Gets the average Jitter measured in connection to the Server
    pub fn get_jitter(&self) -> f32 {
        return self.server_connection.as_ref().unwrap().get_jitter();
    }

    // ticks

    /// Gets the current tick of the Client
    pub fn get_client_tick(&self) -> u16 {
        return self.tick_manager.get_client_tick();
    }

    /// Gets the last received tick from the Server
    pub fn get_server_tick(&self) -> u16 {
        return self
            .server_connection
            .as_ref()
            .unwrap()
            .get_last_received_tick();
    }

    // interpolation

    /// Gets the interpolation tween amount for the current frame
    pub fn get_interpolation(&self) -> f32 {
        self.tick_manager.fraction
    }

    // internal functions

    fn internal_send_with_connection(
        host_tick: u16,
        sender: &mut MessageSender,
        connection: &mut ServerConnection<T>,
        packet_type: PacketType,
        packet: Packet,
    ) {
        let new_payload = connection.process_outgoing_header(
            host_tick,
            connection.get_last_received_tick(),
            packet_type,
            packet.payload(),
        );
        sender
            .send(Packet::new_raw(new_payload))
            .expect("send failed!");
        connection.mark_sent();
    }

    fn internal_send_connectionless(
        sender: &mut MessageSender,
        packet_type: PacketType,
        packet: Packet,
    ) {
        let new_payload =
            naia_shared::utils::write_connectionless_payload(packet_type, packet.payload());
        sender
            .send(Packet::new_raw(new_payload))
            .expect("send failed!");
    }
}
