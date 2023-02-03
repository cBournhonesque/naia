use std::marker::PhantomData;

use naia_serde::{BitReader, Serde, SerdeErr, UnsignedVariableInteger};

use crate::{messages::message_channel::ChannelReader, types::MessageIndex};

pub struct IndexedMessageReader<P> {
    phantom_p: PhantomData<P>,
}

impl<P> IndexedMessageReader<P> {
    pub fn read_messages(
        channel_reader: &dyn ChannelReader<P>,
        reader: &mut BitReader,
    ) -> Result<Vec<(MessageIndex, P)>, SerdeErr> {
        let mut last_read_id: Option<MessageIndex> = None;
        let mut output = Vec::new();

        loop {
            let channel_continue = bool::de(reader)?;
            if !channel_continue {
                break;
            }

            let id_w_msg = Self::read_message(channel_reader, reader, &last_read_id)?;
            last_read_id = Some(id_w_msg.0);
            output.push(id_w_msg);
        }

        Ok(output)
    }

    fn read_message(
        channel_reader: &dyn ChannelReader<P>,
        reader: &mut BitReader,
        last_read_id: &Option<MessageIndex>,
    ) -> Result<(MessageIndex, P), SerdeErr> {
        let message_id: MessageIndex = if let Some(last_id) = last_read_id {
            let id_diff = UnsignedVariableInteger::<3>::de(reader)?.get() as MessageIndex;
            last_id.wrapping_add(id_diff)
        } else {
            // read message id
            MessageIndex::de(reader)?
        };

        // read payload
        let new_message = channel_reader.read(reader)?;

        Ok((message_id, new_message))
    }
}
