//! # Naia Client
//! A cross-platform client that can send/receive events to/from a server, and
//! has a pool of in-scope replicates that are synced with the server.

#![deny(
    missing_docs,
    missing_debug_implementations,
    trivial_casts,
    trivial_numeric_casts,
    unsafe_code,
    unstable_features,
    unused_import_braces
)]

mod client;
mod client_config;
mod command_receiver;
mod command_sender;
mod connection_state;
mod dual_command_receiver;
mod dual_command_sender;
mod error;
mod event;
mod packet_writer;
mod ping_manager;
mod replicate_action;
mod replicate_manager;
mod server_connection;
mod tick_manager;
mod tick_queue;

pub use naia_shared::{
    find_my_ip_address, wrapping_diff, Instant, LinkConditionerConfig, LocalComponentKey,
    LocalEntityKey, LocalObjectKey, LocalReplicateKey, NaiaKey, Random, Ref, Replicate,
};

pub use client::Client;
pub use client_config::ClientConfig;
pub use event::Event;
pub use naia_client_socket::Packet;
