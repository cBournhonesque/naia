// #[cfg(feature="bincode")]
// use serde::{Deserialize, Serialize};
//
// #[cfg(feature="bincode")]
// use bincode;
//
//
// use crate::{error::SerdeErr, reader_writer::{BitReader, BitWrite}, serde::Serde, UnsignedVariableInteger};
//
//
// impl<'a, T: Serialize + Deserialize<'a> + Clone + PartialEq> Serde for T {
//     fn ser(&self, writer: &mut dyn BitWrite) {
//         let binary = bincode::serialize(&self).unwrap();
//         let length = UnsignedVariableInteger::<5>::new(binary.len() as u64);
//         length.ser(writer);
//         binary.iter().for_each(|byte| {
//             writer.write_byte(*byte);
//         });
//     }
//
//     fn de(reader: &mut BitReader) -> Result<T, SerdeErr> {
//         let length_int = UnsignedVariableInteger::<5>::de(reader)?;
//         let length_usize = length_int.get() as usize;
//         let mut output: Vec<u8> = Vec::with_capacity(length_usize);
//         for _ in 0..length_usize {
//             output.push(reader.read_byte()?);
//         }
//         let res = bincode::deserialize::<T>(output.as_slice())?;
//         Ok(res)
//     }
// }