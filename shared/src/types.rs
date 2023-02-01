pub type PacketIndex = u16;
use bevy_reflect::prelude::*;
pub type Tick = u16;
pub type MessageId = u16;
pub type ShortMessageId = u8;
pub enum HostType {
    Server,
    Client,
}
