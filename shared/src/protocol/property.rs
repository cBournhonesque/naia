use std::fmt::{Debug, Display, Formatter};
use std::ops::{Deref, DerefMut, Add, AddAssign, Mul, MulAssign};


use ::serde::{Serialize, Deserialize, Serializer, Deserializer};
use naia_serde::{BitReader, BitWrite, BitWriter, Serde, SerdeErr};

use crate::protocol::property_mutate::PropertyMutator;
use crate::protocol::replicable_property::ReplicableProperty;

/// A Property of an Component/Message, that contains data
/// which must be tracked for updates
#[derive(Clone)]
pub struct Property<T: Serde> {
    inner: T,
    mutator: Option<PropertyMutator>,
    mutator_index: u8,
}

impl<T: Serde> PartialEq for Property<T> where T: PartialEq {
    fn eq(&self, other: &Self) -> bool {
        self.inner.eq(&other.inner)
    }
}

impl<T: Serde> Serialize for Property<T> where T: Serialize {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error> where S: Serializer {
        self.inner.serialize(serializer)
    }
}


/// Deserialize the property according to the underlying field's deserializer
/// Again, same issues as with Default, we had to use 0 as mutator index
impl<'de, T: Serde> Deserialize<'de> for Property<T> where T: Deserialize<'de> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error> where D: Deserializer<'de> {
        let inner = T::deserialize(deserializer)?;
        Ok(Self::new(inner, 0))
    }
}



impl<T: Serde> Add for Property<T>
where T: Add<Output = T> {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        let mut res = self.clone();
        *res = self.inner + rhs.inner;
        res
    }
}

impl<T: Serde> AddAssign for Property<T>
    where T: AddAssign {

    fn add_assign(&mut self, rhs: Self) {
        *self.deref_mut() += rhs.inner;
    }
}

impl<T: Serde> Mul<f32> for Property<T>
    where T: Mul<f32, Output = T> {
    type Output = Self;

    fn mul(self, rhs: f32) -> Self::Output {
        let mut res = self.clone();
        *res = self.inner * rhs;
        res
    }
}

impl<T: Serde> MulAssign<f32> for Property<T>
    where T: MulAssign<f32> {

    fn mul_assign(&mut self, rhs: f32) {
        *self.deref_mut() *= rhs;
    }
}

impl<T: Serde> Debug for Property<T>
where T: Debug {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        self.inner.fmt(f)
    }
}

impl<T: Serde> Display for Property<T>
where T: Display {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        self.inner.fmt(f)
    }
}

/// Implement default for Property<T>
/// Note that this is invalid and shouldn't be used because the mutator_index is arbitrarily set to 0
/// It's mostly useful for bevy_inspector_egui
/// TODO: instead, impl Default on the replicate struct, and use new(default(), default(), etc.) or new_complete
///  so we dont need to implement default here
impl<T: Serde> Default for Property<T>
where T: Default {
    fn default() -> Self {
        Self::new(T::default(), 0)
    }
}

cfg_if! {
    if #[cfg(feature = "bevy_support")]
    {
        use std::any::Any;
        use bevy_reflect::{Reflect, ReflectMut, ReflectOwned, ReflectRef, TypeInfo};

        impl<T: Serde> Reflect for Property<T> where T: Reflect {
            fn type_name(&self) -> &str {
                self.inner.type_name()
            }

            fn get_type_info(&self) -> &'static TypeInfo {
                self.inner.get_type_info()
            }

            fn into_any(self: Box<Self>) -> Box<dyn Any> {
                Box::new(self.inner).into_any()
            }

            fn as_any(&self) -> &dyn Any {
                self.inner.as_any()
            }

            fn as_any_mut(&mut self) -> &mut dyn Any {
                self.inner.as_any_mut()
            }

            fn into_reflect(self: Box<Self>) -> Box<dyn Reflect> {
                Box::new(self.inner).into_reflect()
            }

            fn as_reflect(&self) -> &dyn Reflect {
                self.inner.as_reflect()
            }

            fn as_reflect_mut(&mut self) -> &mut dyn Reflect {
                self.inner.as_reflect_mut()
            }

            fn apply(&mut self, value: &dyn Reflect) {
                self.inner.apply(value)
            }

            fn set(&mut self, value: Box<dyn Reflect>) -> Result<(), Box<dyn Reflect>> {
                self.inner.set(value)
            }

            fn reflect_ref(&self) -> ReflectRef {
                self.inner.reflect_ref()
            }

            fn reflect_mut(&mut self) -> ReflectMut {
                self.inner.reflect_mut()
            }

            fn reflect_owned(self: Box<Self>) -> ReflectOwned {
                Box::new(self.inner).reflect_owned()
            }

            fn clone_value(&self) -> Box<dyn Reflect> {
                self.inner.clone_value()
            }
}

    }

}


impl<T: Serde> Property<T> {
    fn read_inner(reader: &mut BitReader) -> Result<T, SerdeErr> {
        T::de(reader)
    }
}

// should be shared
impl<T: Serde> ReplicableProperty for Property<T> {
    type Inner = T;

    /// Create a new Property
    fn new(value: Self::Inner, mutator_index: u8) -> Self {
        Property::<T> {
            inner: value,
            mutator: None,
            mutator_index,
        }
    }

    /// Set value to the value of another Property, queues for update if value
    /// changes
    fn mirror(&mut self, other: &Self) {
        **self = (**other).clone();
    }

    // Serialization / deserialization

    /// Writes contained value into outgoing byte stream
    fn write(&self, writer: &mut dyn BitWrite) {
        self.inner.ser(writer);
    }

    /// Given a cursor into incoming packet data, initializes the Property with
    /// the synced value
    fn new_read(reader: &mut BitReader, mutator_index: u8) -> Result<Self, SerdeErr> {
        let inner = Self::read_inner(reader)?;

        Ok(Property::<T> {
            inner,
            mutator: None,
            mutator_index,
        })
    }

    /// Reads from a stream and immediately writes to a stream
    /// Used to buffer updates for later
    fn read_write(reader: &mut BitReader, writer: &mut BitWriter) -> Result<(), SerdeErr> {
        T::de(reader)?.ser(writer);
        Ok(())
    }

    /// Given a cursor into incoming packet data, updates the Property with the
    /// synced value
    fn read(&mut self, reader: &mut BitReader) -> Result<(), SerdeErr> {
        self.inner = Self::read_inner(reader)?;
        Ok(())
    }

    // Comparison

    /// Compare to another property
    fn equals(&self, other: &Self) -> bool {
        self.inner == other.inner
    }

    // Internal

    /// Set an PropertyMutator to track changes to the Property
    fn set_mutator(&mut self, mutator: &PropertyMutator) {
        self.mutator = Some(mutator.clone_new());
    }
}

// It could be argued that Property here is a type of smart-pointer,
// but honestly this is mainly for the convenience of type coercion
impl<T: Serde> Deref for Property<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<T: Serde> DerefMut for Property<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // Just assume inner value will be changed, queue for update
        if let Some(mutator) = &mut self.mutator {
            mutator.mutate(self.mutator_index);
        }
        &mut self.inner
    }
}
